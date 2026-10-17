//! `pragma backtest`: recent history checked against today's lock, counting what each commit
//! introduced that pragma would have stopped. Each run saves its report for the app.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use pragma_engine::backtest::{self, CACHE_FILE, REPORT_FILE};
use pragma_engine::lock::{LOCK_FILE, Lock};
use pragma_engine::overrides::{OVERRIDES_FILE, Overrides};
use pragma_engine::rules::CompiledLock;
use pragma_engine::writes::write_atomic;

use crate::json::BacktestOutput;
use crate::output::print_json;
use crate::repo_here;

/// Hits listed under each rule in the human output.
const SHOWN_PER_RULE: usize = 5;

/// "1 commit", "2 commits".
fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        _ => format!("{count} {noun}s"),
    }
}

/// The line that says some commits were not fully checked: a partial clone lacks their files, so
/// the counts leave them out.
fn incomplete_note(commits: usize) -> String {
    let whose = if commits == 1 { "its" } else { "their" };
    format!(
        "{} could not be fully checked: {whose} files are not in this clone.",
        plural(commits, "commit")
    )
}

pub fn run(cwd: &Path, depth: usize, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let dir = repo
        .effective_pragma_dir()
        .context("this repo has no .pragma/; run pragma init")?;
    let lock = Lock::load(&dir.join(LOCK_FILE))?;
    let compiled = CompiledLock::compile(&lock)?;
    let exceptions = Overrides::load(&dir.join(OVERRIDES_FILE))?.exceptions()?;
    let report = backtest::run(
        &repo.toplevel,
        &compiled,
        &exceptions,
        &repo.shared_state_dir().join(CACHE_FILE),
        depth,
    )?;
    let output = BacktestOutput {
        report,
        rules: lock.clauses.len(),
        rules_checked: backtest::rules_checked(&compiled),
    };
    write_atomic(
        &repo.state_dir().join(REPORT_FILE),
        serde_json::to_string_pretty(&output)?.as_bytes(),
    )?;

    if json {
        return print_json(&output);
    }
    let report = &output.report;
    println!(
        "Checked {} against {}.",
        plural(report.commits_checked, "commit"),
        plural(output.rules_checked, "rule")
    );
    if report.commits_checked < report.depth {
        // The walk stopped at the repo's first commit, which has nothing before it to compare with.
        let commits = report.commits_checked + usize::from(!report.head.is_empty());
        if commits < report.depth {
            println!(
                "The history has only {}, fewer than the {} asked for.",
                plural(commits, "commit"),
                report.depth
            );
        }
        if !report.head.is_empty() {
            println!(
                "The first commit is not checked: there is nothing before it to compare with."
            );
        }
    }
    if output.rules_checked == 0 {
        println!("None of the rules in the lock can be checked against history.");
        return Ok(());
    }
    if report.hits.is_empty() {
        // Nothing found is no all-clear when some commits could not be read.
        match report.incomplete_commits {
            0 => println!("No violations in recent history."),
            incomplete => println!("{}", incomplete_note(incomplete)),
        }
        return Ok(());
    }
    println!(
        "pragma would have stopped {} these commits introduced.",
        plural(report.total, "violation")
    );
    // The rules broken most come first; ties keep the lock's order.
    let order: HashMap<&str, usize> = compiled
        .clauses()
        .enumerate()
        .map(|(index, clause)| (clause.id.as_str(), index))
        .collect();
    let mut broken: Vec<(&String, &usize)> = report.counts.iter().collect();
    broken.sort_by_key(|(id, count)| {
        (
            std::cmp::Reverse(**count),
            order.get(id.as_str()).copied().unwrap_or(usize::MAX),
        )
    });
    for (id, count) in broken {
        match compiled.clause(id) {
            Some(clause) => println!(
                "\n{count} x {} ({}:{})",
                clause.text, clause.anchor.path, clause.anchor.lines[0]
            ),
            None => println!("\n{count} x {id}"),
        }
        let hits: Vec<_> = report
            .hits
            .iter()
            .filter(|hit| &hit.finding.clause_id == id)
            .collect();
        for hit in hits.iter().take(SHOWN_PER_RULE) {
            let place = match hit.finding.line {
                0 => hit.finding.file.clone(),
                line => format!("{}:{line}", hit.finding.file),
            };
            println!("    {}  {place}  {}", hit.short, hit.subject);
        }
        if hits.len() > SHOWN_PER_RULE {
            println!("    and {} more", hits.len() - SHOWN_PER_RULE);
        }
    }
    if report.incomplete_commits > 0 {
        println!("\n{}", incomplete_note(report.incomplete_commits));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::incomplete_note;

    #[test]
    fn the_incomplete_note_agrees_with_its_count() {
        assert_eq!(
            incomplete_note(1),
            "1 commit could not be fully checked: its files are not in this clone."
        );
        assert_eq!(
            incomplete_note(2),
            "2 commits could not be fully checked: their files are not in this clone."
        );
    }
}
