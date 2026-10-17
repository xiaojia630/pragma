//! `pragma init`: creates `.pragma/` in the root checkout. Existing files are left alone.

use std::path::Path;

use anyhow::Result;
use pragma_engine::config::{CONFIG_FILE, DEFAULT_CONFIG};
use pragma_engine::lock::{LOCK_FILE, Lock};
use pragma_engine::writes::{record_write, write_atomic};

use crate::json::InitOutput;
use crate::output::print_json;
use crate::repo_here;

pub const README: &str = "# .pragma\n\n\
This folder holds pragma's settings for this repo. pragma makes coding agents follow the\n\
engineering specs in this repo's docs.\n\n\
- `config.md`: repo settings.\n\
- `overrides.md`: corrections and exceptions, kept by pragma.\n\
- `pragma.lock`: the rules that block edits, compiled from the docs. It is generated; review its\n  \
diffs like a lockfile.\n\
- `specs/`: extra specs written for pragma, in plain markdown.\n\n\
Deleting this folder turns pragma off for the repo. The docs stay exactly as they are.\n";

pub fn run(cwd: &Path, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let dir = repo.writable_pragma_dir();
    let files = [
        ("README.md", README.to_string()),
        (CONFIG_FILE, DEFAULT_CONFIG.to_string()),
        (LOCK_FILE, Lock::empty().to_json()),
    ];
    let mut created = Vec::new();
    for (name, contents) in files {
        let path = dir.join(name);
        if path.exists() {
            continue;
        }
        write_atomic(&path, contents.as_bytes())?;
        record_write(
            &repo.shared_state_dir(),
            &format!(".pragma/{name}"),
            contents.as_bytes(),
        )?;
        created.push(format!(".pragma/{name}"));
    }
    if json {
        return print_json(&InitOutput {
            pragma_dir: dir,
            created,
        });
    }
    if created.is_empty() {
        println!("{} is already set up.", dir.display());
    } else {
        println!("Created {} in {}.", created.join(", "), dir.display());
        println!("Commit .pragma/ so every worktree and machine sees it.");
    }
    Ok(())
}
