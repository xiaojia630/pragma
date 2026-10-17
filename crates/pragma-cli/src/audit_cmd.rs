//! `pragma audit`: the whole tree checked against the lock's rules. The result is the repo's
//! baseline; each run adds a line to the history. An audit of an area, or of what a branch
//! changed, is a partial view and is never saved.

use std::path::Path;

use anyhow::{Context, Result};
use pragma_engine::audit;
use pragma_engine::docs::walk_files;
use pragma_engine::lock::{LOCK_FILE, Lock};
use pragma_engine::overrides::{OVERRIDES_FILE, Overrides, apply_to_audit};
use pragma_engine::repo_index::package_roots;
use pragma_engine::rules::CompiledLock;
use pragma_engine::writes::write_atomic;

use crate::json::AuditOutput;
use crate::output::print_json;
use crate::repo_here;

/// `area` and `changed_from` narrow the audit; the command line never passes both.
pub fn run(cwd: &Path, area: Option<&str>, changed_from: Option<&str>, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let dir = repo
        .effective_pragma_dir()
        .context("this repo has no .pragma/; run pragma init")?;
    let lock = Lock::load(&dir.join(LOCK_FILE))?;
    let compiled = CompiledLock::compile(&lock)?;
    let changed_files = changed_from
        .map(|base| audit::changed_files(&repo.toplevel, &compiled, base))
        .transpose()?;
    let mut report = match &changed_files {
        Some(files) => audit::run_files(&repo.toplevel, &compiled, files)?,
        None => audit::run(&repo.toplevel, &compiled, area)?,
    };
    let exceptions = Overrides::load(&dir.join(OVERRIDES_FILE))?.exceptions()?;
    let para_of = |clause_id: &str| {
        compiled
            .clause(clause_id)
            .map(|clause| clause.anchor.para_hash.clone())
            .unwrap_or_default()
    };
    let looked_at = |file: &str| match (&changed_files, area) {
        (Some(files), _) => files.iter().any(|changed| changed == file),
        (None, Some(area)) => audit::in_area(file, area),
        (None, None) => true,
    };
    let stale = apply_to_audit(&mut report, &exceptions, looked_at, para_of);
    let output = AuditOutput {
        report,
        stale_exceptions: stale,
        package_roots: package_roots(&walk_files(&repo.toplevel)),
        changed_files,
    };
    let (report, stale) = (&output.report, &output.stale_exceptions);
    if area.is_none() && changed_from.is_none() {
        let state = repo.state_dir();
        write_atomic(
            &state.join("audit.json"),
            serde_json::to_string_pretty(&output)?.as_bytes(),
        )?;
        audit::append_history(&state, report)?;
    }

    if json {
        return print_json(&output);
    }
    println!(
        "Checked {} files against {} rules.",
        report.files_checked,
        lock.clauses.len()
    );
    if let Some(base) = changed_from {
        println!("Not saved: this result covers only the files changed since {base}.");
    } else if area.is_some() {
        println!("Not saved: this result covers only part of the repo.");
    }
    if report.findings.is_empty() {
        println!("No violations.");
    }
    for clause in compiled.clauses() {
        let count = report.counts.get(&clause.id).copied().unwrap_or(0);
        if count == 0 {
            continue;
        }
        println!(
            "\n{count} x {} ({}:{})",
            clause.text, clause.anchor.path, clause.anchor.lines[0]
        );
        for found in report
            .findings
            .iter()
            .filter(|found| found.finding.clause_id == clause.id)
        {
            let finding = &found.finding;
            println!(
                "    {}  {}:{}  {}",
                found.id, finding.file, finding.line, finding.matched
            );
        }
    }
    if !report.findings.is_empty() {
        println!("\nMark one as not a violation with:");
        println!("  pragma exception add <id> --reason deliberate|check-is-wrong");
    }
    if !stale.is_empty() {
        println!(
            "\n{} exceptions no longer match any code; the code changed or went away.",
            stale.len()
        );
    }
    Ok(())
}
