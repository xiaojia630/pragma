//! What the edit check records in `<git dir>/pragma/events.jsonl`, and each session's blocks:
//! the counts behind escalation and the blocks its next edit has not yet cleared.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::hashing::short_hash;
use crate::rules::Finding;
use crate::writes::write_atomic;

/// The edit check's log, in a checkout's own `<git dir>/pragma/`.
pub const EVENTS_FILE: &str = "events.jsonl";
/// Matched text kept in an event, in characters: enough to recognize the code, while a log the
/// app reads often stays small.
pub const MATCHED_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    Blocked,
    Unchecked,
    Escalated,
    /// The session's next edit to the file no longer broke a clause that had blocked it.
    Cleared,
}

/// One line of `events.jsonl`. Fields added later are optional, so older lines still parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub kind: EventKind,
    /// The agent's session id, when the hook input had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Repo-relative.
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clause_id: Option<String>,
    /// Why an edit went unchecked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The finding's line, 1-based; absent for a finding about the file itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The finding's matched text, cut to `MATCHED_CHARS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    /// How long the check took, from reading the hook input to its decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub took_ms: Option<u32>,
}

impl Event {
    pub fn new(kind: EventKind, session: Option<String>, file: String) -> Event {
        Event {
            at_ms: now_ms(),
            kind,
            session,
            file,
            clause_id: None,
            reason: None,
            line: None,
            matched: None,
            took_ms: None,
        }
    }

