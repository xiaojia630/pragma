mod common;

use common::{TestRepo, pattern_clause, stdout};
use serde_json::{Value, json};

fn status(repo: &TestRepo, config_dir: &std::path::Path) -> Value {
    let output = common::pragma()
        .args(["status", "--json"])
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", common::stderr(&output));
    serde_json::from_str(&stdout(&output)).unwrap()
}

#[test]
fn status_of_a_repo_without_pragma_exits_0_and_says_not_enabled() {
    let repo = TestRepo::new();
    let config = repo.temp_dir().join("claude");
    let value = status(&repo, &config);
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["lock_format"], 1);
    assert_eq!(value["enabled"], false);
    assert_eq!(value["is_root_checkout"], true);
    assert_eq!(value["pragma_dir"], Value::Null);
    assert_eq!(value["paused"], false);
    assert_eq!(value["hooks"]["installed"], false);
    assert_eq!(
        value["hooks"]["settings"],
        config.join("settings.json").to_str().unwrap()
    );
    assert_eq!(value["clauses"], Value::Null);
    assert_eq!(value["lock_error"], Value::Null);
    assert_eq!(value["extraction"], json!({ "state": "never" }));
    assert_eq!(value["lock_pending"], false);
    assert_eq!(value["lock_unsettled"], 0);
}

