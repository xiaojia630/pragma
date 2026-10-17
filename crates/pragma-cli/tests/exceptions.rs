mod common;

use common::{audit_repo, drift_repo};
use serde_json::Value;

#[test]
fn drift_findings_can_be_marked_intended() {
    let repo = drift_repo();
    assert!(repo.run(&["init"]).status.success());
    let report: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    let id = report["findings"][1]["id"].as_str().unwrap().to_string();
    let added = repo.run(&["exception", "add", &id, "--reason", "intended"]);
    assert!(added.status.success(), "{}", common::stderr(&added));
    let report: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    assert_eq!(report["findings"].as_array().unwrap().len(), 1);
    assert!(
        repo.read(".pragma/overrides.md")
            .contains("reference: src/renderer/lib/ipc.ts")
    );
}

#[test]
fn exceptions_suppress_findings_everywhere_and_go_stale_when_code_changes() {
    let repo = audit_repo();
    let report: Value = serde_json::from_slice(&repo.run(&["audit", "--json"]).stdout).unwrap();
    let codegen = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|found| found["file"] == "src/codegen.ts")
        .unwrap();
    let id = codegen["id"].as_str().unwrap();
    let added = repo.run(&[
        "exception",
        "add",
        id,
        "--reason",
        "deliberate",
        "--note",
        "CommonJS-only module",
    ]);
    assert!(added.status.success(), "{}", common::stderr(&added));
    let report: Value = serde_json::from_slice(&repo.run(&["audit", "--json"]).stdout).unwrap();
    assert_eq!(report["counts"]["no-require"], 1);
    assert!(
        repo.read(".pragma/overrides.md")
            .contains("reason: deliberate")
    );
    assert!(
        repo.read(".git/pragma/feedback.jsonl")
            .contains("CommonJS-only module")
    );
    repo.write("src/codegen.ts", "export {};\n");
    let report: Value = serde_json::from_slice(&repo.run(&["audit", "--json"]).stdout).unwrap();
    assert_eq!(report["stale_exceptions"].as_array().unwrap().len(), 1);
}

#[test]
fn intended_is_only_for_drift_and_unknown_ids_fail() {
    let repo = audit_repo();
    let report: Value = serde_json::from_slice(&repo.run(&["audit", "--json"]).stdout).unwrap();
    let id = report["findings"][0]["id"].as_str().unwrap();
    assert!(
        !repo
            .run(&["exception", "add", id, "--reason", "intended"])
            .status
            .success()
    );
    let unknown = repo.run(&[
        "exception",
        "add",
        "0000000000000000",
        "--reason",
        "deliberate",
    ]);
    assert!(common::stderr(&unknown).contains("run one first"));
}

#[test]
fn exception_add_reports_json() {
    let repo = audit_repo();
    let report: Value = serde_json::from_slice(&repo.run(&["audit", "--json"]).stdout).unwrap();
    let id = report["findings"][0]["id"].as_str().unwrap().to_string();
    let args = ["exception", "add", &id, "--reason", "deliberate", "--json"];
    let first: Value = serde_json::from_slice(&repo.run(&args).stdout).unwrap();
    assert_eq!(first["recorded"], true);
    assert_eq!(first["exception"]["kind"], "violation");
    let again: Value = serde_json::from_slice(&repo.run(&args).stdout).unwrap();
    assert_eq!(again["recorded"], false);
}