    /// Adds what a check found: the clause, the line and the matched text.
    pub fn with_finding(self, finding: &Finding) -> Event {
        let matched = match finding.matched.char_indices().nth(MATCHED_CHARS) {
            Some((end, _)) => format!("{}…", &finding.matched[..end]),
            None => finding.matched.clone(),
        };
        Event {
            clause_id: Some(finding.clause_id.clone()),
            line: Some(finding.line).filter(|line| *line > 0),
            matched: Some(matched),
            ..self
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Appends `events` in one write.
pub fn append(state_dir: &Path, events: &[Event]) -> io::Result<()> {
    if events.is_empty() {
        return Ok(());
    }
    let mut text = String::new();
    for event in events {
        text.push_str(&serde_json::to_string(event).map_err(io::Error::other)?);
        text.push('\n');
    }
    fs::create_dir_all(state_dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join(EVENTS_FILE))?;
    file.write_all(text.as_bytes())
}

/// The events in the last `max_bytes` of the log, in file order: the log is never pruned, so
/// a reader that wants only recent events reads only its end. A line the window cuts is
/// skipped, as are lines that do not parse.
pub fn read_tail(state_dir: &Path, max_bytes: u64) -> Vec<Event> {
    let Ok(mut file) = File::open(state_dir.join(EVENTS_FILE)) else {
        return Vec::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    // From one byte early, so a window that starts exactly on a line keeps that line.
    let from = start.saturating_sub(1);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(from)).is_err()
        || file.take(len - from).read_to_end(&mut bytes).is_err()
    {
        return Vec::new();
    }
    let whole_lines = match start {
        0 => &bytes[..],
        _ => match bytes.iter().position(|byte| *byte == b'\n') {
            Some(newline) => &bytes[newline + 1..],
            None => &[],
        },
    };
    String::from_utf8_lossy(whole_lines)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// A session's blocks, in `blocks/<session hash>.json`, keyed `<clause id>|<file>`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionBlocks {
    /// Edits blocked per clause and file since the last escalation.
    pub counts: BTreeMap<String, u32>,
    /// Blocks the session's next edit to the file has not yet cleared.
    pub open: BTreeSet<String>,
}

impl SessionBlocks {
    fn path(state_dir: &Path, session: &str) -> PathBuf {
        state_dir
            .join("blocks")
            .join(format!("{}.json", short_hash(session.as_bytes())))
    }

    /// The session's blocks: empty when it has none yet, and for a file an older pragma wrote
    /// (a flat map of counts) or one that does not parse.
    pub fn load(state_dir: &Path, session: &str) -> SessionBlocks {
        fs::read_to_string(Self::path(state_dir, session))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, state_dir: &Path, session: &str) -> io::Result<()> {
        let text = serde_json::to_string(self).map_err(io::Error::other)?;
        write_atomic(&Self::path(state_dir, session), text.as_bytes())
    }

    /// Counts one more block of `clause_id` on `file`, opens it, and returns the new count.
    pub fn block(&mut self, clause_id: &str, file: &str) -> u32 {
        let key = key(clause_id, file);
        self.open.insert(key.clone());
        let count = self.counts.entry(key).or_insert(0);
        *count += 1;
        *count
    }

    /// The edit went through after repeated blocks: the block is closed and its count starts
    /// over, so blocking resumes.
    pub fn escalate(&mut self, clause_id: &str, file: &str) {
        let key = key(clause_id, file);
        self.counts.remove(&key);
        self.open.remove(&key);
    }

    /// Closes the open blocks on `file` whose clause `still_broken` does not name, and returns
    /// their clause ids.
    pub fn clear(&mut self, file: &str, still_broken: impl Fn(&str) -> bool) -> Vec<String> {
        let suffix = format!("|{file}");
        let cleared: Vec<String> = self
            .open
            .iter()
            .filter_map(|key| key.strip_suffix(&suffix))
            .filter(|clause_id| !still_broken(clause_id))
            .map(str::to_string)
            .collect();
        for clause_id in &cleared {
            self.open.remove(&key(clause_id, file));
        }
        cleared
    }
}

fn key(clause_id: &str, file: &str) -> String {
    format!("{clause_id}|{file}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: EventKind) -> Event {
        Event {
            at_ms: 1,
            ..Event::new(kind, Some("s".into()), "src/a.ts".into())
        }
    }

    #[test]
    fn appends_and_reads_events() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = Event {
            took_ms: Some(3),
            ..event(EventKind::Blocked).with_finding(&Finding {
                clause_id: "c".into(),
                file: "src/a.ts".into(),
                line: 4,
                matched: "asProvisioned(t)!".into(),
            })
        };
        append(temp.path(), std::slice::from_ref(&blocked)).unwrap();
        append(
            temp.path(),
            &[event(EventKind::Unchecked), event(EventKind::Cleared)],
        )
        .unwrap();
        append(temp.path(), &[]).unwrap();
        let events = read_tail(temp.path(), u64::MAX);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], blocked);
        assert_eq!(events[0].line, Some(4));
        assert_eq!(events[0].matched.as_deref(), Some("asProvisioned(t)!"));
        assert_eq!(events[2].kind, EventKind::Cleared);
    }

