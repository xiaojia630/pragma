#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};

use common::{TestRepo, canned_claude, fake_calls, logged_out_claude, stderr, stdout};
use serde_json::{Value, json};

struct Setup {
    repo: TestRepo,
    home: PathBuf,
    fake_dir: PathBuf,
}

fn setup() -> Setup {
    let repo = TestRepo::new();
    repo.write(
        "AGENTS.md",
        "# Rules\n\nDo not hand-edit migrations.\n\nNever require() modules.\n\n\
         Background: the desktop app.\n\nSee the [guide](docs/guide.md).\n\nProtect everywhere.\n",
    );
    repo.write("docs/guide.md", "# Guide\n\nDo not hand-edit migrations.\n");
    // The protected-paths clause covers a real file, so it stays blocking (R8).
    repo.write("drizzle/0001_init.sql", "create table a;\n");
    repo.write(
        "docs/adr/0001-old.md",
        "# ADR\n\nNever require() modules here.\n",
    );
    repo.commit();
    let home = repo.temp_dir().join("home");
    let fake_dir = repo.temp_dir().join("fake");
    Setup {
        repo,
        home,
        fake_dir,
    }
}

impl Setup {
    fn extract(&self, args: &[&str], claude: &Path) -> std::process::Output {
        common::pragma()
            .arg("extract")
            .args(args)
            .env("PRAGMA_HOME", &self.home)
            .env("PRAGMA_CLAUDE_BIN", claude)
            .current_dir(&self.repo.root)
            .output()
            .unwrap()
    }

    fn extract_json(&self, args: &[&str], claude: &Path) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let output = self.extract(&all, claude);
        assert!(output.status.success(), "{}", stderr(&output));
        serde_json::from_str(&stdout(&output)).unwrap()
    }

    fn state(&self, name: &str) -> PathBuf {
        self.repo.root.join(".git/pragma").join(name)
    }

    fn index(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.state("index.json")).unwrap()).unwrap()
    }
}

