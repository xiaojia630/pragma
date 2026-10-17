//! Finds a repo's spec docs: the known names, `docs/**`, `.pragma/specs/**`, the docs they link
//! to, and the include and exclude globs in `.pragma/config.md`.

use std::collections::{BTreeSet, VecDeque};
use std::path::Path;

use anyhow::Result;

use crate::config::DocGlobs;
use crate::frontmatter;
use crate::lock::Scope;
use crate::paths::join_relative;
use crate::refs::{self, RefSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocFile {
    pub rel: String,
    /// ADRs and docs marked superseded, deprecated, rejected or historical: they describe the
    /// past, so drift skips them and they yield no clauses.
    pub historical: bool,
}

pub(crate) const KNOWN_NAMES: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "CONTRIBUTING.md",
    "README.md",
];
const ROOT_GLOSSARIES: &[&str] = &["CONTEXT.md", "GLOSSARY.md"];
pub(crate) const SKIPPED_DIRS: &[&str] = &[".git", "node_modules", "vendor", "third_party"];

/// True when a component of `rel` is one of `SKIPPED_DIRS`: a path the walk never enters, so
/// nothing that counts files (the audit, history) counts it either.
pub(crate) fn in_skipped_dir(rel: &str) -> bool {
    rel.split('/').any(|part| SKIPPED_DIRS.contains(&part))
}

/// Every file in the repo, repo-relative: respects .gitignore, includes dotfiles, and skips
/// `.git`, `node_modules`, `vendor` and `third_party`.
pub fn walk_files(root: &Path) -> Vec<String> {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| !SKIPPED_DIRS.iter().any(|skip| entry.file_name() == *skip))
        .build();
    let mut files: Vec<String> = walker
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| crate::paths::repo_relative(root, entry.path()))
        .collect();
    files.sort();
    files
}

fn is_seed(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    KNOWN_NAMES.contains(&name)
        || ROOT_GLOSSARIES.contains(&rel)
        || rel == ".github/copilot-instructions.md"
        || rel == ".cursorrules"
        || (rel.starts_with(".cursor/rules/") && rel.ends_with(".mdc"))
        || (rel.starts_with("docs/") && rel.ends_with(".md"))
        || (rel.starts_with(".pragma/specs/") && rel.ends_with(".md"))
}

/// Changelogs and licenses are not specs, and neither is pragma's own folder (apart from the
/// specs written for pragma).
fn is_excluded(rel: &str) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel).to_ascii_uppercase();
    let own_files = rel.starts_with(".pragma/") && !rel.starts_with(".pragma/specs/");
    name.starts_with("CHANGELOG") || name.starts_with("LICENSE") || own_files
}

pub fn discover(root: &Path, globs: &DocGlobs) -> Result<Vec<DocFile>> {
    discover_in(root, globs, &walk_files(root))
}

