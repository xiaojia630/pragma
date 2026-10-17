mod common;

use common::{TestRepo, pragma};
use serde_json::{Value, json};

#[test]
fn init_creates_pragma_dir_once() {
    let repo = TestRepo::new();
    let first: Value = serde_json::from_slice(&repo.run(&["init", "--json"]).stdout).unwrap();
    assert_eq!(
        first["created"],
        json!([
            ".pragma/README.md",
            ".pragma/config.md",
            ".pragma/pragma.lock"
        ])
    );
    let lock: Value = serde_json::from_str(&repo.read(".pragma/pragma.lock")).unwrap();
    assert_eq!(lock, json!({ "format": 1, "clauses": [] }));
    let second: Value = serde_json::from_slice(&repo.run(&["init", "--json"]).stdout).unwrap();
    assert_eq!(second["created"], json!([]));
    assert!(
        repo.read(".git/pragma/writes.json")
            .contains(".pragma/pragma.lock")
    );
}

#[test]
fn init_in_a_worktree_writes_to_the_root_checkout() {
    let repo = TestRepo::new();
    repo.git(&["worktree", "add", "-q", "../task", "-b", "task"]);
    let task = repo.temp_dir().join("task");
    let output = pragma().arg("init").current_dir(&task).output().unwrap();
    assert!(output.status.success());
    assert!(repo.root.join(".pragma/pragma.lock").exists());
    assert!(!task.join(".pragma").exists());
}

#[test]
fn version_reports_the_lock_format() {
    let output = pragma().args(["version", "--json"]).output().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["lock_format"], 1);
}
