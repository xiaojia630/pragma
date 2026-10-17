//! `pragma exception add`: records that a finding is not a violation, in `.pragma/overrides.md`
//! in the root checkout, so it stays suppressed in every worktree and on every machine.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use pragma_engine::audit::AuditFinding;
use pragma_engine::drift::DriftFinding;
use pragma_engine::events::now_ms;
use pragma_engine::lock::{LOCK_FILE, Lock};
use pragma_engine::overrides::{Exception, OVERRIDES_FILE, Overrides, Reason};
use pragma_engine::writes::{record_write, write_atomic};
use serde::Deserialize;

use crate::json::ExceptionOutput;
use crate::output::print_json;
use crate::repo_here;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReasonArg {
    /// The rule is right, and this code is sanctioned anyway.
    Deliberate,
    /// The check fired on code that does not break the rule.
    CheckIsWrong,
    /// A drift finding that is meant (an example path, say).
    Intended,
}

#[derive(Deserialize)]
struct Saved<T> {
    findings: Vec<T>,
}

fn saved<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str::<Saved<T>>(&text)
        .with_context(|| format!("reading {}", path.display()))?
        .findings)
}

pub fn add(
    cwd: &Path,
    id: &str,
    reason: ReasonArg,
    note: Option<String>,
    json: bool,
) -> Result<()> {
    let repo = repo_here(cwd)?;
    let effective = repo
        .effective_pragma_dir()
        .context("this repo has no .pragma/; run pragma init first")?;
    let target = repo.writable_pragma_dir();
    if !target.is_dir() {
        bail!(
            "pragma writes .pragma/ only in the root checkout ({}), which has none; commit .pragma/ \
             and check it out there first",
            target.display()
        );
    }
    let state = repo.state_dir();
    let audit: Vec<AuditFinding> = saved(&state.join("audit.json"))?;
    let drift: Vec<DriftFinding> = saved(&state.join("drift.json"))?;
    let exception = if let Some(found) = audit.into_iter().find(|found| found.id == id) {
        let reason = match reason {
            ReasonArg::Deliberate => Reason::Deliberate,
            ReasonArg::CheckIsWrong => Reason::CheckIsWrong,
            ReasonArg::Intended => {
                bail!("--reason intended is for drift; use deliberate or check-is-wrong")
            }
        };
        let lock = Lock::load(&effective.join(LOCK_FILE))?;
        let clause = lock
            .clauses
            .iter()
            .find(|clause| clause.id == found.finding.clause_id)
            .context("that finding's rule is no longer in the lock; run pragma audit again")?;
        Exception::Violation {
            clause: clause.id.clone(),
            para_hash: clause.anchor.para_hash.clone(),
            file: found.finding.file,
            matched: found.finding.matched,
            reason,
            note,
        }
    } else if let Some(found) = drift.into_iter().find(|found| found.id == id) {
        if !matches!(reason, ReasonArg::Intended) {
            bail!("drift findings take --reason intended");
        }
        Exception::Drift {
            doc: found.doc,
            para_hash: found.para_hash,
            reference: found.reference,
            note,
        }
    } else {
        bail!("no finding {id} in the last pragma audit or pragma drift; run one first");
    };

    let path = target.join(OVERRIDES_FILE);
    let mut overrides = Overrides::load(&path)?;
    if !overrides.add_exception(exception.clone())? {
        if json {
            return print_json(&ExceptionOutput {
                recorded: false,
                exception,
            });
        }
        println!("{id} is already an exception.");
        return Ok(());
    }
    let text = overrides.to_text()?;
    write_atomic(&path, text.as_bytes())?;
    let shared = repo.shared_state_dir();
    record_write(&shared, ".pragma/overrides.md", text.as_bytes())?;
    let mut feedback = OpenOptions::new()
        .create(true)
        .append(true)
        .open(shared.join("feedback.jsonl"))?;
    writeln!(
        feedback,
        "{}",
        serde_json::json!({ "at_ms": now_ms(), "exception": exception })
    )?;
    if json {
        return print_json(&ExceptionOutput {
            recorded: true,
            exception,
        });
    }
    println!("Recorded {id} in .pragma/overrides.md. Commit it so it holds everywhere.");
    Ok(())
}