#[test]
fn status_of_an_enabled_repo_counts_clauses_and_reports_pause_and_hooks() {
    let repo = TestRepo::new();
    repo.lock(json!([pattern_clause(
        "no-require",
        "require($$$A)",
        &["**/*.ts"],
        "require('a');",
        "import a from 'a';"
    )]));
    let config = repo.temp_dir().join("claude");
    assert!(
        common::pragma()
            .args(["hooks", "install"])
            .env("CLAUDE_CONFIG_DIR", &config)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(repo.run(&["pause"]).status.success());
    let value = status(&repo, &config);
    assert_eq!(value["enabled"], true);
    assert_eq!(
        value["pragma_dir"],
        repo.root.join(".pragma").to_str().unwrap()
    );
    assert_eq!(value["paused"], true);
    assert_eq!(value["hooks"]["installed"], true);
    assert_eq!(value["clauses"], 1);
    assert_eq!(value["lock_error"], Value::Null);
}

#[test]
fn status_reports_a_lock_it_cannot_read_instead_of_failing() {
    let repo = TestRepo::new();
    repo.write(".pragma/pragma.lock", r#"{"format": 99, "clauses": []}"#);
    let value = status(&repo, &repo.temp_dir().join("claude"));
    assert_eq!(value["enabled"], true);
    assert_eq!(value["clauses"], Value::Null);
    assert!(
        value["lock_error"]
            .as_str()
            .unwrap()
            .contains("update pragma")
    );
}

#[test]
fn status_in_a_worktree_is_not_the_root_checkout() {
    let repo = TestRepo::new();
    repo.write(".pragma/pragma.lock", r#"{"format": 1, "clauses": []}"#);
    repo.commit();
    let task = repo.temp_dir().join("task");
    repo.git(&[
        "worktree",
        "add",
        "-q",
        task.to_str().unwrap(),
        "-b",
        "task",
    ]);
    let output = common::pragma()
        .args(["status", "--json"])
        .current_dir(&task)
        .output()
        .unwrap();
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["is_root_checkout"], false);
    assert_eq!(value["enabled"], true);
    assert_eq!(value["clauses"], 0);
}

#[test]
fn status_human_output_and_outside_a_repo() {
    let repo = TestRepo::new();
    let output = repo.run(&["status"]);
    assert!(output.status.success());
    assert!(
        stdout(&output).contains("not turned on"),
        "{}",
        stdout(&output)
    );
    let outside = tempfile::tempdir().unwrap();
    let output = common::pragma()
        .arg("status")
        .current_dir(outside.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(common::stderr(&output).contains("not inside a git repository"));
}

#[test]
fn hooks_status_reads_the_settings_without_writing() {
    let repo = TestRepo::new();
    let settings = repo.temp_dir().join("claude/settings.json");
    let path = settings.to_str().unwrap();
    let run = || {
        let output = repo.run(&["hooks", "status", "--settings", path, "--json"]);
        assert!(output.status.success());
        serde_json::from_str::<Value>(&stdout(&output)).unwrap()
    };
    let before = run();
    assert_eq!(before["installed"], false);
    assert_eq!(before["settings"], path);
    assert_eq!(
        before["events"],
        serde_json::json!([
            { "event": "PreToolUse", "matcher": "Edit|Write|MultiEdit", "subcommand": "pre-tool-use" }
        ])
    );
    assert!(!settings.exists());
    repo.run(&["hooks", "install", "--settings", path]);
    assert_eq!(run()["installed"], true);
    repo.run(&["hooks", "uninstall", "--settings", path]);
    assert_eq!(run()["installed"], false);
}

#[cfg(unix)]
#[test]
fn status_reports_extraction_done_then_paused_with_the_reason() {
    let repo = TestRepo::new();
    repo.write(
        "AGENTS.md",
        "# Rules\n\nDo not hand-edit migrations.\n\nBackground: an app.\n",
    );
    repo.write("drizzle/0001_init.sql", "create table a;\n");
    repo.commit();
    let config = repo.temp_dir().join("claude");
    let home = repo.temp_dir().join("home");
    let extract = |claude: &std::path::Path| {
        common::pragma()
            .args(["extract", "--json"])
            .env("PRAGMA_HOME", &home)
            .env("PRAGMA_CLAUDE_BIN", claude)
            .current_dir(&repo.root)
            .output()
            .unwrap()
    };
    assert!(
        extract(&common::canned_claude(&repo.temp_dir().join("fake")))
            .status
            .success()
    );
    let done = &status(&repo, &config)["extraction"];
    assert_eq!(done["state"], "done");
    assert_eq!(
        done["clauses"],
        json!({ "blocking": 1, "judged": 0, "context": 1 })
    );
    assert!(done["at"].as_u64().unwrap() > 0);
    assert!(done.get("reason").is_none());

    repo.write("AGENTS.md", "# Rules\n\nSomething new.\n");
    let output = extract(&common::logged_out_claude(&repo.temp_dir().join("out")));
    assert_eq!(output.status.code(), Some(1));
    let paused = &status(&repo, &config)["extraction"];
    assert_eq!(paused["state"], "paused");
    assert_eq!(paused["reason"]["kind"], "logged-out");
    assert_eq!(
        paused["clauses"]["blocking"], 1,
        "the last index still stands"
    );

    let output = repo.run(&["status"]);
    assert!(
        stdout(&output).contains("Reading the docs is paused: the claude CLI is not logged in"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn status_reports_a_running_extraction_with_its_progress() {
    let repo = TestRepo::new();
    let state = repo.root.join(".git/pragma");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let _guard = common::hold_extract_lock(&repo, std::process::id(), now);
    std::fs::write(
        state.join("extract-progress.json"),
        json!({ "total": 40, "done": 12, "cached": 10, "clauses_so_far": 9, "started_at": now })
            .to_string(),
    )
    .unwrap();
    let running = &status(&repo, &repo.temp_dir().join("claude"))["extraction"];
    assert_eq!(running["state"], "running");
    assert_eq!(running["at"], now);
    assert_eq!(running["progress"]["done"], 12);
    assert_eq!(running["progress"]["total"], 40);
    let output = repo.run(&["status"]);
    assert!(stdout(&output).contains("Reading the docs: 12 of 40 paragraphs."));
}

#[cfg(unix)]
#[test]
fn status_reports_a_pending_lock_update_until_pragma_lock_runs() {
    let repo = TestRepo::new();
    repo.write(
        "AGENTS.md",
        "# Rules\n\nDo not hand-edit migrations.\n\nBackground: an app.\n",
    );
    repo.write("drizzle/0001_init.sql", "create table a;\n");
    repo.lock(json!([]));
    repo.commit();
    let config = repo.temp_dir().join("claude");
    let pending = || {
        let value = status(&repo, &config);
        (
            value["lock_pending"].clone(),
            value["lock_unsettled"].clone(),
        )
    };
    assert_eq!(pending(), (json!(false), json!(0)), "no index yet");
    let claude = common::canned_claude(&repo.temp_dir().join("fake"));
    let extract = || {
        let output = common::pragma()
            .args(["extract", "--json"])
            .env("PRAGMA_HOME", repo.temp_dir().join("home"))
            .env("PRAGMA_CLAUDE_BIN", &claude)
            .current_dir(&repo.root)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", common::stderr(&output));
    };
    extract();
    assert_eq!(pending(), (json!(true), json!(2)));
    let human = stdout(&repo.run(&["status"]));
    assert!(human.contains("The lock is behind the docs"), "{human}");

    assert!(repo.run(&["lock"]).status.success());
    assert_eq!(pending(), (json!(false), json!(0)));
    let human = stdout(&repo.run(&["status"]));
    assert!(!human.contains("pragma lock"), "{human}");

    // An edit that changes no blocking clause only needs settling: no rule change pending.
    repo.write(
        "AGENTS.md",
        "# Rules\n\nDo not hand-edit migrations.\n\nBackground: a desktop app.\n",
    );
    extract();
    assert_eq!(pending(), (json!(false), json!(1)));
    let human = stdout(&repo.run(&["status"]));
    assert!(!human.contains("behind the docs"), "{human}");
    assert!(human.contains("1 doc paragraphs"), "{human}");
}
