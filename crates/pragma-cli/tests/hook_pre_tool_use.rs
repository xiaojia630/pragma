mod common;

use common::{TestRepo, pattern_clause, protected_clause, stderr};
use serde_json::json;

fn enabled_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.lock(json!([
        pattern_clause(
            "no-bang",
            "asProvisioned($$$ARGS)!",
            &[],
            "const id = asProvisioned(t)!.id;",
            "const p = asProvisioned(t);"
        ),
        protected_clause("migrations", &["drizzle/[0-9]*.sql"]),
    ]));
    repo.write("src/a.ts", "const p = asProvisioned(task);\n");
    repo.commit();
    repo
}

fn events(repo: &TestRepo) -> String {
    std::fs::read_to_string(repo.root.join(".git/pragma/events.jsonl")).unwrap_or_default()
}

#[test]
fn blocks_an_edit_that_introduces_a_violation() {
    let repo = enabled_repo();
    let input = repo.edit_input(
        "src/a.ts",
        "const p = asProvisioned(task);",
        "const id = asProvisioned(task)!.id;",
    );
    let output = repo.hook(input, &[]);
    assert_eq!(output.status.code(), Some(2));
    let message = stderr(&output);
    assert!(
        message.starts_with("pragma blocked this edit.\n"),
        "{message}"
    );
    assert!(
        message.contains("Rule (AGENTS.md:10, \"Rules\"): Never write asProvisioned($$$ARGS)!."),
        "{message}"
    );
    assert!(
        message.contains("Found: src/a.ts:1  const id = asProvisioned(task)!.id;"),
        "{message}"
    );
    assert!(events(&repo).contains("\"kind\":\"blocked\""));
}

#[test]
fn allows_clean_edits_and_moved_violations() {
    let repo = enabled_repo();
    let clean = repo.edit_input("src/a.ts", "const p", "const provisioned");
    assert_eq!(repo.hook(clean, &[]).status.code(), Some(0));
    repo.write("src/b.ts", "const id = asProvisioned(t)!.id;\nexport {};\n");
    let moved = repo.edit_input(
        "src/b.ts",
        "const id = asProvisioned(t)!.id;\nexport {};",
        "export {};\nconst id = asProvisioned(t)!.id;",
    );
    assert_eq!(repo.hook(moved, &[]).status.code(), Some(0));
}

