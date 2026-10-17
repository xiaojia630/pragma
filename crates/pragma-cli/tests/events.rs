mod common;

use std::fs;
use std::path::Path;

use common::{TestRepo, pattern_clause, pragma, stderr, stdout};
use serde_json::{Value, json};

/// A repo with a `no-bang` rule and a file that keeps it.
fn enabled_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.lock(json!([pattern_clause(
        "no-bang",
        "asProvisioned($$$ARGS)!",
        &[],
        "const id = asProvisioned(t)!.id;",
        "const p = asProvisioned(t);"
    )]));
    repo.write("src/a.ts", "const p = asProvisioned(task);\n");
    repo.commit();
    repo
}

/// An Edit of `src/a.ts` in `checkout` that adds a violation on line 2.
fn violating(checkout: &Path, session: &str) -> Value {
    json!({
        "session_id": session, "cwd": checkout, "tool_name": "Edit",
        "tool_input": { "file_path": checkout.join("src/a.ts"),
                        "old_string": "const p = asProvisioned(task);",
                        "new_string": "const p = asProvisioned(task);\nconst id = asProvisioned(task)!.id;" }
    })
}

/// Writes these events as the checkout's log, one JSON line each.
fn write_log(state_dir: &Path, events: &[Value]) {
    fs::create_dir_all(state_dir).unwrap();
    let lines: Vec<String> = events.iter().map(Value::to_string).collect();
    fs::write(state_dir.join("events.jsonl"), lines.join("\n") + "\n").unwrap();
}

fn blocked_at(at_ms: u64, clause: &str) -> Value {
    json!({ "at_ms": at_ms, "kind": "blocked", "session": "s", "file": "src/a.ts",
            "clause_id": clause, "line": 2, "matched": "asProvisioned(t)!", "took_ms": 4 })
}

fn events_in(dir: &Path, args: &[&str]) -> Vec<Value> {
    let output = pragma()
        .args([&["events", "--json"], args].concat())
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
    parsed["events"].as_array().unwrap().clone()
}

fn at_ms(events: &[Value]) -> Vec<u64> {
    events
        .iter()
        .map(|event| event["at_ms"].as_u64().unwrap())
        .collect()
}

#[test]
fn lists_a_block_with_its_clause() {
    let repo = enabled_repo();
    assert_eq!(
        repo.hook(violating(&repo.root, "session-1"), &[])
            .status
            .code(),
        Some(2)
    );
    let events = events_in(&repo.root, &[]);
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event["kind"], "blocked");
    assert_eq!(event["session"], "session-1");
    assert_eq!(event["file"], "src/a.ts");
    assert_eq!(event["clause_id"], "no-bang");
    assert_eq!(event["line"], 2);
    assert_eq!(event["matched"], "asProvisioned(task)!");
    assert!(event["took_ms"].is_u64());
    assert!(event["at_ms"].as_u64().unwrap() > 1_600_000_000_000);
    let clause = &event["clause"];
    assert_eq!(clause["id"], "no-bang");
    assert_eq!(clause["text"], "Never write asProvisioned($$$ARGS)!.");
    assert_eq!(clause["fix"], "Use an explicit null check.");
    assert_eq!(clause["rule"], "forbidden-pattern");
    assert_eq!(clause["source"]["path"], "AGENTS.md");
    assert_eq!(clause["source"]["lines"], json!([10, 10]));
    assert_eq!(clause["source"]["heading_path"], json!(["Rules"]));
}

#[test]
fn limit_keeps_the_most_recent_oldest_first() {
    let repo = enabled_repo();
    let state = repo.root.join(".git/pragma");
    // Hooks running side by side can append out of order.
    write_log(
        &state,
        &[
            blocked_at(100, "no-bang"),
            blocked_at(300, "no-bang"),
            blocked_at(200, "no-bang"),
            blocked_at(400, "no-bang"),
        ],
    );
    assert_eq!(at_ms(&events_in(&repo.root, &[])), vec![100, 200, 300, 400]);
    assert_eq!(
        at_ms(&events_in(&repo.root, &["--limit", "2"])),
        vec![300, 400]
    );
    assert!(events_in(&repo.root, &["--limit", "0"]).is_empty());
}

#[test]
fn since_ms_keeps_only_later_events() {
    let repo = enabled_repo();
    let state = repo.root.join(".git/pragma");
    write_log(
        &state,
        &[
            blocked_at(100, "no-bang"),
            blocked_at(200, "no-bang"),
            blocked_at(300, "no-bang"),
        ],
    );
    assert_eq!(
        at_ms(&events_in(&repo.root, &["--since-ms", "200"])),
        vec![300],
        "the cursor itself is excluded"
    );
    assert_eq!(
        at_ms(&events_in(
            &repo.root,
            &["--since-ms", "99", "--limit", "1"]
        )),
        vec![300]
    );
    assert!(events_in(&repo.root, &["--since-ms", "300"]).is_empty());
}