/// `discover` over a file list the caller already walked (`walk_files`), so a command that
/// needs the files too walks the repo once.
pub fn discover_in(root: &Path, globs: &DocGlobs, files: &[String]) -> Result<Vec<DocFile>> {
    let include = (!globs.include.is_empty())
        .then(|| Scope::new(&globs.include))
        .transpose()?;
    let exclude = (!globs.exclude.is_empty())
        .then(|| Scope::new(&globs.exclude))
        .transpose()?;
    let walked: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    let wanted =
        |rel: &str| !is_excluded(rel) && !exclude.as_ref().is_some_and(|scope| scope.matches(rel));
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = files
        .iter()
        .filter(|rel| is_seed(rel) || include.as_ref().is_some_and(|scope| scope.matches(rel)))
        .filter(|rel| wanted(rel))
        .cloned()
        .collect();
    while let Some(rel) = queue.pop_front() {
        if !found.insert(rel.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        let dir = rel.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
        for reference in refs::extract(&text) {
            if !matches!(reference.source, RefSource::Link | RefSource::Import) {
                continue;
            }
            let target = reference.text.split(['#', '?']).next().unwrap_or("");
            if !(target.ends_with(".md") || target.ends_with(".mdc")) || target.contains("://") {
                continue;
            }
            let linked = match target.strip_prefix('/') {
                Some(rooted) => join_relative("", rooted),
                None => join_relative(dir, target),
            };
            if let Some(linked) =
                linked.filter(|linked| wanted(linked) && walked.contains(linked.as_str()))
            {
                queue.push_back(linked);
            }
        }
    }
    Ok(found
        .into_iter()
        .map(|rel| {
            let historical = is_historical(
                &rel,
                &std::fs::read_to_string(root.join(&rel)).unwrap_or_default(),
            );
            DocFile { rel, historical }
        })
        .collect())
}

fn is_historical(rel: &str, text: &str) -> bool {
    // An ADR folder counts only under a `docs` ancestor, so live docs in a feature's own
    // `decisions/` folder stay current.
    let parts: Vec<String> = rel.split('/').map(str::to_ascii_lowercase).collect();
    let dirs = &parts[..parts.len().saturating_sub(1)];
    let in_adr_dir = dirs
        .iter()
        .position(|dir| dir == "docs")
        .is_some_and(|docs| {
            dirs[docs + 1..]
                .iter()
                .any(|dir| matches!(dir.as_str(), "adr" | "adrs" | "decisions"))
        });
    if in_adr_dir {
        return true;
    }
    let past = ["superseded", "deprecated", "rejected", "historical"];
    if let (Some(yaml), _) = frontmatter::split(text) {
        let status = yaml
            .lines()
            .find_map(|line| line.trim().strip_prefix("status:"));
        if status.is_some_and(|status| past.contains(&status.trim().to_ascii_lowercase().as_str()))
        {
            return true;
        }
    }
    text.lines().take(30).any(|line| {
        let line = line
            .trim()
            .trim_start_matches(['*', '_', '#', ' '])
            .to_ascii_lowercase();
        line.strip_prefix("status")
            .map(|rest| rest.trim_start_matches(['*', '_', ':', ' ']))
            .is_some_and(|rest| past.iter().any(|word| rest.starts_with(word)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn finds_known_docs_and_follows_links() {
        let (_temp, root) = crate::testutil::repo();
        write(
            &root,
            "AGENTS.md",
            "Read [the overview](agents/overview.md) first.\n",
        );
        write(
            &root,
            "agents/overview.md",
            "See [patterns](../agents/patterns.md#x).\n",
        );
        write(&root, "agents/patterns.md", "Patterns.\n");
        write(&root, "agents/unlinked.md", "Nobody links here.\n");
        write(&root, "CLAUDE.md", "@AGENTS.md\n");
        write(&root, "CHANGELOG.md", "# Changes\n");
        write(&root, "docs/design.md", "Design.\n");
        write(&root, ".cursor/rules/style.mdc", "Style.\n");
        write(&root, ".pragma/README.md", "pragma's own readme.\n");
        write(
            &root,
            ".pragma/specs/naming.md",
            "Name files in kebab-case.\n",
        );
        let docs = discover(&root, &DocGlobs::default()).unwrap();
        let rels: Vec<&str> = docs.iter().map(|doc| doc.rel.as_str()).collect();
        assert_eq!(
            rels,
            vec![
                ".cursor/rules/style.mdc",
                ".pragma/specs/naming.md",
                "AGENTS.md",
                "CLAUDE.md",
                "README.md",
                "agents/overview.md",
                "agents/patterns.md",
                "docs/design.md",
            ]
        );
    }

    #[test]
    fn marks_adrs_and_superseded_docs_as_historical() {
        let (_temp, root) = crate::testutil::repo();
        write(&root, "docs/adr/0001-x.md", "# ADR\n");
        write(
            &root,
            "docs/old.md",
            "---\nstatus: superseded\n---\n# Old\n",
        );
        write(
            &root,
            "docs/older.md",
            "# Older\n\n**Status:** Deprecated\n",
        );
        write(&root, "docs/current.md", "# Current\n\nStatus: accepted\n");
        let docs = discover(&root, &DocGlobs::default()).unwrap();
        let historical: Vec<&str> = docs
            .iter()
            .filter(|doc| doc.historical)
            .map(|doc| doc.rel.as_str())
            .collect();
        assert_eq!(
            historical,
            vec!["docs/adr/0001-x.md", "docs/old.md", "docs/older.md"]
        );
    }

    #[test]
    fn applies_config_globs() {
        let (_temp, root) = crate::testutil::repo();
        write(&root, "guides/style.md", "Style.\n");
        write(&root, "docs/private.md", "Private.\n");
        let globs = DocGlobs {
            include: vec!["guides/**".into()],
            exclude: vec!["docs/private.md".into()],
        };
        let rels: Vec<String> = discover(&root, &globs)
            .unwrap()
            .into_iter()
            .map(|doc| doc.rel)
            .collect();
        assert_eq!(rels, vec!["README.md", "guides/style.md"]);
    }

    #[test]
    fn skips_links_into_unwalked_or_ignored_files() {
        let (_temp, root) = crate::testutil::repo();
        write(&root, ".gitignore", "notes/\n");
        write(
            &root,
            "README.md",
            "See [dep](node_modules/x/README.md), [vendored](vendor/y/GUIDE.md) and [private](notes/private.md).\n",
        );
        write(&root, "node_modules/x/README.md", "Dependency.\n");
        write(&root, "vendor/y/GUIDE.md", "Vendored.\n");
        write(&root, "notes/private.md", "Private.\n");
        let rels: Vec<String> = discover(&root, &DocGlobs::default())
            .unwrap()
            .into_iter()
            .map(|doc| doc.rel)
            .collect();
        assert_eq!(rels, vec!["README.md"]);
    }

    #[test]
    fn keeps_live_docs_in_decision_folders() {
        let (_temp, root) = crate::testutil::repo();
        write(
            &root,
            "src/features/decisions/AGENTS.md",
            "Live guidance.\n",
        );
        write(&root, "docs/decisions/0001-x.md", "# Decision\n");
        let docs = discover(&root, &DocGlobs::default()).unwrap();
        let historical: Vec<&str> = docs
            .iter()
            .filter(|doc| doc.historical)
            .map(|doc| doc.rel.as_str())
            .collect();
        assert_eq!(historical, vec!["docs/decisions/0001-x.md"]);
        assert!(
            docs.iter()
                .any(|doc| doc.rel == "src/features/decisions/AGENTS.md" && !doc.historical)
        );
    }
}
