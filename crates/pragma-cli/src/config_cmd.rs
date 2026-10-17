//! `pragma config show | set`: the repo settings in `.pragma/config.md`.

use std::path::Path;

use anyhow::{Result, bail};
use pragma_engine::config::{CONFIG_FILE, Config, DEFAULT_CONFIG, set_doc_globs};
use pragma_engine::lock::Scope;
use pragma_engine::writes::{record_write, write_atomic};

use crate::output::print_json;
use crate::repo_here;

pub fn show(cwd: &Path, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let config = Config::load(repo.effective_pragma_dir().as_deref())?;
    print_config(&config, json)
}

/// The globs to store for one key: the given ones, an empty list when cleared, else unchanged.
fn requested(globs: Vec<String>, clear: bool) -> Result<Option<Vec<String>>> {
    for glob in &globs {
        Scope::new(std::slice::from_ref(glob))
            .map_err(|error| anyhow::anyhow!("bad glob {glob:?}: {error:#}"))?;
    }
    Ok(match (globs.is_empty(), clear) {
        (false, _) => Some(globs),
        (true, true) => Some(Vec::new()),
        (true, false) => None,
    })
}

pub fn set(
    cwd: &Path,
    include: Vec<String>,
    exclude: Vec<String>,
    clear_include: bool,
    clear_exclude: bool,
    json: bool,
) -> Result<()> {
    let repo = repo_here(cwd)?;
    if repo.root_checkout.as_ref() != Some(&repo.toplevel) {
        bail!(
            "pragma changes .pragma/ only in the root checkout; run this there{}",
            repo.root_checkout
                .as_ref()
                .map(|root| format!(" ({})", root.display()))
                .unwrap_or_default()
        );
    }
    let include = requested(include, clear_include)?;
    let exclude = requested(exclude, clear_exclude)?;
    if include.is_none() && exclude.is_none() {
        bail!(
            "nothing to change; pass --docs-include, --docs-exclude, --clear-include or --clear-exclude"
        );
    }
    let dir = repo.writable_pragma_dir();
    if !dir.is_dir() {
        bail!("this repo has no .pragma/; run pragma init first");
    }
    let path = dir.join(CONFIG_FILE);
    let current = if path.is_file() {
        std::fs::read_to_string(&path)?
    } else {
        DEFAULT_CONFIG.to_string()
    };
    let text = set_doc_globs(&current, include.as_deref(), exclude.as_deref())?;
    write_atomic(&path, text.as_bytes())?;
    record_write(
        &repo.shared_state_dir(),
        &format!(".pragma/{CONFIG_FILE}"),
        text.as_bytes(),
    )?;
    print_config(&Config::load(Some(&dir))?, json)
}

fn print_config(config: &Config, json: bool) -> Result<()> {
    if json {
        return print_json(config);
    }
    for (label, globs) in [
        ("docs.include", &config.docs.include),
        ("docs.exclude", &config.docs.exclude),
    ] {
        if globs.is_empty() {
            println!("{label}: none");
        } else {
            println!("{label}: {}", globs.join(", "));
        }
    }
    Ok(())
}
