mod common;

use common::pragma;
use serde_json::Value;

#[test]
fn version_prints_the_binary_version() {
    let output = pragma().args(["version", "--json"]).output().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn command_line_errors_exit_1_and_help_exits_0() {
    for args in [&["bogus"][..], &["audit", "--nope"][..]] {
        let output = common::pragma().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(1), "{args:?}");
    }
    let output = common::pragma().arg("--help").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
}
