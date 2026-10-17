mod common;

use std::process::Output;

use common::{TestRepo, stderr, stdout};
use serde_json::{Value, json};

/// Runs `pragma commit` with a git identity, since the command runs plain `git commit`.
fn commit(dir: &std::path::Path, args: &[&str]) -> Output {
    common::pragma()
        .arg("commit")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "pragma")
        .env("GIT_AUTHOR_EMAIL", "pragma@example.com")
        .env("GIT_COMMITTER_NAME", "pragma")
        .env("GIT_COMMITTER_EMAIL", "pragma@example.com")
        .output()
        .unwrap()
}

fn git_out(repo: &TestRepo, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(&repo.root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn commit_takes_only_pragma_files_and_leaves_other_staged_changes() {
    let repo = TestRepo::new();
    repo.write(".pragma/pragma.lock", r#"{"format": 1, "clauses": []}"#);
    repo.write(".pragma/config.md", "---\ndocs: {}\n---\n");
    repo.write("other.txt", "staged by someone else\n");
    repo.git(&["add", "other.txt"]);

    let output = commit(
        &repo.root,
        &["--message", "chore(pragma): turn on", "--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let result: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(result["committed"], true);
    assert_eq!(
        result["files"],
        json!([".pragma/config.md", ".pragma/pragma.lock"])
    );
    assert_eq!(result["sha"], git_out(&repo, &["rev-parse", "HEAD"]));
    assert_eq!(
        git_out(&repo, &["log", "-1", "--format=%s"]),
        "chore(pragma): turn on"
    );
    let committed = git_out(&repo, &["show", "--name-only", "--format=", "HEAD"]);
    assert!(!committed.contains("other.txt"), "{committed}");
    assert_eq!(
        git_out(&repo, &["diff", "--cached", "--name-only"]),
        "other.txt"
    );
}

#[test]
fn commit_with_nothing_changed_reports_not_committed() {
    let repo = TestRepo::new();
    repo.write(".pragma/pragma.lock", r#"{"format": 1, "clauses": []}"#);
    repo.commit();
    let head = git_out(&repo, &["rev-parse", "HEAD"]);
    let output = commit(&repo.root, &["--message", "again", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let result: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(
        result,
        json!({ "committed": false, "sha": null, "files": [] })
    );
    assert_eq!(git_out(&repo, &["rev-parse", "HEAD"]), head);
    let human = commit(&repo.root, &["--message", "again"]);
    assert!(stdout(&human).contains("Nothing to commit"));
}

#[test]
fn commit_refuses_worktrees_and_repos_without_pragma() {
    let repo = TestRepo::new();
    let output = commit(&repo.root, &["--message", "x"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("pragma init"));

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
    let output = commit(&task, &["--message", "x"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("root checkout"));
}
