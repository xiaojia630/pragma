//! `pragma drift`: the docs checked against the repo. Works in any git repo, enabled or not.

use std::path::Path;

use anyhow::Result;
use pragma_engine::config::Config;
use pragma_engine::docs::discover;
use pragma_engine::drift::{self, Confidence, DriftFinding, DriftKind, DriftReport};
use pragma_engine::overrides::{Exception, OVERRIDES_FILE, Overrides, apply_to_drift};
use pragma_engine::repo::Repo;
use pragma_engine::writes::write_atomic;

use crate::json::{DriftApplyOutput, DriftOutput};
use crate::output::print_json;
use crate::repo_here;

/// Checks the docs and saves the result as this worktree's `drift.json`.
fn check_and_save(repo: &Repo) -> Result<DriftOutput> {
    let root = &repo.toplevel;
    let pragma_dir = repo.effective_pragma_dir();
    let config = Config::load(pragma_dir.as_deref())?;
    let docs = discover(root, &config.docs)?;
    let mut report = drift::run(root, &docs)?;
    let exceptions = match &pragma_dir {
        Some(dir) => Overrides::load(&dir.join(OVERRIDES_FILE))?.exceptions()?,
        None => Vec::new(),
    };
    let stale = apply_to_drift(&mut report, &exceptions);
    let output = DriftOutput {
        report,
        stale_exceptions: stale,
    };
    let saved = serde_json::to_string_pretty(&output)?;
    write_atomic(&repo.state_dir().join("drift.json"), saved.as_bytes())?;
    Ok(output)
}

pub fn run(cwd: &Path, json: bool, diff: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let output = check_and_save(&repo)?;
    if diff {
        print!(
            "{}",
            drift::fix_diff(&repo.toplevel, &output.report.findings)?
        );
        return Ok(());
    }
    if json {
        return print_json(&output);
    }
    print_human(&output.report, &output.stale_exceptions);
    Ok(())
}

/// Applies the suggested fix of each named finding that the fresh check still reports, then
/// saves the check again. The doc is rewritten only here, on request.
pub fn apply(cwd: &Path, ids: Vec<String>, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let report = check_and_save(&repo)?.report;
    let wanted: Vec<DriftFinding> = report
        .findings
        .iter()
        .filter(|finding| ids.contains(&finding.id))
        .cloned()
        .collect();
    let mut applied = Vec::new();
    for fix in drift::apply_fixes(&repo.toplevel, &wanted)? {
        write_atomic(&repo.toplevel.join(&fix.doc), fix.new.as_bytes())?;
        applied.extend(fix.applied);
    }
    let mut skipped: Vec<String> = Vec::new();
    for id in ids {
        if !applied.contains(&id) && !skipped.contains(&id) {
            skipped.push(id);
        }
    }
    applied.sort_by_key(|id| report.findings.iter().position(|f| &f.id == id));
    check_and_save(&repo)?;
    let result = DriftApplyOutput { applied, skipped };
    if json {
        return print_json(&result);
    }
    println!("Applied {} fixes.", result.applied.len());
    if !result.skipped.is_empty() {
        println!(
            "Skipped {}: not found, no suggested fix, or the doc changed: {}",
            result.skipped.len(),
            result.skipped.join(", ")
        );
    }
    Ok(())
}

fn print_human(report: &DriftReport, stale: &[Exception]) {
    println!(
        "Checked {} docs and {} references.",
        report.docs_checked, report.references_checked
    );
    // A path that may be a URL is set apart; the rest is split by how sure the check is.
    let (urls, rest): (Vec<&DriftFinding>, Vec<&DriftFinding>) = report
        .findings
        .iter()
        .partition(|finding| finding.kind == DriftKind::MaybeUrl);
    let (sure, unsure): (Vec<&DriftFinding>, Vec<&DriftFinding>) = rest
        .into_iter()
        .partition(|finding| finding.confidence == Confidence::High);
    if report.findings.is_empty() {
        println!("Every reference checked out.");
    }
    if !sure.is_empty() {
        println!(
            "\n{} references point at things this repo does not have:",
            sure.len()
        );
        print_findings(&sure);
    }
    if !urls.is_empty() {
        println!(
            "\n{} references start with / and match no file (may be URLs, not files):",
            urls.len()
        );
        print_findings(&urls);
    }
    if !unsure.is_empty() {
        println!(
            "\n{} code names not found anywhere (less certain):",
            unsure.len()
        );
        print_findings(&unsure);
    }
    let fixable = report
        .findings
        .iter()
        .filter(|finding| finding.suggestion.is_some())
        .count();
    if fixable > 0 {
        println!("\nApply the {fixable} suggested fixes with: pragma drift --diff | git apply");
    }
    if !report.findings.is_empty() {
        println!("Mark one as intended with: pragma exception add <id> --reason intended");
    }
    if !stale.is_empty() {
        println!(
            "\n{} drift exceptions no longer match anything.",
            stale.len()
        );
    }
}

fn print_findings(findings: &[&DriftFinding]) {
    let mut current_doc = "";
    for finding in findings {
        if finding.doc != current_doc {
            current_doc = &finding.doc;
            println!("  {current_doc}");
        }
        let what = match finding.kind {
            DriftKind::MissingPath => "missing",
            DriftKind::UnknownScript => "no such script",
            DriftKind::UnknownName => "not found in code",
            DriftKind::MaybeUrl => "no such file",
        };
        let suggestion = finding
            .suggestion
            .as_deref()
            .map(|new| format!(" -> {new}"))
            .unwrap_or_default();
        println!(
            "    {:>5}  {}  {}  {what}{suggestion}",
            finding.line, finding.id, finding.reference
        );
    }
}
