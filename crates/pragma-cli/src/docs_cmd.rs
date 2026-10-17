//! `pragma docs`: the spec docs pragma reads, with paragraph counts. Given globs (or
//! `--preview`, which also covers no globs at all), previews them in place of the saved ones
//! without changing anything.

use std::path::Path;

use anyhow::Result;
use pragma_engine::config::{Config, DocGlobs};
use pragma_engine::docs::discover;
use pragma_engine::markdown::paragraphs;

use crate::json::{DocEntry, DocsOutput};
use crate::output::print_json;
use crate::repo_here;

pub fn run(
    cwd: &Path,
    include: Vec<String>,
    exclude: Vec<String>,
    preview: bool,
    json: bool,
) -> Result<()> {
    let repo = repo_here(cwd)?;
    let root = &repo.toplevel;
    let globs = if !preview && include.is_empty() && exclude.is_empty() {
        Config::load(repo.effective_pragma_dir().as_deref())?.docs
    } else {
        DocGlobs { include, exclude }
    };
    let found = discover(root, &globs)?;
    let without_exclude = discover(
        root,
        &DocGlobs {
            include: globs.include.clone(),
            exclude: Vec::new(),
        },
    )?;
    let mut excluded: Vec<String> = without_exclude
        .into_iter()
        .map(|doc| doc.rel)
        .filter(|rel| !found.iter().any(|doc| &doc.rel == rel))
        .collect();
    excluded.sort();
    let mut docs: Vec<DocEntry> = found
        .into_iter()
        .map(|doc| {
            let text = std::fs::read_to_string(root.join(&doc.rel)).unwrap_or_default();
            DocEntry {
                paragraphs: paragraphs(&text).len(),
                path: doc.rel,
                historical: doc.historical,
            }
        })
        .collect();
    docs.sort_by(|a, b| a.path.cmp(&b.path));
    let output = DocsOutput {
        total_paragraphs: docs.iter().map(|doc| doc.paragraphs).sum(),
        docs,
        excluded,
    };
    if json {
        return print_json(&output);
    }
    print_human(&output);
    Ok(())
}

fn print_human(output: &DocsOutput) {
    println!(
        "{} docs, {} paragraphs.",
        output.docs.len(),
        output.total_paragraphs
    );
    let mut biggest: Vec<&DocEntry> = output.docs.iter().collect();
    biggest.sort_by(|a, b| b.paragraphs.cmp(&a.paragraphs).then(a.path.cmp(&b.path)));
    for doc in biggest.into_iter().take(10) {
        let note = if doc.historical { "  (historical)" } else { "" };
        println!("  {:>5}  {}{note}", doc.paragraphs, doc.path);
    }
    if !output.excluded.is_empty() {
        println!(
            "\n{} docs left out by the exclude globs.",
            output.excluded.len()
        );
    }
}
