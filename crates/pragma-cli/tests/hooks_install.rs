mod common;

use common::{TestRepo, stdout};
use serde_json::{Value, json};

#[test]
fn install_and_uninstall_keep_user_settings() {
    let repo = TestRepo::new();
    let settings = repo.temp_dir().join("claude/settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    std::fs::write(&settings, r#"{"model": "opus"}"#).unwrap();
    let path = settings.to_str().unwrap();
    let output = repo.run(&["hooks", "install", "--settings", path, "--json"]);
    assert!(output.status.success());
    let report: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["installed"], true);
    assert_eq!(report["settings"], path);
    let installed: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(installed["model"], "opus");
    let command = installed["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(command.contains("PRAGMA_HOOK") && command.contains("hook pre-tool-use"));
    assert!(
        repo.run(&["hooks", "uninstall", "--settings", path])
            .status
            .success()
    );
    let removed: Value =
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(removed, json!({ "model": "opus" }));
}

#[test]
fn install_refuses_to_overwrite_broken_settings() {
    let repo = TestRepo::new();
    let settings = repo.temp_dir().join("settings.json");
    std::fs::write(&settings, "{ not json").unwrap();
    let output = repo.run(&["hooks", "install", "--settings", settings.to_str().unwrap()]);
    assert!(!output.status.success());
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{ not json");
}

#[cfg(unix)]
#[test]
fn install_writes_through_a_symlink_and_keeps_the_target_mode() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let repo = TestRepo::new();
    let dotfiles = repo.temp_dir().join("dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    let target = dotfiles.join("settings.json");
    std::fs::write(&target, r#"{"model": "opus"}"#).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link_dir = repo.temp_dir().join("claude");
    std::fs::create_dir_all(&link_dir).unwrap();
    let link = link_dir.join("settings.json");
    symlink(&target, &link).unwrap();

    let output = repo.run(&["hooks", "install", "--settings", link.to_str().unwrap()]);
    assert!(output.status.success());
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.contains("PRAGMA_HOOK"));
    assert!(text.contains("\"model\": \"opus\""));
    let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
