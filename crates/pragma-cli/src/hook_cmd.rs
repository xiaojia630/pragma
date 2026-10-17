//! `pragma hook <event>`: Claude Code hook entry points. A hook must never get in the agent's
//! way by accident, so every failure lets the edit through (exit 0); only a real block exits 2.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use clap::Subcommand;
use pragma_engine::edit::{HookInput, proposed_edit};
use pragma_engine::env::{Env, ProcessEnv};
use pragma_engine::events::{self, Event, EventKind, SessionBlocks};
use pragma_engine::gate::{Gate, gate};
use pragma_engine::introduced::introduced_by_edit;
use pragma_engine::lock::{LOCK_FILE, Lock};
use pragma_engine::message::{Violation, block_message};
use pragma_engine::paths::repo_relative;
use pragma_engine::rules::{CompiledLock, Mode};

/// The hard cap on an edit check. Past it, the edit goes through and is recorded as unchecked.
const DEADLINE: Duration = Duration::from_millis(50);
/// Test-only: overrides the deadline, in milliseconds. Never set by pragma itself.
const DEADLINE_ENV: &str = "PRAGMA_HOOK_DEADLINE_MS";
/// What the watchdog can record if the deadline passes: state dir, session and file.
/// Set once by whichever of the watchdog and the main path decides the exit first.
static DECIDED: Mutex<bool> = Mutex::new(false);
static CONTEXT: Mutex<Option<(PathBuf, Option<String>, String)>> = Mutex::new(None);
/// Files bigger than this skip content rules.
const MAX_CONTENT_BYTES: usize = 512 * 1024;
/// Edits a session may have blocked by the same clause on the same file; the next one is let
/// through instead, but only when no other clause still blocks that edit. The count is per
/// session, not per turn, until turn checks exist: nothing but an escalation starts it over.
const ESCALATE_AT: u32 = 3;

#[derive(Subcommand)]
pub enum HookEvent {
    /// Claude Code's PreToolUse hook. Reads the hook JSON on stdin.
    PreToolUse,
}

pub enum Outcome {
    Allow,
    Block(String),
}

pub fn run(event: HookEvent) -> ExitCode {
    std::panic::set_hook(Box::new(|_| {}));
    let mut stdin = String::new();
    if std::io::stdin().read_to_string(&mut stdin).is_err() {
        return ExitCode::SUCCESS;
    }
    // The budget measures the check, so the clock starts after stdin is read.
    let started = Instant::now();
    start_watchdog(deadline(&ProcessEnv));
    let outcome = std::panic::catch_unwind(|| match event {
        HookEvent::PreToolUse => pre_tool_use(&stdin, &ProcessEnv, started),
    });
    // Decide exactly once: after this the watchdog can no longer exit, and if it already
    // decided, its exit(0) is in progress and this thread waits for it.
    let mut decided = DECIDED.lock().unwrap_or_else(|e| e.into_inner());
    if *decided {
        std::process::exit(0);
    }
    *decided = true;
    match outcome {
        Ok(Outcome::Block(message)) => {
            eprint!("{message}");
            std::process::exit(2)
        }
        Ok(Outcome::Allow) | Err(_) => ExitCode::SUCCESS,
    }
}

fn deadline(env: &impl Env) -> Duration {
    env.var(DEADLINE_ENV)
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(DEADLINE)
}

/// The hard cap: past the deadline the edit goes through, recorded as unchecked when known.
fn start_watchdog(deadline: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(deadline);
        let mut decided = DECIDED.lock().unwrap_or_else(|e| e.into_inner());
        if *decided {
            return;
        }
        *decided = true;
        if let Ok(context) = CONTEXT.lock()
            && let Some((state, session, file)) = context.as_ref()
        {
            let event = Event {
                reason: Some("deadline".to_string()),
                ..Event::new(EventKind::Unchecked, session.clone(), file.clone())
            };
            let _ = events::append(state, &[event]);
        }
        std::process::exit(0);
    });
}

