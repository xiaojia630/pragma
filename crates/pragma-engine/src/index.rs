//! `<git dir>/pragma/index.json`: a checkout's full clause index, every rung, rebuilt by
//! `pragma extract`. Blocking clauses here are proposals; only the lock blocks.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::extract::Rung;
use crate::lock::{Check, settled_key};
use crate::markdown::Anchor;
use crate::writes::write_atomic;

pub const INDEX_FILE: &str = "index.json";
/// 2 adds `paragraphs`, which `pragma lock` needs; 3 marks each paragraph's frontmatter
/// fingerprint and the copies that collapsed into another doc's. An older index does not load
/// and is rebuilt.
pub const INDEX_FORMAT: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub format: u32,
    pub extractor: u32,
    pub model: String,
    /// Milliseconds since the Unix epoch.
    pub generated_at: u64,
    /// Every paragraph the run covered, with or without clauses, failed ones included: what
    /// `pragma lock` reads to tell a paragraph that still exists from one that is gone.
    pub paragraphs: Vec<IndexedParagraph>,
    /// Closer docs first (precedence), then by doc path and position.
    pub clauses: Vec<IndexedClause>,
    /// Paragraphs with no answer this run, and docs that could not be read; they are retried
    /// on the next.
    #[serde(default)]
    pub failed: Vec<FailedParagraph>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexedClause {
    pub id: String,
    pub anchor: Anchor,
    pub text: String,
    pub rung: Rung,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub check: Option<Check>,
    pub fix: String,
    pub origin: Origin,
    /// Why a proposed check did not make the clause blocking: it failed validation (the
    /// clause has no check), or its scope covers no file in the repo (R8; the check is kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_reason: Option<String>,
}

/// Where a clause came from: the repo's docs (the pack arrives in S8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedParagraph {
    pub path: String,
    pub para_hash: String,
    pub heading_path: Vec<String>,
    /// The fingerprint of the doc's frontmatter scope and rung, when it sets either.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmatter: Option<String>,
    /// The doc whose identical paragraph (same text and scope basis) answered for this one:
    /// its clauses are listed there, so this copy has not answered for itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_as: Option<String>,
}

impl IndexedParagraph {
    /// Its identity for the lock (`settled_key`).
    pub fn key(&self) -> String {
        settled_key(&self.path, &self.para_hash, self.frontmatter.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailedParagraph {
    pub path: String,
    /// Empty when the whole doc could not be read.
    pub para_hash: String,
    /// `[0, 0]` when the whole doc could not be read.
    pub lines: [u32; 2],
    pub reason: String,
}

/// Clause counts per rung.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RungCounts {
    pub blocking: usize,
    pub judged: usize,
    pub context: usize,
}

impl Index {
    /// The checkout's index, or None when there is none or it does not read (it is rebuilt).
    pub fn load(state_dir: &Path) -> Option<Index> {
        let text = std::fs::read_to_string(state_dir.join(INDEX_FILE)).ok()?;
        serde_json::from_str::<Index>(&text)
            .ok()
            .filter(|index| index.format == INDEX_FORMAT)
    }

    pub fn save(&self, state_dir: &Path) -> io::Result<()> {
        let mut text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        text.push('\n');
        write_atomic(&state_dir.join(INDEX_FILE), text.as_bytes())
    }

    /// The index restricted to the docs `keep` accepts. A kept copy whose answering copy is
    /// dropped keeps its `same_as`: that copy's clauses are gone with it, so the lock update
    /// leaves the kept copy's settled state as it is.
    pub fn retain_docs(&mut self, keep: impl Fn(&str) -> bool) {
        self.paragraphs.retain(|para| keep(&para.path));
        self.clauses.retain(|clause| keep(&clause.anchor.path));
        self.failed.retain(|failed| keep(&failed.path));
    }

    pub fn counts(&self) -> RungCounts {
        let mut counts = RungCounts::default();
        for clause in &self.clauses {
            match clause.rung {
                Rung::Blocking => counts.blocking += 1,
                Rung::Judged => counts.judged += 1,
                Rung::Context => counts.context += 1,
            }
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::LibraryRule;

    fn clause(id: &str, rung: Rung, check: Option<Check>) -> IndexedClause {
        IndexedClause {
            id: id.into(),
            anchor: Anchor {
                path: "AGENTS.md".into(),
                heading_path: vec!["Rules".into()],
                para_hash: "abc".into(),
                lines: [3, 4],
                quote: "Do not hand-edit migrations.".into(),
            },
            text: "Do not hand-edit migrations.".into(),
            rung,
            scope: vec!["drizzle/**".into()],
            check,
            fix: "Run the generator.".into(),
            origin: Origin::Repo,
            dropped_reason: None,
        }
    }

    #[test]
    fn saves_loads_and_counts() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("pragma");
        assert_eq!(Index::load(&state), None);
        let index = Index {
            format: INDEX_FORMAT,
            extractor: 1,
            model: "haiku".into(),
            generated_at: 42,
            paragraphs: vec![],
            clauses: vec![
                clause(
                    "a",
                    Rung::Blocking,
                    Some(Check::Library(LibraryRule::ProtectedPaths)),
                ),
                clause("b", Rung::Judged, None),
                clause("c", Rung::Judged, None),
            ],
            failed: vec![],
        };
        index.save(&state).unwrap();
        assert_eq!(Index::load(&state), Some(index.clone()));
        assert_eq!(
            index.counts(),
            RungCounts {
                blocking: 1,
                judged: 2,
                context: 0
            }
        );
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(state.join(INDEX_FILE)).unwrap())
                .unwrap();
        assert_eq!(json["clauses"][0]["rung"], "blocking");
        assert_eq!(json["clauses"][0]["origin"], "repo");
        assert_eq!(json["clauses"][0]["check"]["rule"], "protected-paths");
        assert_eq!(json["clauses"][1]["check"], serde_json::Value::Null);
    }

    #[test]
    fn a_corrupt_or_newer_index_does_not_load() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(INDEX_FILE), "{ nope").unwrap();
        assert_eq!(Index::load(temp.path()), None);
        std::fs::write(
            temp.path().join(INDEX_FILE),
            r#"{"format": 99, "extractor": 1, "model": "x", "generated_at": 0, "paragraphs": [], "clauses": []}"#,
        )
        .unwrap();
        assert_eq!(Index::load(temp.path()), None);
    }
}
