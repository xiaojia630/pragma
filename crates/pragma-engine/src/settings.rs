//! pragma's entries in Claude Code's settings.json. pragma owns only entries whose command
//! carries its marker, and leaves everything else as it was.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::env::{Env, home_dir};

pub const MARKER: &str = "PRAGMA_HOOK";

pub struct HookSpec {
    pub event: &'static str,
    pub matcher: Option<&'static str>,
    pub subcommand: &'static str,
    pub timeout_secs: u32,
}

/// The hooks pragma installs. S4 adds SessionStart, UserPromptSubmit, Stop and SessionEnd.
pub const HOOKS: &[HookSpec] = &[HookSpec {
    event: "PreToolUse",
    matcher: Some("Edit|Write|MultiEdit"),
    subcommand: "pre-tool-use",
    timeout_secs: 1,
}];

/// Claude Code's user settings: `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json`.
pub fn settings_path(env: &impl Env) -> Option<PathBuf> {
    env.var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| home_dir(env).map(|home| home.join(".claude")))
        .map(|dir| dir.join("settings.json"))
}

/// The shell command for one hook. It does nothing when the binary is missing, and keeps the
/// binary's exit code, because 2 is how a hook blocks.
pub fn hook_command(subcommand: &str) -> String {
    format!(
        "{MARKER}=1; p=\"${{PRAGMA_HOME:-$HOME/.pragma}}/bin/pragma\"; [ -x \"$p\" ] || exit 0; \
         exec \"$p\" hook {subcommand}"
    )
}

pub fn install(settings: Value) -> Result<Value> {
    let mut settings = uninstall(settings)?;
    let root = settings
        .as_object_mut()
        .ok_or_else(|| anyhow!("settings.json is not a JSON object"))?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("settings.json \"hooks\" is not an object"))?;
    for spec in HOOKS {
        let groups = hooks
            .entry(spec.event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| anyhow!("settings.json hooks.{} is not a list", spec.event))?;
        let mut group = Map::new();
        if let Some(matcher) = spec.matcher {
            group.insert("matcher".into(), json!(matcher));
        }
        group.insert(
            "hooks".into(),
            json!([{ "type": "command", "command": hook_command(spec.subcommand), "timeout": spec.timeout_secs }]),
        );
        groups.push(Value::Object(group));
    }
    Ok(settings)
}

/// Removes pragma's entries, and any group, event list or `hooks` object that removing them
/// left empty. Nothing else changes.
pub fn uninstall(mut settings: Value) -> Result<Value> {
    let root = settings
        .as_object_mut()
        .ok_or_else(|| anyhow!("settings.json is not a JSON object"))?;
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(settings);
    };
    let mut emptied_events = Vec::new();
    for (event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let before = groups.len();
        groups.retain_mut(|group| {
            let Some(entries) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let had = entries.len();
            entries.retain(|entry| !is_pragma(entry));
            !(had > 0 && entries.is_empty())
        });
        if before > 0 && groups.is_empty() {
            emptied_events.push(event.clone());
        }
    }
    for event in &emptied_events {
        hooks.remove(event);
    }
    if !emptied_events.is_empty() && hooks.is_empty() {
        root.remove("hooks");
    }
    Ok(settings)
}

/// Whether every hook pragma installs has its entry under its event.
pub fn is_installed(settings: &Value) -> bool {
    HOOKS.iter().all(|spec| {
        settings["hooks"][spec.event]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|group| group["hooks"].as_array())
            .flatten()
            .any(|entry| {
                is_pragma(entry)
                    && entry["command"].as_str().is_some_and(|command| {
                        command.ends_with(&format!("hook {}", spec.subcommand))
                    })
            })
    })
}

fn is_pragma(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.trim_start().starts_with(&format!("{MARKER}=1;")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pragma_commands(settings: &Value) -> Vec<String> {
        settings["hooks"]["PreToolUse"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|group| group["hooks"].as_array().cloned().unwrap_or_default())
            .filter(is_pragma)
            .map(|entry| entry["command"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn installs_into_empty_settings() {
        let settings = install(json!({})).unwrap();
        let group = &settings["hooks"]["PreToolUse"][0];
        assert_eq!(group["matcher"], "Edit|Write|MultiEdit");
        assert_eq!(group["hooks"][0]["type"], "command");
        assert_eq!(group["hooks"][0]["timeout"], 1);
        assert!(
            group["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("hook pre-tool-use")
        );
    }

    #[test]
    fn install_is_idempotent_and_keeps_user_settings_in_order() {
        let user = json!({
            "model": "opus",
            "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "mine" }] }] },
            "theme": "dark"
        });
        let twice = install(install(user).unwrap()).unwrap();
        assert_eq!(pragma_commands(&twice).len(), 1);
        assert_eq!(
            twice["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "mine"
        );
        let keys: Vec<&String> = twice.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["model", "hooks", "theme"]);
    }

    #[test]
    fn uninstall_removes_only_pragma_entries() {
        let user = json!({ "hooks": { "Stop": [{ "hooks": [] }] } });
        let round_trip = uninstall(install(user.clone()).unwrap()).unwrap();
        assert_eq!(round_trip, user);
        assert_eq!(uninstall(install(json!({})).unwrap()).unwrap(), json!({}));
    }

    #[test]
    fn a_user_command_that_merely_mentions_the_marker_survives() {
        let user = json!({ "hooks": { "PreToolUse": [{ "hooks": [
            { "type": "command", "command": "echo PRAGMA_HOOK_DEBUG" }
        ] }] } });
        let installed = install(user.clone()).unwrap();
        assert_eq!(pragma_commands(&installed).len(), 1);
        assert_eq!(uninstall(installed).unwrap(), user);
    }

    #[test]
    fn is_installed_needs_a_pragma_entry_for_every_event() {
        assert!(!is_installed(&json!({})));
        assert!(!is_installed(&json!({ "hooks": "nope" })));
        let installed = install(json!({ "model": "opus" })).unwrap();
        assert!(is_installed(&installed));
        assert!(!is_installed(&uninstall(installed).unwrap()));
        let lookalike = json!({ "hooks": { "PreToolUse": [{ "hooks": [
            { "type": "command", "command": "echo PRAGMA_HOOK=1; hook pre-tool-use" }
        ] }] } });
        assert!(!is_installed(&lookalike));
    }

    #[test]
    fn refuses_settings_that_are_not_an_object() {
        assert!(install(json!([1, 2])).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_command_keeps_exit_code_2_and_skips_a_missing_binary() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let run = || {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(hook_command("pre-tool-use"))
                .env("PRAGMA_HOME", home.path())
                .status()
                .unwrap()
                .code()
        };
        assert_eq!(run(), Some(0));
        let bin = home.path().join("bin/pragma");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::fs::write(
            &bin,
            "#!/bin/sh\n[ \"$1 $2\" = \"hook pre-tool-use\" ] && exit 2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(run(), Some(2));
    }
}
