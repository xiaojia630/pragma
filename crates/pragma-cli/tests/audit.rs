mod common;

use common::{TestRepo, audit_repo, stdout};
use serde_json::Value;

#[test]
fn audit_finds_the_baseline_and_writes_history() {
    let repo = audit_repo();
    let report: Value = serde_json::from_slice(&repo.run(&["audit", "--json"]).stdout).unwrap();
    assert_eq!(report["files_checked"], 2);
    assert_eq!(report["counts"]["no-require"], 2);
    let history = repo.read(".git/pragma/audit-history.jsonl");
    assert_eq!(history.lines().count(), 1);
    let saved = repo.read(".git/pragma/audit.json");
    let human = stdout(&repo.run(&["audit", "--area", "src/codegen.ts"]));
    assert!(human.contains("Not saved"), "{human}");
    assert_eq!(repo.read(".git/pragma/audit.json"), saved);
    assert_eq!(
        repo.read(".git/pragma/audit-history.jsonl").lines().count(),
        1,
        "an --area run leaves the baseline and history alone"
    );
    assert!(human.contains("1 x Never write require($$$A)."), "{human}");
}

#[test]
fn audit_needs_pragma_to_be_set_up() {
    let repo = TestRepo::new();
    let output = repo.run(&["audit"]);
    assert!(!output.status.success());
    assert!(common::stderr(&output).contains("run pragma init"));
}

#[test]
fn audit_reports_package_roots_for_grouping() {
    let repo = audit_repo();
    repo.write("package.json", "{}");
    repo.write("apps/web/package.json", "{}");
    let report: Value = serde_json::from_slice(&repo.run(&["audit", "--json"]).stdout).unwrap();
    assert_eq!(report["package_roots"], serde_json::json!(["", "apps/web"]));
}

/// A repo whose `main` has three `require()` files, and a task worktree at `<temp>/task` that
/// fixes one, adds one to another (uncommitted), adds an untracked one, deletes one and leaves
/// one alone, while `main` changes the one it left alone.
fn task_repo() -> (TestRepo, std::path::PathBuf) {
    let repo = audit_repo();
    repo.write("src/fixed.ts", "const f = require('f');\n");
    repo.write("src/untouched.ts", "const u = require('u');\n");
    repo.write("src/changed.ts", "export const c = 1;\n");
    repo.write("src/gone.ts", "const g = require('g');\n");
    repo.write(".gitattributes", "src/gen.ts linguist-generated\n");
    repo.commit();
    repo.git(&["worktree", "add", "-q", "../task", "-b", "task"]);
    let task = repo.temp_dir().join("task");
    let write = |rel: &str, text: &str| std::fs::write(task.join(rel), text).unwrap();
    write("src/fixed.ts", "import f from 'f';\n");
    std::fs::remove_file(task.join("src/gone.ts")).unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=pragma",
                "-c",
                "user.email=pragma@example.com",
            ])
            .args(args)
            .current_dir(&task)
            .status()
            .unwrap();
        assert!(status.success());
    };
    git(&["commit", "-q", "-am", "fix f"]);
    write("src/changed.ts", "export const c = require('c');\n");
    write("src/new.ts", "const n = require('n');\n");
    write("src/gen.ts", "const g = require('g');\n");
    write("README.md", "# changed\n");
    repo.write("src/untouched.ts", "const u = require('u');\nexport {};\n");
    repo.commit();
    (repo, task)
}

fn audit_in(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    common::pragma()
        .arg("audit")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn changed_from_audits_only_what_the_branch_changed_and_saves_nothing() {
    let (repo, task) = task_repo();
    let output = audit_in(&task, &["--changed-from", "main", "--json"]);
    assert!(output.status.success(), "{}", common::stderr(&output));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report["changed_files"],
        serde_json::json!(["src/changed.ts", "src/fixed.ts", "src/new.ts"]),
        "deleted, generated, uncovered and unchanged files are left out"
    );
    assert_eq!(report["files_checked"], 3);
    let files: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|found| found["file"].as_str().unwrap())
        .collect();
    assert_eq!(files, vec!["src/changed.ts", "src/new.ts"]);
    assert!(report["package_roots"].is_array() && report["stale_exceptions"].is_array());
    let state = repo.root.join(".git/worktrees/task/pragma");
    assert!(!state.join("audit.json").exists() && !state.join("audit-history.jsonl").exists());
    let human = common::stdout(&audit_in(&task, &["--changed-from", "main"]));
    assert!(
        human.contains("Not saved: this result covers only the files changed since main."),
        "{human}"
    );
    let full: Value = serde_json::from_slice(&audit_in(&task, &["--json"]).stdout).unwrap();
    assert!(
        full.get("changed_files").is_none(),
        "absent for a full audit"
    );
}

