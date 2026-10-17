//! `pragma status`: what pragma is doing in this repo, for the app and for people.

use std::path::Path;

use anyhow::Result;
use pragma_engine::env::ProcessEnv;
use pragma_engine::extract::status::{self as extraction, ExtractionState};
use pragma_engine::gate::is_paused;
use pragma_engine::lock::{LOCK_FILE, LOCK_FORMAT, Lock};
use pragma_engine::lock_update;

use crate::hooks_cmd::hooks_state;
use crate::json::Status;
use crate::lock_cmd::index_for_lock;
use crate::output::print_json;
use crate::repo_here;

pub fn run(cwd: &Path, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let pragma_dir = repo.effective_pragma_dir();
    let (lock, lock_error) = match &pragma_dir {
        None => (None, None),
        Some(dir) => match Lock::load(&dir.join(LOCK_FILE)) {
            Ok(lock) => (Some(lock), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        },
    };
    let is_root_checkout = repo.root_checkout.as_ref() == Some(&repo.toplevel);
    let update = lock
        .as_ref()
        .filter(|_| is_root_checkout)
        .and_then(|lock| Some(lock_update::update(lock, &index_for_lock(&repo)?)));
    let lock_pending = update.as_ref().is_some_and(|update| update.changes_rules());
    let lock_unsettled = update.as_ref().map_or(0, |update| update.settled);
    let status = Status {
        version: env!("CARGO_PKG_VERSION").to_string(),
        lock_format: LOCK_FORMAT,
        enabled: pragma_dir.is_some(),
        is_root_checkout,
        pragma_dir,
        paused: is_paused(&repo),
        hooks: hooks_state(None, &ProcessEnv),
        clauses: lock.map(|lock| lock.clauses.len()),
        lock_error,
        extraction: extraction::status(&repo),
        lock_pending,
        lock_unsettled,
    };
    if json {
        return print_json(&status);
    }
    print_human(&status);
    Ok(())
}

fn print_human(status: &Status) {
    println!(
        "pragma {} (lock format {})",
        status.version, status.lock_format
    );
    match &status.pragma_dir {
        None => println!("pragma is not turned on for this repo. Run pragma init."),
        Some(dir) => {
            println!("Settings: {}", dir.display());
            if !status.is_root_checkout {
                println!("This is a worktree; pragma changes .pragma/ only in the root checkout.");
            }
            if let Some(error) = &status.lock_error {
                println!("The lock cannot be read: {error}");
            } else if let Some(clauses) = status.clauses {
                println!("The lock holds {clauses} rules.");
            }
        }
    }
    let extraction = &status.extraction;
    let counts = extraction
        .clauses
        .map(|c| {
            format!(
                ": {} blocking, {} judged, {} context",
                c.blocking, c.judged, c.context
            )
        })
        .unwrap_or_default();
    match extraction.state {
        ExtractionState::Never => println!("The docs have not been read yet. Run pragma extract."),
        ExtractionState::Running => match &extraction.progress {
            Some(progress) => println!(
                "Reading the docs: {} of {} paragraphs.",
                progress.done, progress.total
            ),
            None => println!("Reading the docs."),
        },
        ExtractionState::Done => println!("Clauses from the docs{counts}."),
        ExtractionState::Paused => println!(
            "Reading the docs is paused: {}",
            extraction
                .reason
                .as_ref()
                .map(|reason| reason.message.as_str())
                .unwrap_or("the model was unavailable")
        ),
    }
    if status.lock_pending {
        println!("The lock is behind the docs. Run pragma lock.");
    } else if status.lock_unsettled > 0 {
        println!(
            "{} doc paragraphs are new or edited with no rule change; the next pragma lock records them.",
            status.lock_unsettled
        );
    }
    if status.paused {
        println!("pragma is paused for this repo on this machine.");
    }
    if status.hooks.installed {
        println!("Claude Code hooks: installed.");
    } else {
        println!("Claude Code hooks: not installed. Run pragma hooks install.");
    }
}
