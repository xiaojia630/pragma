mod common;

use common::{TestRepo, stderr, stdout};
use serde_json::{Value, json};

fn lock_json(repo: &TestRepo) -> Value {
    let output = repo.run(&["lock", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_str(&stdout(&output)).unwrap()
}

#[test]
fn lock_refuses_without_an_index_or_outside_the_root_checkout() {
    let repo = TestRepo::new();
    let output = repo.run(&["lock"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("pragma init"),
        "{}",
        stderr(&output)
    );

    repo.lock(json!([]));
    repo.commit();
    let output = repo.run(&["lock", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("run pragma extract first"),
        "{}",
        stderr(&output)
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
        .args(["lock", "--json"])
        .current_dir(&task)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("root checkout"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn lock_subcommands_still_work() {
    let repo = TestRepo::new();
    repo.lock(json!([common::protected_clause("p", &["drizzle/**"])]));
    assert!(repo.run(&["lock", "check"]).status.success());
    assert!(stdout(&repo.run(&["lock", "show"])).contains("Do not hand-edit migrations."));
}

#[cfg(unix)]
mod with_extraction {
    use super::*;
    use common::canned_claude;
    use std::path::PathBuf;

    struct Setup {
        repo: TestRepo,
        home: PathBuf,
        claude: PathBuf,
    }

    fn setup() -> Setup {
        let repo = TestRepo::new();
        repo.write(
            "AGENTS.md",
            "# Rules\n\nDo not hand-edit migrations.\n\nBackground: an app.\n",
        );
        repo.write("drizzle/0001_init.sql", "create table a;\n");
        repo.write(".pragma/config.md", "---\ndocs: {}\n---\n");
        // A hand-written clause, anchored to a paragraph no doc has: never touched.
        repo.lock(json!([common::pattern_clause(
            "hand",
            "require($$$A)",
            &["**/*.ts"],
            "require('a');",
            "import a from 'a';"
        )]));
        repo.commit();
        let home = repo.temp_dir().join("home");
        let claude = canned_claude(&repo.temp_dir().join("fake"));
        Setup { repo, home, claude }
    }

    impl Setup {
        fn extract(&self) {
            let output = common::pragma()
                .args(["extract", "--json"])
                .env("PRAGMA_HOME", &self.home)
                .env("PRAGMA_CLAUDE_BIN", &self.claude)
                .current_dir(&self.repo.root)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", stderr(&output));
        }

        fn locked(&self) -> Value {
            serde_json::from_str(&self.repo.read(".pragma/pragma.lock")).unwrap()
        }

        fn ids(&self) -> Vec<String> {
            self.locked()["clauses"]
                .as_array()
                .unwrap()
                .iter()
                .map(|clause| clause["id"].as_str().unwrap().to_string())
                .collect()
        }
    }

    #[test]
    fn lock_takes_blocking_clauses_and_an_unchanged_lock_stays_byte_identical() {
        let setup = setup();
        setup.extract();
        let first = lock_json(&setup.repo);
        assert_eq!(first["kept"], 1, "{first}");
        // Both AGENTS.md paragraphs are settled, the one with no blocking clause too.
        assert_eq!(first["settled"], 2);
        assert_eq!(first["removed"], json!([]));
        assert_eq!(first["changed"], json!([]));
        let added = first["added"].as_array().unwrap();
        assert_eq!(added.len(), 1);
        let locked = setup.locked();
        let migrations = locked["clauses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|clause| clause["id"] == added[0])
            .unwrap();
        assert_eq!(migrations["origin"], "extracted");
        assert_eq!(migrations["scope"], json!(["drizzle/**"]));
        assert_eq!(migrations["anchor"]["path"], "AGENTS.md");
        assert_eq!(locked["paragraphs"].as_array().unwrap().len(), 2);
        // One short string per paragraph: "<doc>#<para_hash>".
        assert_eq!(
            locked["paragraphs"][0],
            format!(
                "AGENTS.md#{}",
                migrations["anchor"]["para_hash"].as_str().unwrap()
            )
        );
        let text = setup.repo.read(".pragma/pragma.lock");
        assert!(
            text.lines().any(|line| line.trim()
                == format!("\"{}\",", locked["paragraphs"][0].as_str().unwrap())
                || line.trim() == format!("\"{}\"", locked["paragraphs"][0].as_str().unwrap())),
            "{text}"
        );
        let hand = locked["clauses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|clause| clause["id"] == "hand")
            .unwrap();
        assert!(hand.get("origin").is_none());
        // The written lock is sound and sorted, and the hook and audit read it.
        assert!(setup.repo.run(&["lock", "check"]).status.success());
        let edit = setup
            .repo
            .write_input("drizzle/0001_init.sql", "drop table a;\n");
        let blocked = setup.repo.hook(edit, &[]);
        assert_eq!(blocked.status.code(), Some(2), "{}", stderr(&blocked));
        assert!(stderr(&blocked).contains("Do not hand-edit migrations."));
        assert!(setup.repo.run(&["audit", "--json"]).status.success());
        let mut sorted = setup.ids();
        sorted.sort();
        assert_eq!(setup.ids(), sorted);
        let writes: Value = serde_json::from_str(
            &std::fs::read_to_string(setup.repo.root.join(".git/pragma/writes.json")).unwrap(),
        )
        .unwrap();
        assert!(writes.get(".pragma/pragma.lock").is_some());

        // Nothing changed: the same bytes, down to the mtime.
        let bytes = setup.repo.read(".pragma/pragma.lock");
        let modified = std::fs::metadata(setup.repo.root.join(".pragma/pragma.lock"))
            .unwrap()
            .modified()
            .unwrap();
        setup.extract();
        let again = lock_json(&setup.repo);
        assert_eq!(
            again,
            json!({ "added": [], "removed": [], "changed": [], "kept": 2, "settled": 0 })
        );
        assert_eq!(setup.repo.read(".pragma/pragma.lock"), bytes);
        assert_eq!(
            std::fs::metadata(setup.repo.root.join(".pragma/pragma.lock"))
                .unwrap()
                .modified()
                .unwrap(),
            modified
        );
        let human = stdout(&setup.repo.run(&["lock"]));
        assert!(human.contains("up to date"), "{human}");
    }

    #[test]
    fn a_changed_paragraph_replaces_its_clauses_and_a_gone_one_drops_them() {
        let setup = setup();
        setup.extract();
        let first = lock_json(&setup.repo);
        let old = first["added"][0].as_str().unwrap().to_string();

        setup.repo.write(
            "AGENTS.md",
            "# Rules\n\nNever hand-edit the migrations.\n\nBackground: an app.\n",
        );
        setup.extract();
        let changed = lock_json(&setup.repo);
        assert_eq!(changed["changed"], json!([old]), "{changed}");
        assert_eq!(changed["removed"], json!([]));
        assert_eq!(changed["kept"], 1);
        let new = changed["added"][0].as_str().unwrap().to_string();
        assert_ne!(new, old);
        let mut ids = setup.ids();
        ids.sort();
        let mut expected = vec!["hand".to_string(), new.clone()];
        expected.sort();
        assert_eq!(ids, expected);

        setup
            .repo
            .write("AGENTS.md", "# Background\n\nBackground: an app.\n");
        setup.extract();
        let gone = lock_json(&setup.repo);
        assert_eq!(gone["removed"], json!([new]), "{gone}");
        assert_eq!(gone["added"], json!([]));
        assert_eq!(setup.ids(), vec!["hand"]);
        let human = stdout(&setup.repo.run(&["lock"]));
        assert!(human.contains("up to date"), "{human}");
    }

    #[test]
    fn a_settled_paragraph_takes_no_new_blocking_clause_from_a_later_extraction() {
        let setup = setup();
        setup.extract();
        lock_json(&setup.repo);
        let before = setup.repo.read(".pragma/pragma.lock");

        // Another machine with a cold cache gets a blocking proposal for every paragraph.
        let eager = common::fake_claude(
            &setup.repo.temp_dir().join("eager"),
            r#"out=''
sep=''
while IFS= read -r line; do
  case "$line" in '{"id":"'*) ;; *) continue ;; esac
  id=$(printf '%s' "$line" | sed 's/^{"id":"\([^"]*\)".*/\1/')
  out="$out$sep{\"id\":\"$id\",\"clauses\":[{\"text\":\"Leave the app alone.\",\"rung\":\"blocking\",\"scope\":[\"drizzle/**\"],\"check\":{\"rule\":\"protected-paths\"},\"fix\":\"Do not.\"}]}"
  sep=','
done
printf '{"type":"result","is_error":false,"result":"","structured_output":{"paragraphs":[%s]}}\n' "$out""#,
        );
        let output = common::pragma()
            .args(["extract", "--json"])
            .env("PRAGMA_HOME", setup.repo.temp_dir().join("cold-home"))
            .env("PRAGMA_CLAUDE_BIN", &eager)
            .current_dir(&setup.repo.root)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        let index: Value = serde_json::from_str(
            &std::fs::read_to_string(setup.repo.root.join(".git/pragma/index.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(index["clauses"].as_array().unwrap().len(), 2);
        let status: Value =
            serde_json::from_str(&stdout(&setup.repo.run(&["status", "--json"]))).unwrap();
        assert_eq!(status["lock_pending"], false);
        let again = lock_json(&setup.repo);
        assert_eq!(
            again,
            json!({ "added": [], "removed": [], "changed": [], "kept": 2, "settled": 0 })
        );
        assert_eq!(setup.repo.read(".pragma/pragma.lock"), before);
    }

    impl Setup {
        fn clause_by_id(&self, id: &Value) -> Value {
            self.locked()["clauses"]
                .as_array()
                .unwrap()
                .iter()
                .find(|clause| &clause["id"] == id)
                .cloned()
                .unwrap_or_else(|| panic!("no clause {id} in the lock"))
        }

        fn status(&self) -> Value {
            serde_json::from_str(&stdout(&self.repo.run(&["status", "--json"]))).unwrap()
        }

        fn blocks_a_migration_edit(&self) -> bool {
            let edit = self
                .repo
                .write_input("drizzle/0001_init.sql", "drop table a;\n");
            self.repo.hook(edit, &[]).status.code() == Some(2)
        }
    }

    #[test]
    fn a_rule_still_written_in_another_doc_stays_when_its_first_copy_goes() {
        let setup = setup();
        setup
            .repo
            .write("CLAUDE.md", "# Rules\n\nDo not hand-edit migrations.\n");
        setup.repo.commit();
        setup.extract();
        let first = lock_json(&setup.repo);
        // One copy answers for both, and the other copy is not settled: it has not answered.
        let added = first["added"].as_array().unwrap().clone();
        assert_eq!(added.len(), 1, "{first}");
        assert_eq!(setup.clause_by_id(&added[0])["anchor"]["path"], "AGENTS.md");
        let settled = setup.locked()["paragraphs"].clone();
        assert!(
            settled
                .as_array()
                .unwrap()
                .iter()
                .all(|key| !key.as_str().unwrap().starts_with("CLAUDE.md#")),
            "{settled}"
        );

        setup
            .repo
            .write("AGENTS.md", "# Rules\n\nBackground: an app.\n");
        setup.repo.commit();
        setup.extract();
        let second = lock_json(&setup.repo);
        assert_eq!(second["changed"], json!(added), "{second}");
        let now = second["added"].as_array().unwrap();
        assert_eq!(now.len(), 1, "{second}");
        assert_eq!(setup.clause_by_id(&now[0])["anchor"]["path"], "CLAUDE.md");
        assert!(setup.blocks_a_migration_edit());
    }

    #[test]
    fn a_frontmatter_change_reaches_the_lock() {
        let setup = setup();
        setup.repo.write(
            ".cursor/rules/db.mdc",
            "---\nglobs: drizzle/**\n---\nLeave the migrations alone.\n",
        );
        setup
            .repo
            .write(".pragma/specs/db.md", "Never touch migrations by hand.\n");
        setup.repo.commit();
        setup.extract();
        lock_json(&setup.repo);
        let by_doc = |doc: &str| {
            setup.locked()["clauses"]
                .as_array()
                .unwrap()
                .iter()
                .find(|clause| clause["anchor"]["path"] == doc)
                .cloned()
        };
        let cursor = by_doc(".cursor/rules/db.mdc").unwrap();
        assert_eq!(cursor["scope"], json!(["drizzle/**"]));
        assert!(by_doc(".pragma/specs/db.md").is_some());

        // The text stays; the frontmatter narrows one scope and makes the other rule judged.
        setup.repo.write(
            ".cursor/rules/db.mdc",
            "---\nglobs: drizzle/0001_init.sql\n---\nLeave the migrations alone.\n",
        );
        setup.repo.write(
            ".pragma/specs/db.md",
            "---\nrung: judged\n---\nNever touch migrations by hand.\n",
        );
        setup.repo.commit();
        setup.extract();
        assert_eq!(setup.status()["lock_pending"], true);
        let update = lock_json(&setup.repo);
        assert_eq!(update["changed"].as_array().unwrap().len(), 2, "{update}");
        assert_eq!(
            by_doc(".cursor/rules/db.mdc").unwrap()["scope"],
            json!(["drizzle/0001_init.sql"])
        );
        assert!(by_doc(".pragma/specs/db.md").is_none());
        assert!(setup.repo.run(&["lock", "check"]).status.success());
        assert_eq!(setup.status()["lock_pending"], false);
    }

    #[test]
    fn an_untracked_draft_does_not_reach_the_committed_lock() {
        let setup = setup();
        setup.extract();
        lock_json(&setup.repo);
        let before = setup.repo.read(".pragma/pragma.lock");

        setup.repo.write(
            "docs/draft.md",
            "# Draft\n\nDo not hand-edit the drizzle migrations.\n",
        );
        setup.extract();
        assert_eq!(setup.status()["lock_pending"], false);
        assert_eq!(setup.status()["lock_unsettled"], 0);
        let update = lock_json(&setup.repo);
        assert_eq!(update["added"], json!([]), "{update}");
        assert_eq!(setup.repo.read(".pragma/pragma.lock"), before);

        setup.repo.git(&["add", "docs/draft.md"]);
        assert_eq!(setup.status()["lock_pending"], true);
        let tracked = lock_json(&setup.repo);
        let added = tracked["added"].as_array().unwrap();
        assert_eq!(added.len(), 1, "{tracked}");
        assert_eq!(
            setup.clause_by_id(&added[0])["anchor"]["path"],
            "docs/draft.md"
        );
    }

    #[test]
    fn lock_refuses_to_write_a_lock_with_problems() {
        let setup = setup();
        setup.extract();
        // A hand-edited index proposes a blocking clause whose scope does not compile.
        let path = setup.repo.root.join(".git/pragma/index.json");
        let mut index: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for clause in index["clauses"].as_array_mut().unwrap() {
            if clause["rung"] == "blocking" {
                clause["scope"] = json!(["[bad"]);
            }
        }
        std::fs::write(&path, index.to_string()).unwrap();
        let before = setup.repo.read(".pragma/pragma.lock");
        let output = setup.repo.run(&["lock", "--json"]);
        assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
        assert!(stderr(&output).contains("[bad"), "{}", stderr(&output));
        assert_eq!(setup.repo.read(".pragma/pragma.lock"), before);
    }

    #[test]
    fn a_doc_that_cannot_be_read_changes_nothing_in_the_lock() {
        use std::os::unix::fs::PermissionsExt;
        let setup = setup();
        setup.extract();
        lock_json(&setup.repo);
        let before = setup.repo.read(".pragma/pragma.lock");

        let agents = setup.repo.root.join("AGENTS.md");
        std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o000)).unwrap();
        setup.extract();
        let index: Value = serde_json::from_str(
            &std::fs::read_to_string(setup.repo.root.join(".git/pragma/index.json")).unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&agents, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            index["failed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|failed| failed["path"] == "AGENTS.md"),
            "{index}"
        );
        assert_eq!(setup.status()["lock_pending"], false);
        let update = lock_json(&setup.repo);
        assert_eq!(update["removed"], json!([]), "{update}");
        assert_eq!(update["changed"], json!([]), "{update}");
        assert_eq!(setup.repo.read(".pragma/pragma.lock"), before);
        assert!(setup.blocks_a_migration_edit());
    }
}
