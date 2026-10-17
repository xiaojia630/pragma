//! `pragma events`: what the edit check decided in this checkout, each event joined with its
//! clause from the effective lock. The app polls it for an open task, so a repo without
//! `.pragma/` gives an empty list rather than an error, and only the end of the log is read.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use pragma_engine::events::{self, Event, EventKind};
use pragma_engine::lock::{LOCK_FILE, Lock};

use crate::json::{ClauseInfo, EventInfo, EventsOutput};
use crate::lock_cmd::clause_info;
use crate::output::print_json;
use crate::repo_here;

/// Events shown when no limit is given.
pub const DEFAULT_LIMIT: usize = 200;
/// The end of the log read on each run: about a thousand events, so a poll stays cheap
/// however long the log grows.
const TAIL_BYTES: u64 = 256 * 1024;
/// Clause text shown per line in the human output, in characters.
const TEXT_CHARS: usize = 80;

pub fn run(cwd: &Path, limit: usize, since_ms: Option<u64>, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let events = match repo.effective_pragma_dir() {
        None => Vec::new(),
        Some(dir) => {
            // The clauses only add detail: events still list when the lock does not load.
            let clauses: HashMap<String, ClauseInfo> = Lock::load(&dir.join(LOCK_FILE))
                .map(|lock| lock.clauses)
                .unwrap_or_default()
                .into_iter()
                .map(|clause| (clause.id.clone(), clause_info(clause)))
                .collect();
            recent(
                events::read_tail(&repo.state_dir(), TAIL_BYTES),
                limit,
                since_ms,
            )
            .into_iter()
            .map(|event| EventInfo {
                clause: event
                    .clause_id
                    .as_ref()
                    .and_then(|id| clauses.get(id))
                    .cloned(),
                event,
            })
            .collect()
        }
    };
    let output = EventsOutput { events };
    if json {
        return print_json(&output);
    }
    if output.events.is_empty() {
        println!("No edit checks recorded in this checkout.");
    }
    for info in &output.events {
        println!("{}", line(info));
    }
    Ok(())
}

/// The last `limit` events after `since_ms`, oldest first. Hooks running side by side can
/// append slightly out of order, so the log is sorted by time.
fn recent(mut events: Vec<Event>, limit: usize, since_ms: Option<u64>) -> Vec<Event> {
    events.retain(|event| since_ms.is_none_or(|since| event.at_ms > since));
    events.sort_by_key(|event| event.at_ms);
    let start = events.len().saturating_sub(limit);
    events.split_off(start)
}

/// `HH:MM:SS  kind  file:line  what`: the clause's text, or the reason an edit went unchecked.
fn line(info: &EventInfo) -> String {
    let event = &info.event;
    let kind = match event.kind {
        EventKind::Blocked => "blocked",
        EventKind::Escalated => "escalated",
        EventKind::Unchecked => "unchecked",
        EventKind::Cleared => "cleared",
    };
    let place = match event.line {
        Some(line) => format!("{}:{line}", event.file),
        None => event.file.clone(),
    };
    let what = match (&info.clause, &event.clause_id, &event.reason) {
        (Some(clause), ..) => cut(&clause.text),
        (None, Some(id), _) => id.clone(),
        (None, None, Some(reason)) => cut(reason),
        (None, None, None) => String::new(),
    };
    format!("{}  {kind:<9}  {place}  {what}", clock(event.at_ms))
        .trim_end()
        .to_string()
}

fn cut(text: &str) -> String {
    match text.char_indices().nth(TEXT_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// The time of day, local where the platform says what local is, else UTC.
fn clock(at_ms: u64) -> String {
    let seconds = (at_ms / 1000) as i64;
    #[cfg(unix)]
    {
        let time = seconds as libc::time_t;
        // SAFETY: localtime_r, the thread-safe form, writes only into the zeroed struct passed
        // in; a null return means it could not convert.
        unsafe {
            let mut tm: libc::tm = std::mem::zeroed();
            if !libc::localtime_r(&time, &mut tm).is_null() {
                return format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec);
            }
        }
    }
    let day = seconds.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, day / 60 % 60, day % 60)
}
