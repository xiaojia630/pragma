//! `.pragma/pragma.lock`: the compiled blocking clauses. Generated, committed, and the only
//! source of blocking rules.

use std::path::Path;

use anyhow::{Context, Result, bail};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::markdown::Anchor;

pub const LOCK_FORMAT: u32 = 1;
pub const LOCK_FILE: &str = "pragma.lock";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    pub format: u32,
    #[serde(default)]
    pub clauses: Vec<LockedClause>,
    /// The doc paragraphs `pragma lock` has decided on, with or without blocking clauses
    /// (§5.2), as `"<doc>#<para_hash>"`, plus `@<fingerprint>` for a doc whose frontmatter sets
    /// scope or rung (`settled_key`): for these the lock's blocking clauses are authoritative,
    /// so a later extraction cannot add or drop any until the paragraph's text or its doc's
    /// frontmatter changes. Absent in locks written by hand or before extraction; readers that
    /// do not know it ignore it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paragraphs: Vec<String>,
}

/// A paragraph's identity for the lock: its doc path, its text hash and, when the doc's
/// frontmatter sets scope or rung, that frontmatter's fingerprint (frontmatter wins over the
/// text, so a change there is a change to the paragraph's clauses).
pub fn settled_key(doc: &str, para_hash: &str, frontmatter: Option<&str>) -> String {
    match frontmatter {
        Some(fingerprint) => format!("{doc}#{para_hash}@{fingerprint}"),
        None => format!("{doc}#{para_hash}"),
    }
}