pub fn pre_tool_use(stdin: &str, env: &impl Env, started: Instant) -> Outcome {
    check_edit(stdin, env, started, events::now_ms())
}

/// The edit check. Every event it records carries `at_ms`: one check is one moment, so its
/// events never sort out of the order they were written in.
fn check_edit(stdin: &str, env: &impl Env, started: Instant, at_ms: u64) -> Outcome {
    let Ok(input) = serde_json::from_str::<HookInput>(stdin) else {
        return Outcome::Allow;
    };
    let cwd = input
        .cwd
        .clone()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let Some(cwd) = cwd else {
        return Outcome::Allow;
    };
    let Gate::On { repo, pragma_dir } = gate(&cwd, env) else {
        return Outcome::Allow;
    };
    let read = |path: &std::path::Path| std::fs::read_to_string(path).ok();
    let Some(edit) = proposed_edit(&input, &cwd, read) else {
        return Outcome::Allow;
    };
    let Some(rel) = repo_relative(&repo.toplevel, &edit.file_path) else {
        return Outcome::Allow;
    };
    let state = repo.state_dir();
    if let Ok(mut context) = CONTEXT.lock() {
        *context = Some((state.clone(), input.session_id.clone(), rel.clone()));
    }
    let new_event = |kind: EventKind| Event {
        at_ms,
        ..Event::new(kind, input.session_id.clone(), rel.clone())
    };
    let unchecked = |reason: &str| {
        let event = Event {
            reason: Some(reason.to_string()),
            ..new_event(EventKind::Unchecked)
        };
        let _ = events::append(&state, &[event]);
        Outcome::Allow
    };

    let lock_path = pragma_dir.join(LOCK_FILE);
    if !lock_path.exists() {
        return Outcome::Allow;
    }
    let lock = match Lock::load(&lock_path).and_then(|lock| CompiledLock::compile(&lock)) {
        Ok(lock) => lock,
        Err(error) => return unchecked(&format!("{error:#}")),
    };

    let mut found = lock.path_findings(&rel, edit.op, Mode::Edit);
    // Content rules skip a file over the size limit, so its open blocks stay open.
    let checked_in_full = edit.after.len() <= MAX_CONTENT_BYTES;
    if checked_in_full {
        found.extend(introduced_by_edit(&lock, &rel, &edit.before, &edit.after));
        if started.elapsed() > deadline(env) {
            return unchecked("over the 50 ms deadline");
        }
    } else if found.is_empty() {
        return unchecked("file over 512 KB");
    }

    // Without a session id there is nothing to count against, so such edits always block. With
    // one, the session's blocks are read: for an allowed edit in a session never blocked, that
    // is one failed open.
    let session = input.session_id.clone().filter(|id| !id.is_empty());
    let mut blocks = session
        .as_ref()
        .map(|session| SessionBlocks::load(&state, session));
    // An edit counts once per clause, however many findings of that clause it has. A clause is
    // due for escalation once its count passes the limit, but the edit goes through only when
    // every clause it breaks is due. While another clause still blocks it, the due clause is
    // recorded blocked like the rest: its count stays and its block stays open, and it
    // escalates on an edit nothing else blocks.
    let mut clauses: Vec<&str> = Vec::new();
    for finding in &found {
        if !clauses.contains(&finding.clause_id.as_str()) {
            clauses.push(&finding.clause_id);
        }
    }
    let mut due: Vec<&str> = Vec::new();
    if let Some(blocks) = blocks.as_mut() {
        for clause_id in &clauses {
            if blocks.block(clause_id, &rel) > ESCALATE_AT {
                due.push(clause_id);
            }
        }
    }
    let goes_through = !clauses.is_empty() && due.len() == clauses.len();
    if let Some(blocks) = blocks.as_mut()
        && goes_through
    {
        for clause_id in &due {
            blocks.escalate(clause_id, &rel);
        }
    }
    let mut decided = Vec::new();
    let mut blocking = Vec::new();
    for finding in &found {
        let kind = if goes_through {
            EventKind::Escalated
        } else {
            blocking.push(finding);
            EventKind::Blocked
        };
        decided.push(new_event(kind).with_finding(finding));
    }
    let cleared = match &mut blocks {
        Some(blocks) if checked_in_full => blocks.clear(&rel, |clause_id| {
            found.iter().any(|f| f.clause_id == clause_id)
        }),
        _ => Vec::new(),
    };
    let took_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
    if let (Some(session), Some(blocks)) = (&session, &blocks)
        && (!found.is_empty() || !cleared.is_empty())
    {
        let _ = blocks.save(&state, session);
    }
    let recorded: Vec<Event> = cleared
        .into_iter()
        .map(|clause_id| Event {
            clause_id: Some(clause_id),
            ..new_event(EventKind::Cleared)
        })
        .chain(decided)
        .map(|event| Event {
            took_ms: Some(took_ms),
            ..event
        })
        .collect();
    let _ = events::append(&state, &recorded);
    if found.is_empty() {
        return Outcome::Allow;
    }
    let lines: Vec<&str> = edit.after.lines().collect();
    let violations: Vec<Violation> = blocking
        .into_iter()
        .filter_map(|finding| {
            Some(Violation {
                clause: lock.clause(&finding.clause_id)?,
                finding,
                line_text: finding
                    .line
                    .checked_sub(1)
                    .and_then(|index| lines.get(index as usize).copied())
                    .unwrap_or(""),
            })
        })
        .collect();
    if violations.is_empty() {
        return Outcome::Allow;
    }
    Outcome::Block(block_message(&violations))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::process::Command;

    use super::*;

    fn clause(id: &str, pattern: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "anchor": { "path": "AGENTS.md", "heading_path": [], "para_hash": id, "lines": [1, 1], "quote": "" },
            "text": format!("Never write {pattern}."),
            "scope": ["**/*.ts"],
            "check": { "kind": "library", "rule": "forbidden-pattern", "pattern": pattern,
                       "examples": { "bad": pattern, "good": "ok" } },
            "fix": "Do not."
        })
    }

    /// A Write hook input creating `src/a.ts` with `content`.
    fn write_input(root: &std::path::Path, content: &str) -> String {
        serde_json::json!({
            "session_id": "s1",
            "cwd": root,
            "tool_name": "Write",
            "tool_input": { "file_path": root.join("src/a.ts"), "content": content }
        })
        .to_string()
    }

    #[test]
    fn every_event_of_one_check_carries_the_checks_own_timestamp() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(init.success());
        fs::create_dir_all(root.join(".pragma")).unwrap();
        fs::write(
            root.join(".pragma/pragma.lock"),
            serde_json::json!({
                "format": 1,
                "clauses": [clause("no-eval", "eval($$$A)"), clause("no-with", "with($$$A)")]
            })
            .to_string(),
        )
        .unwrap();
        let env: HashMap<String, String> = HashMap::new();
        let check = |content: &str, at_ms: u64| {
            check_edit(&write_input(&root, content), &env, Instant::now(), at_ms)
        };

        // Two clauses broken: two blocked events, and the second check clears one of them.
        assert!(matches!(
            check("eval(a);\nwith(b);\n", 4242),
            Outcome::Block(_)
        ));
        assert!(matches!(check("with(b);\n", 4343), Outcome::Block(_)));
        let recorded = events::read_tail(&root.join(".git/pragma"), u64::MAX);
        let summary: Vec<(EventKind, Option<&str>, u64)> = recorded
            .iter()
            .map(|event| (event.kind, event.clause_id.as_deref(), event.at_ms))
            .collect();
        assert_eq!(
            summary,
            vec![
                (EventKind::Blocked, Some("no-eval"), 4242),
                (EventKind::Blocked, Some("no-with"), 4242),
                (EventKind::Cleared, Some("no-eval"), 4343),
                (EventKind::Blocked, Some("no-with"), 4343),
            ]
        );
    }
}
