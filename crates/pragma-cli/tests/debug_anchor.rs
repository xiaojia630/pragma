mod common;

use common::TestRepo;
use serde_json::{Value, json};

#[test]
fn debug_anchor_prints_a_clause_id_and_anchor() {
    let repo = TestRepo::new();
    repo.write(
        "AGENTS.md",
        "# Agents\n\n## Rules\n\n- Never use `require()`.\n- Never re-export.\n",
    );
    let output = repo.run(&["debug", "anchor", "AGENTS.md:6", "--ordinal", "1"]);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["anchor"]["quote"], "- Never re-export.");
    assert_eq!(value["anchor"]["heading_path"], json!(["Agents", "Rules"]));
    assert_eq!(value["anchor"]["lines"], json!([6, 6]));
    assert_eq!(value["id"].as_str().unwrap().len(), 16);
    let with_flag = repo.run(&["debug", "anchor", "AGENTS.md:6", "--ordinal", "1", "--json"]);
    assert!(with_flag.status.success());
    let flagged: Value = serde_json::from_slice(&with_flag.stdout).unwrap();
    assert_eq!(flagged, value);
}

#[test]
fn debug_anchor_rejects_a_line_with_no_paragraph() {
    let repo = TestRepo::new();
    repo.write("AGENTS.md", "# Agents\n\nOne paragraph.\n");
    let output = repo.run(&["debug", "anchor", "AGENTS.md:1"]);
    assert!(!output.status.success());
    assert!(common::stderr(&output).contains("no paragraph at AGENTS.md:1"));
}
