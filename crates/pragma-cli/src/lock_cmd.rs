//! `pragma lock`: updates the lock from the index. `pragma lock check`: validates the effective
//! lock. `pragma lock show`: lists its clauses.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use pragma_engine::index::Index;
use pragma_engine::lock::{Check, LOCK_FILE, LibraryRule, Lock, LockedClause};
use pragma_engine::lock_update;
use pragma_engine::overrides::{OVERRIDES_FILE, Overrides};
use pragma_engine::repo::Repo;
use pragma_engine::rules::validate;
use pragma_engine::writes::{record_write, write_atomic};

use crate::json::{ClauseInfo, LockCheckOutput, LockShowOutput, LockUpdateOutput};
use crate::output::print_json;
use crate::repo_here;

/// Applies the §5.2 update rule (`lock_update`) from this checkout's index to the root
/// checkout's lock. Writes nothing when nothing changed, so the file keeps its bytes.
pub fn update(cwd: &Path, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let Some(root) = repo
        .root_checkout
        .as_ref()
        .filter(|root| *root == &repo.toplevel)
    else {
        bail!("pragma updates the lock only in the root checkout; run this there");
    };
    let dir = root.join(".pragma");
    if !dir.is_dir() {
        bail!("this repo has no .pragma/; run pragma init first");
    }
    let index = index_for_lock(&repo)
        .context("this checkout has no clause index; run pragma extract first")?;
    let path = dir.join(LOCK_FILE);
    let exists = path.exists();
    let lock = if exists {
        Lock::load(&path)?
    } else {
        Lock::empty()
    };
    let update = lock_update::update(&lock, &index);
    if !update.is_empty() || !exists {
        // The hook reads the lock on every edit: a lock with a problem is never written.
        let problems = validate(&update.lock);
        if !problems.is_empty() {
            bail!(
                "the updated lock would have {} problem(s), so .pragma/{LOCK_FILE} was not \
                 changed:\n{}",
                problems.len(),
                problems.join("\n")
            );
        }
        let text = update.lock.to_json();
        write_atomic(&path, text.as_bytes())?;
        record_write(
            &repo.shared_state_dir(),
            &format!(".pragma/{LOCK_FILE}"),
            text.as_bytes(),
        )?;
    }
    let output = LockUpdateOutput {
        added: update.added,
        removed: update.removed,
        kept: update.kept,
        changed: update.changed,
        settled: update.settled,
    };
    let only_paragraphs = update.paragraphs_changed;
    if json {
        return print_json(&output);
    }
    if output.added.is_empty() && output.removed.is_empty() && output.changed.is_empty() {
        if only_paragraphs {
            println!(
                "Updated .pragma/pragma.lock: no blocking clauses changed; {} paragraphs settled.",
                output.settled
            );
            return Ok(());
        }
        println!("pragma.lock is up to date: {} clauses kept.", output.kept);
        return Ok(());
    }
    println!(
        "Updated .pragma/pragma.lock: {} added, {} changed, {} removed, {} kept.",
        output.added.len(),
        output.changed.len(),
        output.removed.len(),
        output.kept
    );
    println!("Review the diff, then commit it with pragma commit.");
    Ok(())
}

/// This checkout's index as the lock sees it: only docs git tracks, so an untracked draft
/// never reaches the committed lock (its clauses still reach the index for judging). None when
/// there is no index. When git cannot list the tracked docs, every doc counts.
pub fn index_for_lock(repo: &Repo) -> Option<Index> {
    let mut index = Index::load(&repo.state_dir())?;
    if let Some(tracked) = tracked_docs(&repo.toplevel, &index) {
        index.retain_docs(|doc| tracked.contains(doc));
    }
    Some(index)
}

/// The index's docs that git tracks (staged ones included), or None when git cannot tell.
fn tracked_docs(root: &Path, index: &Index) -> Option<HashSet<String>> {
    let docs: BTreeSet<&str> = index
        .paragraphs
        .iter()
        .map(|para| para.path.as_str())
        .chain(index.failed.iter().map(|failed| failed.path.as_str()))
        .collect();
    if docs.is_empty() {
        return Some(HashSet::new());
    }
    let output = Command::new("git")
        .args(["--literal-pathspecs", "ls-files", "-z", "--"])
        .args(&docs)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect(),
    )
}

pub fn check(cwd: &Path, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let dir = repo
        .effective_pragma_dir()
        .context("this repo has no .pragma/; run pragma init")?;
    let lock = Lock::load(&dir.join(LOCK_FILE))?;
    let problems = validate(&lock);
    if json {
        print_json(&LockCheckOutput {
            clauses: lock.clauses.len(),
            problems: problems.clone(),
        })?;
    } else if problems.is_empty() {
        println!("pragma.lock is sound: {} clauses.", lock.clauses.len());
    } else {
        for problem in &problems {
            println!("{problem}");
        }
    }
    if !problems.is_empty() {
        bail!("pragma.lock has {} problem(s)", problems.len());
    }
    Ok(())
}

pub fn show(cwd: &Path, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let dir = repo
        .effective_pragma_dir()
        .context("this repo has no .pragma/; run pragma init")?;
    let lock = Lock::load(&dir.join(LOCK_FILE))?;
    let exceptions = Overrides::load(&dir.join(OVERRIDES_FILE))?.exceptions()?;
    let output = LockShowOutput {
        clauses: lock.clauses.into_iter().map(clause_info).collect(),
        exceptions,
    };
    if json {
        return print_json(&output);
    }
    if output.clauses.is_empty() {
        println!("pragma.lock has no clauses.");
    }
    for clause in &output.clauses {
        println!(
            "{}  {} ({}:{})",
            clause.id, clause.text, clause.source.path, clause.source.lines[0]
        );
        println!("    fix: {}", clause.fix);
    }
    if !output.exceptions.is_empty() {
        println!(
            "\n{} exceptions in .pragma/overrides.md.",
            output.exceptions.len()
        );
    }
    Ok(())
}

pub(crate) fn clause_info(clause: LockedClause) -> ClauseInfo {
    let rule = match &clause.check {
        Check::Library(LibraryRule::ProtectedPaths) => "protected-paths",
        Check::Library(LibraryRule::ImportBoundary { .. }) => "import-boundary",
        Check::Library(LibraryRule::ForbiddenPattern { .. }) => "forbidden-pattern",
        Check::Library(LibraryRule::FileNaming { .. }) => "file-naming",
        Check::Pattern { .. } => "pattern",
    };
    ClauseInfo {
        id: clause.id,
        text: clause.text,
        fix: clause.fix,
        rule: rule.to_string(),
        scope: clause.scope,
        source: clause.anchor,
    }
}
