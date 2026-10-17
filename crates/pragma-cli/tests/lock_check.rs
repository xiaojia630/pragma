mod common;

use common::{TestRepo, pattern_clause, stdout};
use serde_json::json;

#[test]
fn a_fresh_lock_is_sound() {
    let repo = TestRepo::new();
    assert!(repo.run(&["init"]).status.success());
    let check = repo.run(&["lock", "check"]);
    assert!(check.status.success());
    assert!(stdout(&check).contains("sound: 0 clauses"));
}

#[test]
fn a_pattern_that_misses_its_bad_example_fails_the_check() {
    let repo = TestRepo::new();
    repo.lock(json!([pattern_clause(
        "c",
        "require($$$A)",
        &[],
        "import a from 'a';",
        "require('a');"
    )]));
    let check = repo.run(&["lock", "check"]);
    assert!(!check.status.success());
    assert!(stdout(&check).contains("does not match its bad example"));
}
