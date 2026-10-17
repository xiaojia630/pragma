mod common;

use common::{TestRepo, stdout};
use serde_json::{Value, json};

fn json_of(output: &std::process::Output) -> Value {
    assert!(output.status.success(), "{}", common::stderr(output));
    serde_json::from_str(&stdout(output)).unwrap()
}

#[test]
fn config_show_reads_the_globs_and_defaults_without_pragma() {
    let repo = TestRepo::new();
    assert_eq!(
        json_of(&repo.run(&["config", "show", "--json"])),
        json!({ "docs": { "include": [], "exclude": [] } })
    );
    repo.write(
        ".pragma/config.md",
        "---\ndocs:\n  include: [\"notes/**\"]\n---\n",
    );
    assert_eq!(
        json_of(&repo.run(&["config", "show", "--json"])),
        json!({ "docs": { "include": ["notes/**"], "exclude": [] } })
    );
    assert!(stdout(&repo.run(&["config", "show"])).contains("notes/**"));
}

#[test]
fn config_set_rewrites_only_the_globs_and_records_the_write() {
    let repo = TestRepo::new();
    repo.write(
        ".pragma/config.md",
        "---\nowner: me\ndocs:\n  include: [old]\n  exclude: [keep]\n---\n# Mine\n\nProse.\n",
    );
    let shown = json_of(&repo.run(&[
        "config",
        "set",
        "--docs-include",
        "a/**",
        "--docs-include",
        "b.md",
        "--json",
    ]));
    assert_eq!(
        shown,
        json!({ "docs": { "include": ["a/**", "b.md"], "exclude": ["keep"] } })
    );
    let text = repo.read(".pragma/config.md");
    assert!(text.contains("owner: me"), "{text}");
    assert!(text.ends_with("---\n# Mine\n\nProse.\n"), "{text}");
    assert!(
        repo.read(".git/pragma/writes.json")
            .contains(".pragma/config.md")
    );

    let cleared = json_of(&repo.run(&[
        "config",
        "set",
        "--clear-include",
        "--clear-exclude",
        "--json",
    ]));
    assert_eq!(cleared, json!({ "docs": { "include": [], "exclude": [] } }));
}

#[test]
fn config_set_creates_the_default_config_when_the_file_is_missing() {
    let repo = TestRepo::new();
    repo.write(".pragma/pragma.lock", r#"{"format": 1, "clauses": []}"#);
    let output = repo.run(&["config", "set", "--docs-exclude", "docs/old/**"]);
    assert!(output.status.success(), "{}", common::stderr(&output));
    assert!(repo.read(".pragma/config.md").contains("docs/old/**"));
    assert!(repo.read(".pragma/config.md").contains("# pragma settings"));
}

#[test]
fn config_set_refuses_bad_input_and_worktrees() {
    let repo = TestRepo::new();
    let output = repo.run(&["config", "set", "--docs-include", "a"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(common::stderr(&output).contains("pragma init"));

    repo.write(".pragma/pragma.lock", r#"{"format": 1, "clauses": []}"#);
    repo.commit();
    assert_eq!(repo.run(&["config", "set"]).status.code(), Some(1));
    assert_eq!(
        repo.run(&["config", "set", "--docs-include", "[bad"])
            .status
            .code(),
        Some(1)
    );
    assert_eq!(
        repo.run(&["config", "set", "--docs-include", "a", "--clear-include"])
            .status
            .code(),
        Some(1)
    );

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
        .args(["config", "set", "--docs-include", "a"])
        .current_dir(&task)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(common::stderr(&output).contains("root checkout"));
    assert!(!repo.root.join(".pragma/config.md").exists());
}
