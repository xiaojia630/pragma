mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{
    TestRepo, pattern_clause, pragma, protected_clause, stderr, stdout, supports_no_lazy_fetch,
};
use serde_json::{Value, json};

/// Commits everything with `message` as the subject.
fn commit(repo: &TestRepo, message: &str) {
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", message]);
}

/// `git rev-parse` with `args`, trimmed.
fn rev_parse(repo: &TestRepo, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("rev-parse")
        .args(args)
        .current_dir(&repo.root)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A repo with a `require()` rule, then two commits that each add one violation and one that
/// adds none.
fn history_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.lock(json!([pattern_clause(
        "no-require",
        "require($$$A)",
        &["**/*.ts"],
        "require('a');",
        "import a from 'a';"
    )]));
    commit(&repo, "set up pragma");
    repo.write("src/a.ts", "const a = require('a');\n");
    commit(&repo, "add a");
    repo.write("src/b.ts", "export const b = 1;\n");
    commit(&repo, "add b");
    repo.write("src/b.ts", "export const b = require('b');\n");
    commit(&repo, "require b");
    repo
}

fn backtest_json(repo: &TestRepo, args: &[&str]) -> Value {
    let output = repo.run(&[&["backtest", "--json"], args].concat());
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_slice(&output.stdout).unwrap()
}

/// A repo with the `require()` rule committed and nothing else yet.
fn rule_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.lock(json!([pattern_clause(
        "no-require",
        "require($$$A)",
        &["**/*.ts"],
        "require('a');",
        "import a from 'a';"
    )]));
    commit(&repo, "set up pragma");
    repo
}

/// What `git clone --filter=blob:none` leaves of `source`, next to it: every commit and tree,
/// and only the blobs HEAD needs.
fn blobless_clone(source: &TestRepo) -> PathBuf {
    source.git(&["config", "uploadpack.allowFilter", "true"]);
    source.git(&[
        "clone",
        "-q",
        "--filter=blob:none",
        &format!("file://{}", source.root.display()),
        "../clone",
    ]);
    source.temp_dir().join("clone")
}