#[test]
fn extract_writes_the_index_and_reports_counts() {
    let setup = setup();
    let claude = canned_claude(&setup.fake_dir);
    let value = setup.extract_json(&[], &claude);
    // AGENTS.md has 5 paragraphs, docs/guide.md repeats one of them, README.md has none and
    // the ADR is historical.
    assert_eq!(value["docs"], 3, "{value}");
    assert_eq!(value["paragraphs"], 5);
    assert_eq!(value["cached"], 0);
    assert_eq!(value["model_calls"], 1);
    assert_eq!(
        value["clauses"],
        json!({ "blocking": 1, "judged": 2, "context": 1 })
    );
    assert_eq!(value["dropped_checks"].as_array().unwrap().len(), 1);
    assert!(
        value["dropped_checks"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("needs a scope")
    );
    assert_eq!(value["errors"], json!([]));
    assert_eq!(fake_calls(&setup.fake_dir), 1);

    let index = setup.index();
    assert_eq!(index["format"], 3);
    // The guide's copy of the migrations paragraph names the doc that answered for it.
    let guide = index["paragraphs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|para| para["path"] == "docs/guide.md")
        .unwrap();
    assert_eq!(guide["same_as"], "AGENTS.md");
    assert_eq!(index["model"], "haiku");
    let clauses = index["clauses"].as_array().unwrap();
    let migrations = &clauses[0];
    assert_eq!(migrations["rung"], "blocking");
    assert_eq!(migrations["anchor"]["path"], "AGENTS.md");
    assert_eq!(migrations["anchor"]["lines"], json!([3, 3]));
    assert_eq!(migrations["anchor"]["heading_path"], json!(["Rules"]));
    assert_eq!(
        migrations["check"],
        json!({ "kind": "library", "rule": "protected-paths" })
    );
    assert_eq!(migrations["scope"], json!(["drizzle/**"]));
    assert_eq!(migrations["fix"], "Run the generator.");
    assert_eq!(migrations["origin"], "repo");
    // docs/guide.md's copy of the paragraph has the same scope basis, so it collapses.
    assert_eq!(
        clauses
            .iter()
            .filter(|clause| clause["text"] == "Do not hand-edit migrations.")
            .count(),
        1
    );
    assert!(migrations.get("dropped_reason").is_none());
    let everything = clauses
        .iter()
        .find(|clause| clause["text"] == "Protect everything.")
        .unwrap();
    assert!(
        everything["dropped_reason"]
            .as_str()
            .unwrap()
            .contains("needs a scope")
    );
    let require = &clauses[1];
    assert_eq!(require["rung"], "judged");
    assert_eq!(require["check"]["kind"], "pattern");
    assert!(
        clauses
            .iter()
            .all(|clause| clause["anchor"]["path"] != "docs/adr/0001-old.md")
    );

    let progress: Value = serde_json::from_str(
        &std::fs::read_to_string(setup.state("extract-progress.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(progress["total"], 5);
    assert_eq!(progress["done"], 5);
    assert_eq!(progress["cached"], 0);
    assert_eq!(progress["clauses_so_far"], 4);
    assert!(progress["started_at"].as_u64().unwrap() > 0);
    assert!(progress["finished_at"].as_u64().unwrap() >= progress["started_at"].as_u64().unwrap());
    assert!(!setup.repo.root.join(".git/pragma/extract.lock").exists());
}

#[test]
fn a_second_run_reads_everything_from_the_cache() {
    let setup = setup();
    let claude = canned_claude(&setup.fake_dir);
    setup.extract_json(&[], &claude);
    let first = setup.index();
    let value = setup.extract_json(&[], &claude);
    assert_eq!(value["cached"], 5);
    assert_eq!(value["model_calls"], 0);
    assert_eq!(fake_calls(&setup.fake_dir), 1);
    assert_eq!(setup.index()["clauses"], first["clauses"]);
    // With everything cached, a missing CLI does not matter.
    let value = setup.extract_json(&[], Path::new("/nonexistent/claude"));
    assert_eq!(value["cached"], 5);
}

#[test]
fn estimate_counts_without_calling_the_model() {
    let setup = setup();
    let claude = canned_claude(&setup.fake_dir);
    let value = setup.extract_json(&["--estimate"], &claude);
    assert_eq!(
        value,
        json!({ "docs": 3, "paragraphs": 5, "uncached_paragraphs": 5, "model_calls": 1 })
    );
    assert_eq!(fake_calls(&setup.fake_dir), 0);
    assert!(!setup.state("index.json").exists());
    setup.extract_json(&[], &claude);
    let value = setup.extract_json(&["--estimate"], &claude);
    assert_eq!(value["uncached_paragraphs"], 0);
    assert_eq!(value["model_calls"], 0);
}

#[test]
fn a_logged_out_model_exits_1_with_the_reason_and_leaves_the_index() {
    let setup = setup();
    let claude = canned_claude(&setup.fake_dir);
    setup.extract_json(&[], &claude);
    let before = std::fs::read_to_string(setup.state("index.json")).unwrap();
    setup
        .repo
        .write("AGENTS.md", "# Rules\n\nA new paragraph.\n");
    let logged_out = logged_out_claude(&setup.repo.temp_dir().join("logged-out"));
    let output = setup.extract(&["--json"], &logged_out);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "logged-out");
    assert!(
        stderr(&output).contains("not logged in"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        std::fs::read_to_string(setup.state("index.json")).unwrap(),
        before
    );
    let progress: Value = serde_json::from_str(
        &std::fs::read_to_string(setup.state("extract-progress.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(progress["error"]["kind"], "logged-out");
}

#[test]
fn a_missing_model_exits_1_and_writes_no_index() {
    let setup = setup();
    let output = setup.extract(&["--json"], Path::new("/nonexistent/claude"));
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(value["error"]["kind"], "missing");
    assert!(!setup.state("index.json").exists());
}

#[test]
fn one_run_per_repo() {
    let setup = setup();
    let claude = canned_claude(&setup.fake_dir);
    let lock = setup.repo.root.join(".git/pragma/extract.lock");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    // A live holder: this test process.
    let guard = common::hold_extract_lock(&setup.repo, std::process::id(), now);
    let output = setup.extract(&["--json"], &claude);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("already running"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fake_calls(&setup.fake_dir), 0);
    // A holder that is gone (its record left behind, its OS lock released) holds nothing.
    drop(guard);
    assert!(lock.exists());
    setup.extract_json(&[], &claude);
    assert!(!lock.exists());
}

#[test]
fn extract_human_output() {
    let setup = setup();
    let output = setup.extract(&[], &canned_claude(&setup.fake_dir));
    assert!(output.status.success(), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("Clauses: 1 blocking, 2 judged, 1 context."),
        "{text}"
    );
}
