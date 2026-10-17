//! `pragma commit`: commits `.pragma/` and nothing else, in the root checkout. For people and
//! the app; hooks never call it.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::json::CommitOutput;
use crate::output::print_json;
use crate::repo_here;

/// Runs git with an argument list (no shell) in `dir` and returns its stdout.
fn git(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .context("running git")
}

fn git_ok(dir: &Path, args: &[&str]) -> Result<String> {
    let output = git(dir, args)?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn run(cwd: &Path, message: &str, json: bool) -> Result<()> {
    let repo = repo_here(cwd)?;
    let Some(root) = repo
        .root_checkout
        .as_ref()
        .filter(|root| *root == &repo.toplevel)
    else {
        bail!("pragma commits .pragma/ only from the root checkout; run this there");
    };
    if !root.join(".pragma").is_dir() {
        bail!("this repo has no .pragma/; run pragma init first");
    }
    git_ok(root, &["add", "--", ".pragma"])?;
    let staged = git(root, &["diff", "--cached", "--quiet", "--", ".pragma"])?;
    let output = if staged.status.success() {
        CommitOutput {
            committed: false,
            sha: None,
            files: Vec::new(),
        }
    } else {
        let names = git_ok(
            root,
            &["diff", "--cached", "--name-only", "-z", "--", ".pragma"],
        )?;
        let files = names
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        git_ok(root, &["commit", "-m", message, "--", ".pragma"])?;
        let sha = git_ok(root, &["rev-parse", "HEAD"])?.trim().to_string();
        CommitOutput {
            committed: true,
            sha: Some(sha),
            files,
        }
    };
    if json {
        return print_json(&output);
    }
    match &output.sha {
        Some(sha) => println!(
            "Committed {} files in .pragma/ as {}.",
            output.files.len(),
            &sha[..sha.len().min(9)]
        ),
        None => println!("Nothing to commit: .pragma/ has no changes."),
    }
    Ok(())
}
