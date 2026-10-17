//! `pragma hooks install | uninstall`: pragma's entries in Claude Code's settings.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use pragma_engine::env::Env;
use pragma_engine::settings::{self, settings_path};
use serde_json::{Value, json};

use crate::json::{
    HookEvent, HooksInstallOutput, HooksState, HooksStatusOutput, HooksUninstallOutput,
};
use crate::output::print_json;

/// Whether pragma's hooks are in `settings` (or Claude Code's default settings file). A missing
/// or unreadable file counts as not installed.
pub fn hooks_state(settings: Option<PathBuf>, env: &impl Env) -> HooksState {
    let path = settings.or_else(|| settings_path(env));
    let installed = path
        .as_deref()
        .is_some_and(|path| read_settings(path).is_ok_and(|value| settings::is_installed(&value)));
    HooksState {
        settings: path,
        installed,
    }
}

pub fn status(settings: Option<PathBuf>, json: bool, env: &impl Env) -> Result<()> {
    let state = hooks_state(settings, env);
    if json {
        return print_json(&HooksStatusOutput {
            state,
            events: settings::HOOKS
                .iter()
                .map(|spec| HookEvent {
                    event: spec.event.to_string(),
                    matcher: spec.matcher.map(str::to_string),
                    subcommand: spec.subcommand.to_string(),
                })
                .collect(),
        });
    }
    let location = state.settings.as_deref().map_or_else(
        || "Claude Code's settings".to_string(),
        |p| p.display().to_string(),
    );
    if state.installed {
        println!("pragma's hooks are installed in {location}");
    } else {
        println!("pragma's hooks are not installed in {location}");
    }
    Ok(())
}

pub fn install(settings: Option<PathBuf>, json: bool, env: &impl Env) -> Result<()> {
    if cfg!(windows) {
        bail!("pragma hooks are not supported on Windows yet");
    }
    let path = resolve(settings, env)?;
    write_settings(&path, &settings::install(read_settings(&path)?)?)?;
    if json {
        return print_json(&HooksInstallOutput {
            settings: path,
            installed: true,
        });
    }
    println!("pragma's hooks are installed in {}", path.display());
    Ok(())
}

pub fn uninstall(settings: Option<PathBuf>, json: bool, env: &impl Env) -> Result<()> {
    let path = resolve(settings, env)?;
    if !path.exists() {
        if json {
            return print_json(&HooksUninstallOutput {
                settings: path,
                removed: false,
            });
        }
        println!("Nothing to remove: {} does not exist.", path.display());
        return Ok(());
    }
    write_settings(&path, &settings::uninstall(read_settings(&path)?)?)?;
    if json {
        return print_json(&HooksUninstallOutput {
            settings: path,
            removed: true,
        });
    }
    println!("pragma's hooks are removed from {}", path.display());
    Ok(())
}

fn resolve(settings: Option<PathBuf>, env: &impl Env) -> Result<PathBuf> {
    settings
        .or_else(|| settings_path(env))
        .context("cannot find Claude Code's settings; set CLAUDE_CONFIG_DIR or HOME")
}

fn read_settings(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is not valid JSON; pragma will not overwrite it",
            path.display()
        )
    })
}

/// Writes the settings to the file they live in. A symlink (a dotfile manager's link, say) is
/// followed, so the link survives, and the target keeps its permissions.
fn write_settings(path: &Path, settings: &Value) -> Result<()> {
    let mut text = serde_json::to_string_pretty(settings)?;
    text.push('\n');
    let target = if path.symlink_metadata().is_ok() {
        fs::canonicalize(path).with_context(|| format!("cannot resolve {}", path.display()))?
    } else {
        path.to_path_buf()
    };
    let mode = fs::metadata(&target).ok().map(|meta| meta.permissions());
    let dir = target
        .parent()
        .context("settings path has no parent directory")?;
    fs::create_dir_all(dir)?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("settings.json");
    let temp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    // The permissions are set before any content is written, so the temp file never holds the
    // settings at a looser mode than the original.
    let mut file = File::create(&temp)?;
    if let Some(mode) = mode {
        file.set_permissions(mode)?;
    }
    file.write_all(text.as_bytes())?;
    drop(file);
    fs::rename(&temp, &target)?;
    Ok(())
}
