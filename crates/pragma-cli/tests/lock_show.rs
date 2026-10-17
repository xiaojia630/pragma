mod common;

use common::{audit_repo, pattern_clause, protected_clause, stdout};
use serde_json::{Value, json};

#[test]
fn shows_each_clause_with_its_fix_source_and_rule() {
    let repo = common::TestRepo::new();
    repo.lock(json!([
        pattern_clause(
            "c1",
            "require($$$A)",
            &["**/*.ts"],
            "require('a');",
            "import a from 'a';"
        ),
        protected_clause("c2", &["drizzle/**"]),
    ]));
    let output = repo.run(&["lock", "show", "--json"]);
    assert!(output.status.success(), "{}", common::stderr(&output));
    let shown: Value = serde_json::from_slice(&output.stdout).unwrap();
    let clauses = shown["clauses"].as_array().unwrap();
    assert_eq!(clauses.len(), 2);
    assert_eq!(clauses[0]["id"], "c1");
    assert_eq!(clauses[0]["text"], "Never write require($$$A).");
    assert_eq!(clauses[0]["fix"], "Use an explicit null check.");
    assert_eq!(clauses[0]["rule"], "forbidden-pattern");
    assert_eq!(clauses[0]["scope"], json!(["**/*.ts"]));
    assert_eq!(clauses[0]["source"]["path"], "AGENTS.md");
    assert_eq!(clauses[0]["source"]["lines"], json!([10, 10]));
    assert_eq!(clauses[1]["rule"], "protected-paths");
    assert_eq!(shown["exceptions"], json!([]));

    let human = repo.run(&["lock", "show"]);
    assert!(stdout(&human).contains("c1  Never write require($$$A). (AGENTS.md:10)"));
}

#[test]
fn lists_recorded_exceptions() {
    let repo = audit_repo();
    let report: Value = serde_json::from_slice(&repo.run(&["audit", "--json"]).stdout).unwrap();
    let id = report["findings"][0]["id"].as_str().unwrap();
    let added = repo.run(&["exception", "add", id, "--reason", "check-is-wrong"]);
    assert!(added.status.success(), "{}", common::stderr(&added));
    let shown: Value =
        serde_json::from_slice(&repo.run(&["lock", "show", "--json"]).stdout).unwrap();
    let exceptions = shown["exceptions"].as_array().unwrap();
    assert_eq!(exceptions.len(), 1);
    assert_eq!(exceptions[0]["kind"], "violation");
    assert_eq!(exceptions[0]["clause"], "no-require");
    assert_eq!(exceptions[0]["reason"], "check-is-wrong");
}

#[test]
fn needs_a_pragma_dir() {
    let repo = common::TestRepo::new();
    let output = repo.run(&["lock", "show", "--json"]);
    assert!(!output.status.success());
    assert!(common::stderr(&output).contains("run pragma init"));
}
