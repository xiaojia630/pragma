//! `pragma debug anchor <doc>:<line>`: the clause id and anchor for a paragraph, for writing a
//! lock by hand.

use std::path::Path;

use anyhow::{Context, Result};
use pragma_engine::markdown::{anchor_for, clause_id, paragraph_at_line};
use pragma_engine::paths::{normalize, repo_relative};

use crate::json::DebugAnchorOutput;
use crate::output::print_json;
use crate::repo_here;

pub fn anchor(cwd: &Path, location: &str, ordinal: u32) -> Result<()> {
    let repo = repo_here(cwd)?;
    let (doc, line) = location.rsplit_once(':').context("expected <doc>:<line>")?;
    let line: u32 = line.parse().context("the line must be a number")?;
    let path = normalize(cwd, Path::new(doc));
    let rel = repo_relative(&repo.toplevel, &path).context("the doc is not inside this repo")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
    let para =
        paragraph_at_line(&text, line).with_context(|| format!("no paragraph at {rel}:{line}"))?;
    let anchor = anchor_for(&rel, &text, &para);
    let id = clause_id(&rel, &anchor.para_hash, ordinal);
    print_json(&DebugAnchorOutput { id, anchor })
}
