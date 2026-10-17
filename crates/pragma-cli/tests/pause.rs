mod common;

use common::TestRepo;

#[test]
fn pause_and_resume_toggle_the_marker_in_the_shared_git_dir() {
    let repo = TestRepo::new();
    let marker = repo.root.join(".git/pragma/paused");
    assert!(repo.run(&["pause"]).status.success());
    assert!(marker.exists());
    assert!(repo.run(&["resume"]).status.success());
    assert!(!marker.exists());
    let paused: serde_json::Value =
        serde_json::from_slice(&repo.run(&["pause", "--json"]).stdout).unwrap();
    assert_eq!(paused["paused"], true);
    let resumed: serde_json::Value =
        serde_json::from_slice(&repo.run(&["resume", "--json"]).stdout).unwrap();
    assert_eq!(resumed["paused"], false);
    assert!(
        repo.run(&["resume"]).status.success(),
        "resuming twice is fine"
    );
}

#[test]
fn pause_outside_a_repo_fails_with_a_clear_message() {
    let outside = tempfile::tempdir().unwrap();
    let output = common::pragma()
        .arg("pause")
        .current_dir(outside.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(common::stderr(&output).contains("not inside a git repository"));
}