    #[test]
    fn reads_only_whole_lines_in_the_tail() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_tail(temp.path(), 100).is_empty(), "no log yet");
        let events: Vec<Event> = (1..=3)
            .map(|at_ms| Event {
                at_ms,
                ..event(EventKind::Blocked)
            })
            .collect();
        append(temp.path(), &events).unwrap();
        let line_len = serde_json::to_string(&events[0]).unwrap().len() as u64 + 1;
        let at = |events: Vec<Event>| events.iter().map(|e| e.at_ms).collect::<Vec<_>>();
        assert_eq!(at(read_tail(temp.path(), u64::MAX)), vec![1, 2, 3]);
        assert_eq!(at(read_tail(temp.path(), 3 * line_len)), vec![1, 2, 3]);
        assert_eq!(
            at(read_tail(temp.path(), 2 * line_len)),
            vec![2, 3],
            "a window starting on a line keeps it"
        );
        assert_eq!(
            at(read_tail(temp.path(), 2 * line_len - 1)),
            vec![3],
            "a cut line is skipped"
        );
        assert!(read_tail(temp.path(), line_len - 1).is_empty());
        assert!(read_tail(temp.path(), 0).is_empty());
    }

    #[test]
    fn old_lines_parse_and_absent_fields_are_not_written() {
        let old = r#"{"at_ms":5,"kind":"blocked","session":"s","file":"a.ts","clause_id":"c"}"#;
        let event: Event = serde_json::from_str(old).unwrap();
        assert_eq!(event.clause_id.as_deref(), Some("c"));
        assert_eq!(
            (event.line, event.matched.as_deref(), event.took_ms),
            (None, None, None)
        );
        assert_eq!(serde_json::to_string(&event).unwrap(), old);
    }

    #[test]
    fn a_finding_about_the_file_has_no_line_and_long_matches_are_cut() {
        let path = event(EventKind::Blocked).with_finding(&Finding {
            clause_id: "naming".into(),
            file: "src/BadName.ts".into(),
            line: 0,
            matched: "src/BadName.ts".into(),
        });
        assert_eq!(path.line, None);
        let long = "é".repeat(MATCHED_CHARS + 10);
        let cut = event(EventKind::Blocked).with_finding(&Finding {
            clause_id: "c".into(),
            file: "src/a.ts".into(),
            line: 1,
            matched: long,
        });
        let matched = cut.matched.unwrap();
        assert_eq!(matched.chars().count(), MATCHED_CHARS + 1);
        assert!(matched.ends_with('…'));
    }

    #[test]
    fn block_counts_are_per_session_clause_and_file() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        let bump = |session: &str, clause: &str, file: &str| {
            let mut blocks = SessionBlocks::load(state, session);
            let count = blocks.block(clause, file);
            blocks.save(state, session).unwrap();
            count
        };
        assert_eq!(bump("s1", "c", "a.ts"), 1);
        assert_eq!(bump("s1", "c", "a.ts"), 2);
        assert_eq!(bump("s1", "c", "b.ts"), 1);
        assert_eq!(bump("s1", "d", "a.ts"), 1);
        assert_eq!(bump("s2", "c", "a.ts"), 1);
        let mut blocks = SessionBlocks::load(state, "s1");
        blocks.escalate("c", "a.ts");
        blocks.save(state, "s1").unwrap();
        assert_eq!(
            bump("s1", "c", "a.ts"),
            1,
            "an escalation starts the count over"
        );
    }

    #[test]
    fn an_edit_clears_only_its_files_blocks_that_it_no_longer_breaks() {
        let mut blocks = SessionBlocks::default();
        blocks.block("c", "a.ts");
        blocks.block("d", "a.ts");
        blocks.block("c", "b.ts");
        assert_eq!(blocks.clear("a.ts", |clause| clause == "d"), vec!["c"]);
        assert_eq!(
            blocks.open,
            BTreeSet::from(["c|b.ts".to_string(), "d|a.ts".to_string()])
        );
        assert!(blocks.clear("a.ts", |clause| clause == "d").is_empty());
        assert_eq!(blocks.clear("a.ts", |_| false), vec!["d"]);
        assert_eq!(blocks.counts["c|a.ts"], 1, "clearing keeps the count");
        assert_eq!(blocks.clear("other.ts", |_| false), Vec::<String>::new());
    }

    #[test]
    fn escalation_closes_the_block() {
        let mut blocks = SessionBlocks::default();
        blocks.block("c", "a.ts");
        blocks.escalate("c", "a.ts");
        assert!(blocks.open.is_empty());
        assert!(blocks.counts.is_empty());
        assert!(blocks.clear("a.ts", |_| false).is_empty());
    }

    #[test]
    fn loads_the_current_shape_and_starts_over_from_anything_else() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path();
        assert_eq!(SessionBlocks::load(state, "s"), SessionBlocks::default());
        let mut blocks = SessionBlocks::default();
        blocks.block("c", "a.ts");
        blocks.save(state, "s").unwrap();
        let path = SessionBlocks::path(state, "s");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"counts":{"c|a.ts":1},"open":["c|a.ts"]}"#
        );
        assert_eq!(SessionBlocks::load(state, "s"), blocks);
        for older in [r#"{"c|a.ts":2}"#, "not json", "[1,2]"] {
            fs::write(&path, older).unwrap();
            assert_eq!(SessionBlocks::load(state, "s"), SessionBlocks::default());
        }
    }
}
