mod common;

use common::{TestRepo, stdout};
use serde_json::{Value, json};

fn docs_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("AGENTS.md", "# Rules\n\nFirst.\n\nSecond.\n");
    repo.write("docs/guide.md", "# Guide\n\nOne paragraph.\n");
    repo.write("docs/old/legacy.md", "# Legacy\n\nOld.\n\nOlder.\n");
    repo.write("notes/extra.md", "# Extra\n\nNotes.\n");
    repo.commit();
    repo
}

fn docs(repo: &TestRepo, args: &[&str]) -> Value {
    let mut all = vec!["docs", "--json"];
    all.extend_from_slice(args);
    let output = repo.run(&all);
    assert!(output.status.success(), "{}", common::stderr(&output));
    serde_json::from_str(&stdout(&output)).unwrap()
}

fn paths(value: &Value) -> Vec<&str> {
    value["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|doc| doc["path"].as_str().unwrap())
        .collect()
}

#[test]
fn docs_lists_discovered_docs_sorted_with_paragraph_counts() {
    let repo = docs_repo();
    let value = docs(&repo, &[]);
    assert_eq!(
        paths(&value),
        [
            "AGENTS.md",
            "README.md",
            "docs/guide.md",
            "docs/old/legacy.md"
        ]
    );
    let agents = &value["docs"][0];
    assert_eq!(agents["paragraphs"], 2);
    assert_eq!(agents["historical"], false);
    assert_eq!(value["docs"][3]["paragraphs"], 2);
    assert_eq!(value["total_paragraphs"], 5);
    assert_eq!(value["excluded"], json!([]));
}

#[test]
fn flags_preview_globs_in_place_of_the_configs() {
    let repo = docs_repo();
    repo.write(
        ".pragma/config.md",
        "---\ndocs:\n  exclude: [\"docs/guide.md\"]\n---\n",
    );
    let configured = docs(&repo, &[]);
    assert!(!paths(&configured).contains(&"docs/guide.md"));
    assert_eq!(configured["excluded"], json!(["docs/guide.md"]));

    let preview = docs(
        &repo,
        &["--include", "notes/**", "--exclude", "docs/old/**"],
    );
    let listed = paths(&preview);
    assert!(listed.contains(&"notes/extra.md"));
    assert!(
        listed.contains(&"docs/guide.md"),
        "config globs are replaced"
    );
    assert!(!listed.contains(&"docs/old/legacy.md"));
    assert_eq!(preview["excluded"], json!(["docs/old/legacy.md"]));
}

#[test]
fn preview_with_no_globs_ignores_the_saved_ones() {
    let repo = docs_repo();
    repo.write(
        ".pragma/config.md",
        "---\ndocs:\n  include: [\"notes/**\"]\n  exclude: [\"docs/guide.md\"]\n---\n",
    );
    let configured = docs(&repo, &[]);
    let saved = paths(&configured);
    assert!(saved.contains(&"notes/extra.md"));
    assert!(!saved.contains(&"docs/guide.md"));

    let preview = docs(&repo, &["--preview"]);
    let listed = paths(&preview);
    assert!(!listed.contains(&"notes/extra.md"), "saved include dropped");
    assert!(listed.contains(&"docs/guide.md"), "saved exclude dropped");
    assert_eq!(preview["excluded"], json!([]));
}

#[test]
fn docs_human_output_counts_and_names_the_biggest_docs() {
    let repo = docs_repo();
    let output = repo.run(&["docs"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("4 docs, 5 paragraphs"), "{text}");
    assert!(text.contains("AGENTS.md"), "{text}");
}

#[test]
fn a_bad_glob_is_an_error() {
    let repo = docs_repo();
    let output = repo.run(&["docs", "--include", "[bad"]);
    assert_eq!(output.status.code(), Some(1));
}