#[test]
fn blocks_writes_to_protected_paths_and_new_files_with_violations() {
    let repo = enabled_repo();
    let output = repo.hook(repo.write_input("drizzle/0007_new.sql", "select 1;"), &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("Found: drizzle/0007_new.sql\n"));
    let output = repo.hook(
        repo.write_input("src/new.ts", "export const x = asProvisioned(t)!;\n"),
        &[],
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn escalates_after_three_blocks_then_blocks_again() {
    let repo = enabled_repo();
    let input = || {
        repo.edit_input(
            "src/a.ts",
            "const p = asProvisioned(task);",
            "const id = asProvisioned(task)!.id;",
        )
    };
    for _ in 0..3 {
        assert_eq!(repo.hook(input(), &[]).status.code(), Some(2));
    }
    assert_eq!(repo.hook(input(), &[]).status.code(), Some(0));
    assert!(events(&repo).contains("\"kind\":\"escalated\""));
    assert_eq!(
        repo.hook(input(), &[]).status.code(),
        Some(2),
        "blocking resumes after an escalation"
    );
}

#[test]
fn an_edit_without_a_session_id_always_blocks() {
    let repo = enabled_repo();
    let mut input = repo.edit_input(
        "src/a.ts",
        "const p = asProvisioned(task);",
        "const id = asProvisioned(task)!.id;",
    );
    input.as_object_mut().unwrap().remove("session_id");
    for _ in 0..4 {
        assert_eq!(repo.hook(input.clone(), &[]).status.code(), Some(2));
    }
    assert!(!events(&repo).contains("escalated"));
}

#[test]
fn fails_open() {
    let repo = enabled_repo();
    let violating = repo.edit_input(
        "src/a.ts",
        "const p = asProvisioned(task);",
        "const id = asProvisioned(task)!.id;",
    );
    assert_eq!(
        repo.hook(violating.clone(), &[("PRAGMA_JUDGE", "1")])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        repo.hook(violating.clone(), &[("PRAGMA_DISABLE", "1")])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        repo.hook(json!("not hook input"), &[]).status.code(),
        Some(0)
    );
    let outside = repo.temp_dir().join("elsewhere.ts");
    let input = json!({ "cwd": repo.root, "tool_name": "Write",
                        "tool_input": { "file_path": outside, "content": "asProvisioned(t)!" } });
    assert_eq!(repo.hook(input, &[]).status.code(), Some(0));
    repo.write(".pragma/pragma.lock", r#"{"format": 99}"#);
    assert_eq!(repo.hook(violating, &[]).status.code(), Some(0));
    assert!(events(&repo).contains("update pragma"));
}

#[test]
fn does_nothing_when_paused_or_not_enabled() {
    let repo = enabled_repo();
    let violating = repo.edit_input(
        "src/a.ts",
        "const p = asProvisioned(task);",
        "const id = asProvisioned(task)!.id;",
    );
    assert!(repo.run(&["pause"]).status.success());
    assert_eq!(repo.hook(violating.clone(), &[]).status.code(), Some(0));
    assert!(repo.run(&["resume"]).status.success());
    assert_eq!(repo.hook(violating.clone(), &[]).status.code(), Some(2));
    std::fs::remove_dir_all(repo.root.join(".pragma")).unwrap();
    assert_eq!(repo.hook(violating, &[]).status.code(), Some(0));
}

#[test]
fn a_worktree_uses_the_root_checkouts_lock() {
    let repo = enabled_repo();
    repo.git(&["worktree", "add", "-q", "../task", "-b", "task"]);
    let task = repo.temp_dir().join("task");
    let input = json!({
        "session_id": "s", "cwd": task, "tool_name": "Edit",
        "tool_input": { "file_path": task.join("src/a.ts"),
                        "old_string": "const p = asProvisioned(task);",
                        "new_string": "const id = asProvisioned(task)!.id;" }
    });
    assert_eq!(repo.hook(input, &[]).status.code(), Some(2));
}

#[test]
fn an_unknown_hook_event_or_flag_is_let_through() {
    for args in [
        &["hook", "bogus-event"][..],
        &["hook", "pre-tool-use", "--foo"][..],
    ] {
        let output = common::pragma().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(stderr(&output), "", "{args:?}");
    }
}

#[test]
fn a_check_past_the_deadline_lets_the_edit_through() {
    let repo = enabled_repo();
    let input = repo.edit_input(
        "src/a.ts",
        "const p = asProvisioned(task);",
        "const id = asProvisioned(task)!.id;",
    );
    let output = repo.hook(input, &[("PRAGMA_HOOK_DEADLINE_MS", "0")]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(stderr(&output), "");
}

/// The recorded events, as JSON.
fn recorded(repo: &TestRepo) -> Vec<serde_json::Value> {
    events(repo)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Each recorded event's kind, session and clause.
fn kinds(repo: &TestRepo) -> Vec<(String, String, String)> {
    recorded(repo)
        .iter()
        .map(|event| {
            let field = |key: &str| event[key].as_str().unwrap_or_default().to_string();
            (field("kind"), field("session"), field("clause_id"))
        })
        .collect()
}

fn kind(kind: &str, session: &str, clause: &str) -> (String, String, String) {
    (kind.into(), session.into(), clause.into())
}

fn in_session(mut input: serde_json::Value, session: &str) -> serde_json::Value {
    input["session_id"] = json!(session);
    input
}

/// An edit to `src/a.ts` that adds a violation of `no-bang` on line 2.
fn violating(repo: &TestRepo) -> serde_json::Value {
    repo.edit_input(
        "src/a.ts",
        "const p = asProvisioned(task);",
        "const p = asProvisioned(task);\nconst id = asProvisioned(task)!.id;",
    )
}

/// An edit to `src/a.ts` that breaks nothing.
fn clean(repo: &TestRepo) -> serde_json::Value {
    repo.edit_input("src/a.ts", "const p", "const provisioned")
}

#[test]
fn a_block_records_its_line_matched_text_and_time() {
    let repo = enabled_repo();
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    let events = recorded(&repo);
    assert_eq!(events.len(), 1);
    let block = &events[0];
    assert_eq!(block["kind"], "blocked");
    assert_eq!(block["session"], "session-1");
    assert_eq!(block["file"], "src/a.ts");
    assert_eq!(block["clause_id"], "no-bang");
    assert_eq!(block["line"], 2);
    assert_eq!(block["matched"], "asProvisioned(task)!");
    assert!(block["took_ms"].as_u64().unwrap() < 50, "{block}");
    assert!(block.get("reason").is_none());

    let output = repo.hook(repo.write_input("drizzle/0007_new.sql", "select 1;"), &[]);
    assert_eq!(output.status.code(), Some(2));
    let path_block = &recorded(&repo)[1];
    assert_eq!(path_block["clause_id"], "migrations");
    assert!(path_block.get("line").is_none(), "{path_block}");
    assert_eq!(path_block["matched"], "drizzle/0007_new.sql");
}

#[test]
fn the_sessions_next_clean_edit_to_the_file_clears_the_block() {
    let repo = enabled_repo();
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    assert_eq!(repo.hook(clean(&repo), &[]).status.code(), Some(0));
    assert_eq!(
        kinds(&repo),
        vec![
            kind("blocked", "session-1", "no-bang"),
            kind("cleared", "session-1", "no-bang"),
        ]
    );
    let cleared = &recorded(&repo)[1];
    assert_eq!(cleared["file"], "src/a.ts");
    assert!(cleared["took_ms"].is_u64());
    assert!(cleared.get("line").is_none());
    assert_eq!(repo.hook(clean(&repo), &[]).status.code(), Some(0));
    assert_eq!(kinds(&repo).len(), 2, "a closed block is not cleared twice");
}

#[test]
fn edits_from_another_session_or_to_another_file_clear_nothing() {
    let repo = enabled_repo();
    repo.write("src/b.ts", "export {};\n");
    repo.commit();
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    let other_session = in_session(clean(&repo), "session-2");
    assert_eq!(repo.hook(other_session, &[]).status.code(), Some(0));
    let other_file = repo.edit_input("src/b.ts", "export {};", "export const b = 1;");
    assert_eq!(repo.hook(other_file, &[]).status.code(), Some(0));
    let mut no_session = clean(&repo);
    no_session.as_object_mut().unwrap().remove("session_id");
    assert_eq!(repo.hook(no_session, &[]).status.code(), Some(0));
    assert_eq!(kinds(&repo), vec![kind("blocked", "session-1", "no-bang")]);
    assert_eq!(repo.hook(clean(&repo), &[]).status.code(), Some(0));
    assert_eq!(kinds(&repo)[1], kind("cleared", "session-1", "no-bang"));
}

#[test]
fn a_second_block_keeps_the_block_open() {
    let repo = enabled_repo();
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    assert_eq!(
        kinds(&repo),
        vec![
            kind("blocked", "session-1", "no-bang"),
            kind("blocked", "session-1", "no-bang"),
        ]
    );
    assert_eq!(repo.hook(clean(&repo), &[]).status.code(), Some(0));
    assert_eq!(
        kinds(&repo)[2..],
        [kind("cleared", "session-1", "no-bang")],
        "one clear closes the block however often it blocked"
    );
}

#[test]
fn an_edit_blocked_by_another_clause_clears_the_ones_it_no_longer_breaks() {
    let repo = enabled_repo();
    repo.lock(json!([
        pattern_clause(
            "no-bang",
            "asProvisioned($$$ARGS)!",
            &[],
            "asProvisioned(t)!",
            "asProvisioned(t)"
        ),
        pattern_clause(
            "no-require",
            "require($$$A)",
            &[],
            "require('a')",
            "import a from 'a'"
        ),
    ]));
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    let other = repo.edit_input(
        "src/a.ts",
        "const p = asProvisioned(task);",
        "const p = asProvisioned(task);\nconst x = require('x');",
    );
    assert_eq!(repo.hook(other, &[]).status.code(), Some(2));
    assert_eq!(
        kinds(&repo),
        vec![
            kind("blocked", "session-1", "no-bang"),
            kind("cleared", "session-1", "no-bang"),
            kind("blocked", "session-1", "no-require"),
        ]
    );
}

#[test]
fn escalation_closes_the_block() {
    let repo = enabled_repo();
    for _ in 0..3 {
        assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    }
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(0));
    let escalated = recorded(&repo).pop().unwrap();
    assert_eq!(escalated["kind"], "escalated");
    assert_eq!(escalated["line"], 2);
    assert_eq!(escalated["matched"], "asProvisioned(task)!");
    assert_eq!(repo.hook(clean(&repo), &[]).status.code(), Some(0));
    assert_eq!(
        kinds(&repo).last().unwrap(),
        &kind("escalated", "session-1", "no-bang"),
        "an escalated block is closed, so nothing is cleared"
    );
}

#[test]
fn an_unchecked_edit_clears_nothing() {
    let repo = enabled_repo();
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    let big = format!(
        "const p = asProvisioned(task);\n{}",
        "// pad\n".repeat(80_000)
    );
    assert_eq!(
        repo.hook(repo.write_input("src/a.ts", &big), &[])
            .status
            .code(),
        Some(0)
    );
    let past_deadline = repo.hook(clean(&repo), &[("PRAGMA_HOOK_DEADLINE_MS", "0")]);
    assert_eq!(past_deadline.status.code(), Some(0));
    let after: Vec<(String, String, String)> = kinds(&repo);
    assert_eq!(after[0], kind("blocked", "session-1", "no-bang"));
    assert_eq!(after[1], kind("unchecked", "session-1", ""));
    assert!(
        !after.iter().any(|(kind, ..)| kind == "cleared"),
        "{after:?}"
    );
    assert_eq!(repo.hook(clean(&repo), &[]).status.code(), Some(0));
    assert_eq!(
        kinds(&repo).last().unwrap(),
        &kind("cleared", "session-1", "no-bang"),
        "the block stayed open"
    );
}

#[test]
fn an_older_blocks_file_starts_the_session_over() {
    let repo = enabled_repo();
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    let blocks: Vec<_> = std::fs::read_dir(repo.root.join(".git/pragma/blocks"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(blocks.len(), 1);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&blocks[0]).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!({ "counts": { "no-bang|src/a.ts": 1 }, "open": ["no-bang|src/a.ts"] })
    );
    // The flat map of counts an older pragma wrote: a count of 3 would escalate the next block.
    std::fs::write(&blocks[0], r#"{"no-bang|src/a.ts": 3}"#).unwrap();
    assert_eq!(
        repo.hook(violating(&repo), &[]).status.code(),
        Some(2),
        "the old counts are not read"
    );
    std::fs::write(&blocks[0], "not json").unwrap();
    assert_eq!(repo.hook(violating(&repo), &[]).status.code(), Some(2));
    assert_eq!(repo.hook(clean(&repo), &[]).status.code(), Some(0));
    assert_eq!(
        kinds(&repo).last().unwrap(),
        &kind("cleared", "session-1", "no-bang")
    );
}

#[test]
fn an_edit_counts_once_toward_escalation_however_many_violations_it_has() {
    let repo = enabled_repo();
    let four = "const a = asProvisioned(task)!.id;\n".repeat(4);
    let input = || {
        repo.edit_input(
            "src/a.ts",
            "const p = asProvisioned(task);",
            &format!("const p = asProvisioned(task);\n{four}"),
        )
    };
    let first = repo.hook(input(), &[]);
    assert_eq!(first.status.code(), Some(2));
    assert_eq!(
        stderr(&first).matches("Found: src/a.ts:").count(),
        4,
        "the message lists every finding"
    );
    assert_eq!(
        kinds(&repo),
        vec![kind("blocked", "session-1", "no-bang"); 4]
    );
    for _ in 0..2 {
        assert_eq!(repo.hook(input(), &[]).status.code(), Some(2));
    }
    assert_eq!(
        repo.hook(input(), &[]).status.code(),
        Some(0),
        "the fourth blocked edit is let through"
    );
    let all = kinds(&repo);
    assert_eq!(all.len(), 16);
    assert!(all[..12].iter().all(|(kind, ..)| kind == "blocked"));
    assert_eq!(
        all[12..],
        vec![kind("escalated", "session-1", "no-bang"); 4]
    );
}

/// `enabled_repo` with the lock `no-bang` and `no-require`, which an edit can break at once.
fn two_clause_repo() -> TestRepo {
    let repo = enabled_repo();
    repo.lock(json!([
        pattern_clause(
            "no-bang",
            "asProvisioned($$$ARGS)!",
            &[],
            "asProvisioned(t)!",
            "asProvisioned(t)"
        ),
        pattern_clause(
            "no-require",
            "require($$$A)",
            &[],
            "require('a')",
            "import a from 'a'"
        ),
    ]));
    repo
}

const BANG: &str = "const id = asProvisioned(task)!.id;";
const REQUIRE: &str = "const x = require('x');";

/// An edit to `src/a.ts` that adds the given lines.
fn adding(repo: &TestRepo, lines: &[&str]) -> serde_json::Value {
    repo.edit_input(
        "src/a.ts",
        "const p = asProvisioned(task);",
        &format!("const p = asProvisioned(task);\n{}", lines.join("\n")),
    )
}

/// The session's saved blocks: counts per `<clause>|<file>` and the ones still open.
fn session_blocks(repo: &TestRepo) -> serde_json::Value {
    let mut files = std::fs::read_dir(repo.root.join(".git/pragma/blocks")).unwrap();
    let path = files.next().unwrap().unwrap().path();
    assert!(files.next().is_none(), "one session, one file");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Each recorded event's kind and clause since the first `skip` events, in a stable order.
fn recorded_since(repo: &TestRepo, skip: usize) -> Vec<(String, String)> {
    let mut events: Vec<(String, String)> = kinds(repo)[skip..]
        .iter()
        .map(|(kind, _, clause)| (kind.clone(), clause.clone()))
        .collect();
    events.sort();
    events
}

fn pair(kind: &str, clause: &str) -> (String, String) {
    (kind.into(), clause.into())
}

#[test]
fn two_clauses_in_one_edit_count_independently() {
    let repo = two_clause_repo();
    for _ in 0..2 {
        let both = adding(&repo, &[BANG, REQUIRE]);
        let output = repo.hook(both, &[]);
        assert_eq!(output.status.code(), Some(2));
        let message = stderr(&output);
        assert!(message.contains("asProvisioned"), "{message}");
        assert!(message.contains("require("), "{message}");
    }
    assert_eq!(
        session_blocks(&repo),
        json!({
            "counts": { "no-bang|src/a.ts": 2, "no-require|src/a.ts": 2 },
            "open": ["no-bang|src/a.ts", "no-require|src/a.ts"]
        })
    );
    // Edits breaking only `no-bang` take it to its limit; `no-require`'s count is not moved.
    assert_eq!(
        repo.hook(adding(&repo, &[BANG]), &[]).status.code(),
        Some(2)
    );
    assert_eq!(
        repo.hook(adding(&repo, &[BANG]), &[]).status.code(),
        Some(0),
        "no-bang's fourth block is let through: nothing else blocks the edit"
    );
    assert_eq!(
        session_blocks(&repo),
        json!({ "counts": { "no-require|src/a.ts": 2 }, "open": [] })
    );
    // Starting over for one clause leaves the other where it was.
    assert_eq!(
        repo.hook(adding(&repo, &[BANG, REQUIRE]), &[])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        session_blocks(&repo)["counts"],
        json!({ "no-bang|src/a.ts": 1, "no-require|src/a.ts": 3 })
    );
}

#[test]
fn a_clause_at_its_limit_is_blocked_while_another_clause_still_blocks_the_edit() {
    let repo = two_clause_repo();
    for _ in 0..3 {
        assert_eq!(
            repo.hook(adding(&repo, &[BANG]), &[]).status.code(),
            Some(2)
        );
    }
    let before = kinds(&repo).len();
    // no-bang's fourth block, in an edit no-require also blocks: the edit does not go through.
    let both = repo.hook(adding(&repo, &[BANG, REQUIRE]), &[]);
    assert_eq!(both.status.code(), Some(2));
    assert!(stderr(&both).contains("asProvisioned"), "{}", stderr(&both));
    assert_eq!(
        recorded_since(&repo, before),
        vec![pair("blocked", "no-bang"), pair("blocked", "no-require")],
        "no escalation for an edit that was blocked"
    );
    assert_eq!(
        session_blocks(&repo),
        json!({
            "counts": { "no-bang|src/a.ts": 4, "no-require|src/a.ts": 1 },
            "open": ["no-bang|src/a.ts", "no-require|src/a.ts"]
        }),
        "no-bang's count is kept and its block is still open"
    );

    // The next edit breaks only no-bang, past its limit: nothing else blocks it, so it goes through.
    let before = kinds(&repo).len();
    assert_eq!(
        repo.hook(adding(&repo, &[BANG]), &[]).status.code(),
        Some(0)
    );
    assert_eq!(
        recorded_since(&repo, before),
        vec![pair("cleared", "no-require"), pair("escalated", "no-bang")]
    );
    assert_eq!(
        session_blocks(&repo),
        json!({ "counts": { "no-require|src/a.ts": 1 }, "open": [] })
    );
}