#[test]
fn an_unknown_clause_keeps_its_id_and_has_no_clause() {
    let repo = enabled_repo();
    write_log(
        &repo.root.join(".git/pragma"),
        &[
            blocked_at(100, "gone"),
            json!({ "at_ms": 200, "kind": "unchecked", "file": "src/a.ts",
                    "reason": "file over 512 KB" }),
            json!({ "at_ms": 300, "kind": "blocked", "file": "src/a.ts", "clause_id": "no-bang" }),
            json!("not an event"),
        ],
    );
    let events = events_in(&repo.root, &[]);
    assert_eq!(events.len(), 3, "a line that does not parse is skipped");
    assert_eq!(events[0]["clause_id"], "gone");
    assert_eq!(events[0]["clause"], Value::Null);
    assert_eq!(events[1]["reason"], "file over 512 KB");
    assert_eq!(events[1]["clause"], Value::Null);
    assert!(events[1].get("clause_id").is_none());
    assert_eq!(events[2]["clause"]["id"], "no-bang");
    assert!(
        events[2].get("line").is_none(),
        "an event from an older pragma"
    );
}

#[test]
fn without_pragma_or_events_the_list_is_empty() {
    let repo = enabled_repo();
    assert!(events_in(&repo.root, &[]).is_empty(), "no events file");
    write_log(
        &repo.root.join(".git/pragma"),
        &[blocked_at(100, "no-bang")],
    );
    fs::remove_dir_all(repo.root.join(".pragma")).unwrap();
    assert!(events_in(&repo.root, &[]).is_empty(), "no .pragma/");
    let output = repo.run(&["events"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "No edit checks recorded in this checkout.\n"
    );
}

#[test]
fn a_worktree_lists_its_own_events() {
    let repo = enabled_repo();
    repo.git(&["worktree", "add", "-q", "../task", "-b", "task"]);
    let task = repo.temp_dir().join("task");
    assert_eq!(
        repo.hook(violating(&repo.root, "root-session"), &[])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        repo.hook(violating(&task, "task-session"), &[])
            .status
            .code(),
        Some(2)
    );
    let in_task = events_in(&task, &[]);
    assert_eq!(in_task.len(), 1);
    assert_eq!(in_task[0]["session"], "task-session");
    assert_eq!(
        in_task[0]["clause"]["id"], "no-bang",
        "joined with the root checkout's lock"
    );
    let in_root = events_in(&repo.root, &[]);
    assert_eq!(in_root.len(), 1);
    assert_eq!(in_root[0]["session"], "root-session");
}

#[test]
fn prints_one_line_per_event() {
    let repo = enabled_repo();
    // 2026-10-16 09:05:07.250 UTC.
    let at = 1_792_141_507_250_u64;
    write_log(
        &repo.root.join(".git/pragma"),
        &[
            blocked_at(at, "no-bang"),
            json!({ "at_ms": at + 1000, "kind": "cleared", "session": "s", "file": "src/a.ts",
                    "clause_id": "gone" }),
            json!({ "at_ms": at + 2000, "kind": "unchecked", "file": "src/big.ts",
                    "reason": "file over 512 KB" }),
        ],
    );
    let output = pragma()
        .args(["events"])
        .env("TZ", "UTC")
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "09:05:07  blocked    src/a.ts:2  Never write asProvisioned($$$ARGS)!.\n\
         09:05:08  cleared    src/a.ts  gone\n\
         09:05:09  unchecked  src/big.ts  file over 512 KB\n"
    );
}

#[test]
fn a_long_log_still_gives_its_newest_events() {
    let repo = enabled_repo();
    let events: Vec<Value> = (1..=3000).map(|at| blocked_at(at, "no-bang")).collect();
    let state = repo.root.join(".git/pragma");
    write_log(&state, &events);
    let size = fs::metadata(state.join("events.jsonl")).unwrap().len();
    assert!(size > 256 * 1024, "{size} bytes");
    assert_eq!(
        at_ms(&events_in(&repo.root, &["--limit", "3"])),
        vec![2998, 2999, 3000]
    );
    let newest = events_in(&repo.root, &[]);
    assert_eq!(newest.len(), 200);
    assert_eq!(newest[0]["at_ms"], 2801);
    assert_eq!(newest[0]["clause"]["id"], "no-bang");
    let tail = events_in(&repo.root, &["--limit", "100000"]);
    assert!(
        tail.len() < 3000 && tail.last().unwrap()["at_ms"] == 3000,
        "only the end of the log is read: {} events",
        tail.len()
    );
    assert_eq!(
        at_ms(&events_in(&repo.root, &["--since-ms", "2999"])),
        vec![3000]
    );
}