#[test]
fn changed_from_applies_exceptions() {
    let (repo, task) = task_repo();
    repo.write(
        ".pragma/overrides.md",
        "---\nexceptions:\n- kind: violation\n  clause: no-require\n  para_hash: p10\n  \
         file: src/new.ts\n  matched: require('n')\n  reason: deliberate\n---\n",
    );
    let report: Value =
        serde_json::from_slice(&audit_in(&task, &["--changed-from", "main", "--json"]).stdout)
            .unwrap();
    assert_eq!(report["counts"]["no-require"], 1);
    assert_eq!(report["findings"][0]["file"], "src/changed.ts");
}

#[test]
fn changed_from_refuses_an_area_and_names_a_bad_ref() {
    let (_repo, task) = task_repo();
    let both = audit_in(&task, &["--area", "src", "--changed-from", "main"]);
    assert!(!both.status.success());
    assert!(
        common::stderr(&both).contains("cannot be used with"),
        "{}",
        common::stderr(&both)
    );
    let bad = audit_in(&task, &["--changed-from", "no-such-branch"]);
    assert!(!bad.status.success());
    assert!(
        common::stderr(&bad).contains("no-such-branch"),
        "{}",
        common::stderr(&bad)
    );
}

/// The overrides file with one `no-require` exception per `(file, matched)`.
fn exceptions(repo: &TestRepo, which: &[(&str, &str)]) {
    let list: String = which
        .iter()
        .map(|(file, matched)| {
            format!(
                "- kind: violation\n  clause: no-require\n  para_hash: p10\n  file: {file}\n  \
                 matched: {matched}\n  reason: deliberate\n"
            )
        })
        .collect();
    repo.write(
        ".pragma/overrides.md",
        &format!("---\nexceptions:\n{list}---\n"),
    );
}

#[test]
fn changed_from_reports_an_exception_stale_only_for_a_file_it_looked_at() {
    let (repo, task) = task_repo();
    exceptions(
        &repo,
        &[
            ("src/untouched.ts", "require('u')"),
            ("src/fixed.ts", "require('f')"),
            ("src/new.ts", "require('n')"),
        ],
    );
    let report: Value =
        serde_json::from_slice(&audit_in(&task, &["--changed-from", "main", "--json"]).stdout)
            .unwrap();
    let stale: Vec<&str> = report["stale_exceptions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|exception| exception["file"].as_str().unwrap())
        .collect();
    assert_eq!(
        stale,
        vec!["src/fixed.ts"],
        "the unchanged file was not looked at; the changed, fixed one was; the live one is not stale"
    );
}

#[cfg(unix)]
#[test]
fn changed_from_refuses_an_option_as_a_ref_without_running_git() {
    let (_repo, task) = task_repo();
    let dir = tempfile::tempdir().unwrap();
    let mut command = common::pragma();
    common::log_git_calls(&mut command, dir.path());
    let output = command
        .args(["audit", "--changed-from=-x"])
        .current_dir(&task)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        common::stderr(&output).contains("-x is not a ref"),
        "{}",
        common::stderr(&output)
    );
    assert_eq!(common::logged_git_calls(dir.path()), Vec::<String>::new());
}

#[cfg(unix)]
#[test]
fn changed_from_runs_git_without_optional_locks() {
    let (_repo, task) = task_repo();
    let dir = tempfile::tempdir().unwrap();
    let mut command = common::pragma();
    common::log_git_calls(&mut command, dir.path());
    let output = command
        .args(["audit", "--changed-from", "main", "--json"])
        .current_dir(&task)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", common::stderr(&output));
    let calls = common::logged_git_calls(dir.path());
    assert!(calls.len() >= 4, "{calls:?}");
    assert!(
        calls
            .iter()
            .all(|call| call.starts_with("GIT_OPTIONAL_LOCKS=0 ")),
        "{calls:?}"
    );
}