/// `pragma backtest` with `args` in `dir`.
fn backtest_in(dir: &Path, args: &[&str]) -> Output {
    pragma()
        .arg("backtest")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

/// The `--json` report of a backtest in `dir`.
fn backtest_json_in(dir: &Path) -> Value {
    let output = backtest_in(dir, &["--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn backtest_reports_what_each_commit_introduced_and_saves_it() {
    let repo = history_repo();
    let report = backtest_json(&repo, &[]);
    assert_eq!(report["depth"], 200);
    assert_eq!(report["commits_checked"], 4);
    assert_eq!(report["rules"], 1);
    assert_eq!(report["rules_checked"], 1);
    assert_eq!(report["head"], rev_parse(&repo, &["HEAD"]));
    assert_eq!(report["total"], 2);
    assert_eq!(report["counts"], json!({ "no-require": 2 }));
    let newest = &report["hits"][0];
    assert_eq!(newest["commit"], rev_parse(&repo, &["HEAD"]));
    assert_eq!(newest["short"], rev_parse(&repo, &["--short", "HEAD"]));
    assert_eq!(newest["subject"], "require b");
    assert_eq!(newest["author"], "pragma");
    assert!(newest["at_s"].as_u64().unwrap() > 1_600_000_000);
    assert_eq!(newest["clause_id"], "no-require");
    assert_eq!(newest["file"], "src/b.ts");
    assert_eq!(newest["line"], 1);
    assert_eq!(newest["matched"], "require('b')");
    assert_eq!(report["hits"][1]["subject"], "add a");
    let saved: Value = serde_json::from_str(&repo.read(".git/pragma/backtest.json")).unwrap();
    assert_eq!(saved, report);
    assert!(
        repo.root.join(".git/pragma/backtest-cache.json").is_file(),
        "the cache lives in the common dir"
    );
}

#[test]
fn since_limits_the_walk() {
    let repo = history_repo();
    let report = backtest_json(&repo, &["--since", "1"]);
    assert_eq!(report["depth"], 1);
    assert_eq!(report["commits_checked"], 1);
    assert_eq!(report["total"], 1);
    assert_eq!(report["hits"][0]["subject"], "require b");
    let saved: Value = serde_json::from_str(&repo.read(".git/pragma/backtest.json")).unwrap();
    assert_eq!(saved["depth"], 1, "every run saves its report");
}

#[test]
fn a_second_run_is_served_from_the_cache() {
    let repo = history_repo();
    assert_eq!(backtest_json(&repo, &[])["total"], 2);
    // Without its blob, `src/a.ts` as `add a` wrote it can no longer be checked.
    let blob = rev_parse(&repo, &["HEAD:src/a.ts"]);
    fs::remove_file(
        repo.root
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )
    .unwrap();
    assert_eq!(
        backtest_json(&repo, &[])["total"],
        2,
        "nothing is read again"
    );
    fs::remove_file(repo.root.join(".git/pragma/backtest-cache.json")).unwrap();
    assert_eq!(
        backtest_json(&repo, &[])["total"],
        1,
        "checked again, the commit whose blob is gone finds nothing"
    );
}

#[test]
fn backtest_needs_pragma_to_be_set_up() {
    let repo = TestRepo::new();
    let output = repo.run(&["backtest"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("run pragma init"));
}

#[test]
fn human_output_lists_the_rules_broken_and_their_commits() {
    let repo = history_repo();
    let human = stdout(&repo.run(&["backtest"]));
    assert!(
        human.contains("Checked 4 commits against 1 rule.\n"),
        "{human}"
    );
    assert!(
        human.contains("The history has only 5 commits, fewer than the 200 asked for.\n"),
        "{human}"
    );
    assert!(
        human.contains("The first commit is not checked: there is nothing before it"),
        "{human}"
    );
    assert!(
        human.contains("pragma would have stopped 2 violations these commits introduced."),
        "{human}"
    );
    assert!(
        human.contains("2 x Never write require($$$A). (AGENTS.md:10)"),
        "{human}"
    );
    let short = rev_parse(&repo, &["--short", "HEAD"]);
    assert!(
        human.contains(&format!("    {short}  src/b.ts:1  require b")),
        "{human}"
    );
    let clean = TestRepo::new();
    clean.lock(json!([pattern_clause(
        "no-require",
        "require($$$A)",
        &["**/*.ts"],
        "require('a');",
        "import a from 'a';"
    )]));
    commit(&clean, "set up pragma");
    let human = stdout(&clean.run(&["backtest"]));
    assert!(
        human.contains("No violations in recent history."),
        "{human}"
    );
}

#[test]
fn one_commit_one_rule_one_violation_read_in_the_singular() {
    let repo = TestRepo::new();
    repo.lock(json!([pattern_clause(
        "no-require",
        "require($$$A)",
        &["**/*.ts"],
        "require('a');",
        "import a from 'a';"
    )]));
    repo.write("src/a.ts", "const a = require('a');\n");
    commit(&repo, "add a");
    let human = stdout(&repo.run(&["backtest"]));
    assert!(
        human.contains("Checked 1 commit against 1 rule.\n"),
        "{human}"
    );
    assert!(
        human.contains("The history has only 2 commits, fewer than the 200 asked for.\n"),
        "{human}"
    );
    assert!(
        human.contains("pragma would have stopped 1 violation these commits introduced."),
        "{human}"
    );
    assert!(human.contains("1 x Never write require($$$A)."), "{human}");
    let exact = stdout(&repo.run(&["backtest", "--since", "2"]));
    assert!(
        !exact.contains("fewer than"),
        "history as long as asked for is not short: {exact}"
    );
    assert!(exact.contains("The first commit is not checked"), "{exact}");
}

#[test]
fn rules_checked_leaves_out_the_rules_that_apply_only_to_edits() {
    let repo = TestRepo::new();
    repo.lock(json!([
        pattern_clause(
            "no-require",
            "require($$$A)",
            &["**/*.ts"],
            "require('a');",
            "import a from 'a';"
        ),
        protected_clause("migrations", &["drizzle/**"]),
    ]));
    repo.write("src/a.ts", "const a = require('a');\n");
    commit(&repo, "add a");
    let report = backtest_json(&repo, &[]);
    assert_eq!(report["rules"], 2);
    assert_eq!(report["rules_checked"], 1);
    let saved: Value = serde_json::from_str(&repo.read(".git/pragma/backtest.json")).unwrap();
    assert_eq!(saved["rules_checked"], 1);
}

#[test]
fn a_lock_of_only_edit_rules_says_nothing_could_be_checked() {
    let repo = TestRepo::new();
    repo.lock(json!([protected_clause("migrations", &["drizzle/**"])]));
    commit(&repo, "set up pragma");
    assert_eq!(backtest_json(&repo, &[])["rules_checked"], 0);
    let human = stdout(&repo.run(&["backtest"]));
    assert!(
        human.contains("Checked 1 commit against 0 rules.\n"),
        "{human}"
    );
    assert!(
        human.contains("None of the rules in the lock can be checked against history."),
        "{human}"
    );
    assert!(!human.contains("No violations"), "{human}");
}

#[test]
fn the_command_applies_exceptions_to_the_history() {
    let repo = history_repo();
    repo.write(
        ".pragma/overrides.md",
        "---\nexceptions:\n- kind: violation\n  clause: no-require\n  para_hash: p10\n  \
         file: src/a.ts\n  matched: require('a')\n  reason: deliberate\n---\n",
    );
    let report = backtest_json(&repo, &[]);
    assert_eq!(report["total"], 1);
    assert_eq!(report["counts"], json!({ "no-require": 1 }));
    let files: Vec<&str> = report["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["file"].as_str().unwrap())
        .collect();
    assert_eq!(files, vec!["src/b.ts"], "the excepted hit is gone");
}

#[cfg(unix)]
#[test]
fn backtest_runs_git_without_optional_locks() {
    let repo = history_repo();
    let dir = tempfile::tempdir().unwrap();
    let mut command = common::pragma();
    common::log_git_calls(&mut command, dir.path());
    let output = command
        .args(["backtest", "--json"])
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let calls = common::logged_git_calls(dir.path());
    assert!(
        calls.iter().any(|call| call.contains("diff-tree"))
            && calls.iter().any(|call| call.contains("cat-file")),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|call| call.starts_with("GIT_OPTIONAL_LOCKS=0 ")),
        "{calls:?}"
    );
}

#[test]
fn backtest_in_a_task_worktree_walks_its_own_head() {
    let repo = history_repo();
    repo.git(&["worktree", "add", "-q", "../task", "-b", "task", "HEAD~1"]);
    let task = repo.temp_dir().join("task");
    let output = pragma()
        .args(["backtest", "--json"])
        .current_dir(&task)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["commits_checked"], 3);
    assert_eq!(report["total"], 1);
    assert_eq!(report["hits"][0]["subject"], "add a");
    assert!(
        repo.root
            .join(".git/worktrees/task/pragma/backtest.json")
            .is_file()
    );
    assert!(!repo.root.join(".git/pragma/backtest.json").exists());
}

#[test]
fn a_partial_clone_does_not_report_an_all_clear_for_history_it_cannot_read() {
    let test = "a_partial_clone_does_not_report_an_all_clear_for_history_it_cannot_read";
    if !supports_no_lazy_fetch(test) {
        return;
    }
    // `fix a` replaces the file `add a` wrote, so HEAD does not hold the blob with the
    // violation, and a blobless clone holds only HEAD's blobs.
    let source = rule_repo();
    source.write("src/a.ts", "const a = require('a');\n");
    commit(&source, "add a");
    source.write("src/a.ts", "import a from 'a';\n");
    commit(&source, "fix a");
    let full = backtest_json(&source, &[]);
    assert_eq!(full["total"], 1, "the whole history has the violation");
    assert_eq!(full["incomplete_commits"], 0);

    let clone = blobless_clone(&source);
    let report = backtest_json_in(&clone);
    assert_eq!(report["commits_checked"], 3);
    assert_eq!(
        report["incomplete_commits"], 2,
        "`add a` lacks its new blob and `fix a` its old one"
    );
    assert_eq!(report["total"], 0);
    assert_eq!(report["counts"], json!({}));
    assert_eq!(report["hits"], json!([]));
    let saved: Value =
        serde_json::from_str(&fs::read_to_string(clone.join(".git/pragma/backtest.json")).unwrap())
            .unwrap();
    assert_eq!(saved, report, "the saved report says so too");

    let human = stdout(&backtest_in(&clone, &[]));
    assert!(
        human.contains("Checked 3 commits against 1 rule.\n"),
        "{human}"
    );
    assert!(
        human
            .contains("2 commits could not be fully checked: their files are not in this clone.\n"),
        "{human}"
    );
    assert!(!human.contains("No violations"), "{human}");
}

#[test]
fn a_partial_clone_does_not_report_a_rename_with_an_edit_as_a_new_violation() {
    let test = "a_partial_clone_does_not_report_a_rename_with_an_edit_as_a_new_violation";
    if !supports_no_lazy_fetch(test) {
        return;
    }
    // Enough unchanged lines for git to take the edited file for the old one, renamed.
    let body = (1..=20)
        .map(|n| format!("export const v{n} = {n};\n"))
        .collect::<String>();
    let source = rule_repo();
    source.write("src/old.ts", &format!("{body}const a = require('a');\n"));
    commit(&source, "add old");
    source.git(&["mv", "src/old.ts", "src/new.ts"]);
    source.write(
        "src/new.ts",
        &format!("{body}const a = require('a');\nexport const extra = 1;\n"),
    );
    commit(&source, "rename and edit");
    let full = backtest_json(&source, &[]);
    assert_eq!(full["total"], 1, "only `add old` introduced the violation");
    assert_eq!(full["hits"][0]["subject"], "add old");

    // Scoring the rename needs `src/old.ts`'s blob, which the clone lacks: the rename then
    // shows as a new file with a violation in it.
    let clone = blobless_clone(&source);
    let report = backtest_json_in(&clone);
    assert_eq!(report["total"], 0, "{report}");
    assert_eq!(report["hits"], json!([]), "{report}");
    assert_eq!(
        report["incomplete_commits"], 2,
        "`add old` lacks its blob and `rename and edit` could not be scored"
    );

    let human = stdout(&backtest_in(&clone, &[]));
    assert!(!human.contains("src/new.ts"), "{human}");
    assert!(
        human
            .contains("2 commits could not be fully checked: their files are not in this clone.\n"),
        "{human}"
    );
    assert!(!human.contains("No violations"), "{human}");
}

#[test]
fn a_partial_clone_keeps_the_hits_of_commits_it_could_check() {
    let test = "a_partial_clone_keeps_the_hits_of_commits_it_could_check";
    if !supports_no_lazy_fetch(test) {
        return;
    }
    let source = rule_repo();
    source.write("src/a.ts", "const a = require('a');\n");
    commit(&source, "add a");
    source.write("src/a.ts", "import a from 'a';\n");
    commit(&source, "fix a");
    // A new file, so only its own blob, which HEAD holds, is needed to check it.
    source.write("src/c.ts", "const c = require('c');\n");
    commit(&source, "add c");

    let clone = blobless_clone(&source);
    let report = backtest_json_in(&clone);
    assert_eq!(report["commits_checked"], 4);
    assert_eq!(report["incomplete_commits"], 2);
    assert_eq!(report["total"], 1);
    assert_eq!(report["counts"], json!({ "no-require": 1 }));
    assert_eq!(report["hits"].as_array().unwrap().len(), 1);
    assert_eq!(report["hits"][0]["subject"], "add c");
    assert_eq!(report["hits"][0]["file"], "src/c.ts");

    let human = stdout(&backtest_in(&clone, &[]));
    assert!(
        human.contains("pragma would have stopped 1 violation these commits introduced."),
        "{human}"
    );
    assert!(human.contains("src/c.ts:1  add c"), "{human}");
    assert!(
        human.ends_with(
            "2 commits could not be fully checked: their files are not in this clone.\n"
        ),
        "{human}"
    );
}

#[test]
fn since_zero_is_refused_and_the_saved_report_stays() {
    let repo = history_repo();
    backtest_json(&repo, &[]);
    let saved = repo.read(".git/pragma/backtest.json");

    for args in [
        &["backtest", "--since", "0"][..],
        &["backtest", "--since", "0", "--json"],
    ] {
        let output = repo.run(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(stdout(&output).is_empty(), "{}", stdout(&output));
        assert!(
            stderr(&output).contains("--since") && stderr(&output).contains("at least 1"),
            "{}",
            stderr(&output)
        );
        assert_eq!(
            repo.read(".git/pragma/backtest.json"),
            saved,
            "{args:?} left the saved report alone"
        );
    }
    assert_eq!(backtest_json(&repo, &["--since", "1"])["depth"], 1);
}