/// The doc path of a `settled_key`.
pub fn settled_doc(key: &str) -> &str {
    key.rsplit_once('#').map_or(key, |(doc, _)| doc)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LockedClause {
    pub id: String,
    pub anchor: Anchor,
    pub text: String,
    #[serde(default)]
    pub scope: Vec<String>,
    pub check: Check,
    pub fix: String,
    /// `extracted` when `pragma lock` took the clause from the docs; absent (or any other
    /// value) for a clause written by hand, which `pragma lock` never touches. Absent in every
    /// lock written before extraction, so lock format 1 still reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<LockOrigin>,
    /// The fingerprint of the doc's frontmatter scope and rung when `pragma lock` took the
    /// clause, absent when the doc set neither: part of the paragraph's identity
    /// (`settled_key`), so editing that frontmatter re-derives the clause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmatter: Option<String>,
}

impl LockedClause {
    /// Taken from the docs by `pragma lock`, which may replace or drop it; anything else is
    /// hand-written and never touched.
    pub fn is_extracted(&self) -> bool {
        self.origin == Some(LockOrigin::Extracted)
    }

    /// The identity of the paragraph the clause was taken from (`settled_key`).
    pub fn paragraph_key(&self) -> String {
        settled_key(
            &self.anchor.path,
            &self.anchor.para_hash,
            self.frontmatter.as_deref(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LockOrigin {
    Extracted,
    /// A value this pragma does not know (a typo, or a later pragma's), kept as written: the
    /// clause counts as hand-written, so one bad value cannot make the whole lock unreadable,
    /// which would let every edit through.
    #[serde(untagged)]
    Other(serde_json::Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Check {
    Library(LibraryRule),
    /// A generated ast-grep pattern that earned blocking through the backtest (S8).
    Pattern {
        pattern: String,
        examples: Examples,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "kebab-case")]
pub enum LibraryRule {
    /// No create, modify or delete through the edit tools inside the clause's scope.
    ProtectedPaths,
    /// Files in scope must not import specifiers matching `forbid`.
    ImportBoundary { forbid: Vec<String> },
    /// An ast-grep pattern that must not appear in files in scope.
    ForbiddenPattern { pattern: String, examples: Examples },
    /// File names in scope must use `case`, and end with `suffix` when set.
    FileNaming {
        case: NamingCase,
        #[serde(default)]
        suffix: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Examples {
    pub bad: String,
    pub good: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NamingCase {
    Kebab,
    Camel,
    Pascal,
    Snake,
}

impl Lock {
    pub fn empty() -> Lock {
        Lock {
            format: LOCK_FORMAT,
            clauses: Vec::new(),
            paragraphs: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Lock> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Lock::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Lock> {
        #[derive(Deserialize)]
        struct FormatOnly {
            format: u32,
        }
        let FormatOnly { format } =
            serde_json::from_str(text).context("pragma.lock is not a pragma lock")?;
        if format > LOCK_FORMAT {
            bail!(
                "pragma.lock uses format {format}, but this pragma reads up to format \
                 {LOCK_FORMAT}; update pragma"
            );
        }
        serde_json::from_str(text).context("pragma.lock is not valid")
    }

    /// Stable bytes: clauses sorted by id, settled paragraphs sorted and deduplicated,
    /// pretty-printed, ending in a newline.
    pub fn to_json(&self) -> String {
        let mut sorted = self.clone();
        sorted.clauses.sort_by(|a, b| a.id.cmp(&b.id));
        sorted.paragraphs.sort();
        sorted.paragraphs.dedup();
        let mut text = serde_json::to_string_pretty(&sorted).expect("a lock always serializes");
        text.push('\n');
        text
    }
}

/// Where a clause applies: globs, where `!glob` excludes. No include globs means everywhere.
#[derive(Debug, Clone)]
pub struct Scope {
    include: Option<GlobSet>,
    exclude: GlobSet,
}

impl Scope {
    pub fn new(globs: &[String]) -> Result<Scope> {
        let mut include = GlobSetBuilder::new();
        let mut exclude = GlobSetBuilder::new();
        let mut has_include = false;
        for glob in globs {
            let (builder, pattern) = match glob.strip_prefix('!') {
                Some(rest) => (&mut exclude, rest),
                None => {
                    has_include = true;
                    (&mut include, glob.as_str())
                }
            };
            let compiled = GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .with_context(|| format!("bad glob {glob:?}"))?;
            builder.add(compiled);
        }
        Ok(Scope {
            include: if has_include {
                Some(include.build()?)
            } else {
                None
            },
            exclude: exclude.build()?,
        })
    }

    pub fn matches(&self, rel_path: &str) -> bool {
        self.include
            .as_ref()
            .is_none_or(|set| set.is_match(rel_path))
            && !self.exclude.is_match(rel_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clause(check: Check) -> LockedClause {
        LockedClause {
            id: "c1".into(),
            anchor: Anchor {
                path: "AGENTS.md".into(),
                heading_path: vec!["Rules".into()],
                para_hash: "abc".into(),
                lines: [3, 3],
                quote: "Never use require().".into(),
            },
            text: "Never use require().".into(),
            scope: vec!["**/*.ts".into()],
            check,
            fix: "Use import.".into(),
            origin: None,
            frontmatter: None,
        }
    }

    #[test]
    fn checks_serialize_with_kind_and_rule_tags() {
        let check = Check::Library(LibraryRule::ForbiddenPattern {
            pattern: "require($$$A)".into(),
            examples: Examples {
                bad: "require('a')".into(),
                good: "import 'a'".into(),
            },
        });
        let json = serde_json::to_value(&check).unwrap();
        assert_eq!(json["kind"], "library");
        assert_eq!(json["rule"], "forbidden-pattern");
        assert_eq!(serde_json::from_value::<Check>(json).unwrap(), check);
        let protected = serde_json::json!({ "kind": "library", "rule": "protected-paths" });
        assert_eq!(
            serde_json::from_value::<Check>(protected).unwrap(),
            Check::Library(LibraryRule::ProtectedPaths)
        );
    }

    #[test]
    fn round_trips_through_stable_json() {
        let mut lock = Lock::empty();
        lock.clauses
            .push(clause(Check::Library(LibraryRule::ProtectedPaths)));
        let text = lock.to_json();
        assert!(text.ends_with('\n'));
        assert_eq!(Lock::parse(&text).unwrap(), lock);
    }

    #[test]
    fn refuses_a_newer_format_with_a_clear_message() {
        let error = Lock::parse(r#"{"format": 99, "clauses": [{"anything": true}]}"#).unwrap_err();
        assert!(format!("{error:#}").contains("update pragma"));
    }

    #[test]
    fn an_unknown_origin_is_hand_written_and_kept_as_written() {
        let text = r#"{"format": 1, "clauses": [
            {"id": "a", "anchor": {"path": "AGENTS.md", "heading_path": [], "para_hash": "h",
             "lines": [1, 1], "quote": ""}, "text": "T.", "scope": ["src/**"],
             "check": {"kind": "library", "rule": "protected-paths"}, "fix": "F.",
             "origin": "extractd"},
            {"id": "b", "anchor": {"path": "AGENTS.md", "heading_path": [], "para_hash": "h",
             "lines": [1, 1], "quote": ""}, "text": "T.", "scope": ["src/**"],
             "check": {"kind": "library", "rule": "protected-paths"}, "fix": "F.",
             "origin": 7},
            {"id": "c", "anchor": {"path": "AGENTS.md", "heading_path": [], "para_hash": "h",
             "lines": [1, 1], "quote": ""}, "text": "T.", "scope": ["src/**"],
             "check": {"kind": "library", "rule": "protected-paths"}, "fix": "F.",
             "origin": "extracted"}
        ]}"#;
        let lock = Lock::parse(text).unwrap();
        let extracted: Vec<bool> = lock
            .clauses
            .iter()
            .map(LockedClause::is_extracted)
            .collect();
        assert_eq!(extracted, vec![false, false, true]);
        let json: serde_json::Value = serde_json::from_str(&lock.to_json()).unwrap();
        assert_eq!(json["clauses"][0]["origin"], "extractd");
        assert_eq!(json["clauses"][1]["origin"], 7);
        assert_eq!(json["clauses"][2]["origin"], "extracted");
    }

    #[test]
    fn scope_includes_and_excludes() {
        let scope = Scope::new(&["src/**/*.ts".into(), "!**/index.ts".into()]).unwrap();
        assert!(scope.matches("src/a/b.ts"));
        assert!(!scope.matches("src/a/index.ts"));
        assert!(!scope.matches("lib/b.ts"));
        assert!(Scope::new(&[]).unwrap().matches("anything/at/all.md"));
        assert!(
            Scope::new(&["!docs/**".into()])
                .unwrap()
                .matches("src/a.ts")
        );
        assert!(
            !Scope::new(&["src/*.ts".into()])
                .unwrap()
                .matches("src/deep/a.ts")
        );
    }
}
