mod common;

use common::{TestRepo, drift_repo};
use serde_json::Value;

#[test]
fn drift_reports_missing_paths_and_suggests_moves() {
    let repo = drift_repo();
    let report: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2, "{report}");
    assert_eq!(
        findings[0]["reference"],
        "src/tasks/browser/task-selectors.ts"
    );
    assert_eq!(findings[0]["suggestion"], "src/tasks/api/task-selectors.ts");
    assert_eq!(findings[1]["reference"], "src/renderer/lib/ipc.ts");
    assert!(findings[1].get("suggestion").is_none());
    assert!(
        repo.read(".git/pragma/drift.json")
            .contains("task-selectors.ts")
    );
}

#[test]
fn drift_diff_applies_and_leaves_unfixable_findings() {
    let repo = drift_repo();
    let diff = repo.run(&["drift", "--diff"]);
    std::fs::write(repo.root.join("fix.patch"), &diff.stdout).unwrap();
    repo.git(&["apply", "fix.patch"]);
    std::fs::remove_file(repo.root.join("fix.patch")).unwrap();
    assert!(
        repo.read("CONTRIBUTING.md")
            .contains("`src/tasks/api/task-selectors.ts`")
    );
    let report: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    assert_eq!(report["findings"].as_array().unwrap().len(), 1);
}

#[test]
fn drift_works_in_a_repo_without_pragma() {
    let repo = drift_repo();
    let output = repo.run(&["drift"]);
    assert!(output.status.success());
    assert!(
        common::stdout(&output).contains("2 references point at things this repo does not have")
    );
}

fn finding_id(report: &Value, reference: &str) -> String {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["reference"] == reference)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn drift_apply_fixes_the_named_findings_and_skips_the_rest() {
    let repo = drift_repo();
    let before: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    let fixable = finding_id(&before, "src/tasks/browser/task-selectors.ts");
    let unfixable = finding_id(&before, "src/renderer/lib/ipc.ts");
    let output = repo.run(&["drift", "apply", &fixable, &unfixable, "nope", "--json"]);
    assert!(output.status.success(), "{}", common::stderr(&output));
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["applied"], serde_json::json!([fixable]));
    assert_eq!(result["skipped"], serde_json::json!([unfixable, "nope"]));
    let doc = repo.read("CONTRIBUTING.md");
    assert!(doc.contains("`src/tasks/api/task-selectors.ts`"), "{doc}");
    assert!(doc.contains("`src/renderer/lib/ipc.ts`"));
    let saved: Value = serde_json::from_str(&repo.read(".git/pragma/drift.json")).unwrap();
    assert_eq!(saved["findings"].as_array().unwrap().len(), 1);
    assert_eq!(saved["findings"][0]["id"], unfixable);
}

#[test]
fn drift_apply_twice_skips_what_is_already_fixed() {
    let repo = drift_repo();
    let before: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    let fixable = finding_id(&before, "src/tasks/browser/task-selectors.ts");
    assert!(repo.run(&["drift", "apply", &fixable]).status.success());
    let output = repo.run(&["drift", "apply", &fixable, "--json"]);
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["applied"], serde_json::json!([]));
    assert_eq!(result["skipped"], serde_json::json!([fixable]));
    let human = repo.run(&["drift", "apply", &fixable]);
    assert!(common::stdout(&human).contains("Skipped 1"));
}

#[test]
fn drift_sets_apart_paths_that_may_be_urls_and_finds_names_without_their_extension() {
    let repo = TestRepo::new();
    repo.write("design/screens/DS-Foundations.dc.html", "<html></html>\n");
    repo.write(
        "docs/deploy.md",
        "# Deploy\n\nThe worker is served at `/sw.js`. The design is `design/screens/DS-Foundations`. \
         The old setup is in `src/gone.ts`.\n",
    );
    repo.commit();

    let report: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2, "{report}");
    assert_eq!(findings[0]["reference"], "/sw.js");
    assert_eq!(findings[0]["kind"], "maybe-url");
    assert_eq!(findings[0]["confidence"], "low");
    assert_eq!(findings[1]["reference"], "src/gone.ts");
    assert_eq!(findings[1]["kind"], "missing-path");
    assert_eq!(findings[1]["confidence"], "high");

    // The headline counts only the file that is certainly gone; the URL is a group of its own.
    let human = common::stdout(&repo.run(&["drift"]));
    let groups: Vec<&str> = human.split("\n\n").collect();
    assert!(
        groups[1].starts_with("1 references point at things this repo does not have:"),
        "{human}"
    );
    assert!(
        groups[1].contains("src/gone.ts") && !groups[1].contains("/sw.js"),
        "{human}"
    );
    assert!(
        groups[2]
            .starts_with("1 references start with / and match no file (may be URLs, not files):"),
        "{human}"
    );
    assert!(
        groups[2].contains("/sw.js  no such file") && !groups[2].contains("src/gone.ts"),
        "{human}"
    );
}

#[test]
fn drift_with_only_possible_urls_does_not_say_everything_checked_out() {
    let repo = TestRepo::new();
    repo.write("docs/deploy.md", "The worker is served at `/sw.js`.\n");
    repo.commit();
    let human = common::stdout(&repo.run(&["drift"]));
    assert!(!human.contains("Every reference checked out"), "{human}");
    assert!(
        !human.contains("point at things this repo does not have"),
        "{human}"
    );
    assert!(
        human.contains("1 references start with / and match no file (may be URLs, not files):"),
        "{human}"
    );
}

#[test]
fn a_possible_url_can_be_marked_intended() {
    let repo = TestRepo::new();
    repo.write("docs/deploy.md", "The worker is served at `/sw.js`.\n");
    repo.commit();
    assert!(repo.run(&["init"]).status.success());
    let report: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    let id = report["findings"][0]["id"].as_str().unwrap().to_string();
    let added = repo.run(&["exception", "add", &id, "--reason", "intended"]);
    assert!(added.status.success(), "{}", common::stderr(&added));
    let report: Value = serde_json::from_slice(&repo.run(&["drift", "--json"]).stdout).unwrap();
    assert!(
        report["findings"].as_array().unwrap().is_empty(),
        "{report}"
    );
}
