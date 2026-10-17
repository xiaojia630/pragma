//! The lock's clauses compiled for repeated use, and the findings they produce.

use std::collections::HashSet;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::lang::CodeLang;
use crate::lock::{Check, Examples, LibraryRule, Lock, LockedClause, NamingCase, Scope};
use crate::matcher::{self, ContentRule};

/// A file's content findings with the byte span of each, and its top-level statements.
#[derive(Debug, Clone, Default)]
pub struct ContentScan {
    pub findings: Vec<(Finding, (usize, usize))>,
    pub top_level: Vec<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub clause_id: String,
    pub file: String,
    /// 1-based; 0 for findings about the file itself (protected paths, naming).
    pub line: u32,
    /// The normalized matched text: the key that decides whether a violation is new.
    pub matched: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Create,
    Modify,
    Delete,
}

/// Where findings are wanted: during an edit, over a turn's diff, or over the whole tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Edit,
    Diff,
    Audit,
}

pub struct CompiledLock {
    clauses: Vec<CompiledClause>,
}

struct CompiledClause {
    clause: LockedClause,
    scope: Scope,
    forbid: Option<Scope>,
    words: Vec<String>,
}

impl CompiledLock {
    pub fn compile(lock: &Lock) -> Result<CompiledLock> {
        let clauses = lock
            .clauses
            .iter()
            .map(|clause| {
                let forbid = match &clause.check {
                    Check::Library(LibraryRule::ImportBoundary { forbid }) => {
                        Some(Scope::new(forbid)?)
                    }
                    _ => None,
                };
                Ok(CompiledClause {
                    clause: clause.clone(),
                    scope: Scope::new(&clause.scope)?,
                    forbid,
                    words: pattern_of(&clause.check)
                        .map(matcher::pattern_words)
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(CompiledLock { clauses })
    }

    pub fn clause(&self, id: &str) -> Option<&LockedClause> {
        self.clauses().find(|clause| clause.id == id)
    }

    pub fn clauses(&self) -> impl Iterator<Item = &LockedClause> {
        self.clauses.iter().map(|compiled| &compiled.clause)
    }

    /// True when some clause's scope covers `rel_path`.
    pub fn covers(&self, rel_path: &str) -> bool {
        self.clauses
            .iter()
            .any(|compiled| compiled.scope.matches(rel_path))
    }

    /// True when a content rule (a pattern or an import boundary) applies to `rel_path`, so its
    /// text is worth reading at all.
    pub fn checks_content(&self, rel_path: &str) -> bool {
        CodeLang::for_path(rel_path).is_some()
            && self.clauses.iter().any(|compiled| {
                compiled.scope.matches(rel_path)
                    && (pattern_of(&compiled.clause.check).is_some() || compiled.forbid.is_some())
            })
    }

    /// Findings that come from the path alone: protected paths (edits only) and file naming
    /// (created files, or every file in an audit).
    pub fn path_findings(&self, rel_path: &str, op: FileOp, mode: Mode) -> Vec<Finding> {
        self.clauses
            .iter()
            .filter(|compiled| compiled.scope.matches(rel_path))
            .filter(|compiled| match &compiled.clause.check {
                Check::Library(LibraryRule::ProtectedPaths) => mode == Mode::Edit,
                Check::Library(LibraryRule::FileNaming { case, suffix }) => {
                    op != FileOp::Delete
                        && (op == FileOp::Create || mode == Mode::Audit)
                        && !name_ok(rel_path, *case, suffix.as_deref())
                }
                _ => false,
            })
            .map(|compiled| Finding {
                clause_id: compiled.clause.id.clone(),
                file: rel_path.to_string(),
                line: 0,
                matched: rel_path.to_string(),
            })
            .collect()
    }

    /// Findings from patterns and import boundaries in a file's text. Parses only when a rule
    /// could match.
    pub fn content_findings(&self, rel_path: &str, text: &str) -> Vec<Finding> {
        let scan = self.content_scan(rel_path, text);
        scan.findings
            .into_iter()
            .map(|(finding, _)| finding)
            .collect()
    }

    /// Like `content_findings`, keeping each finding's byte span and the file's top-level
    /// statements, which the edit check needs to look only at what an edit touched.
    pub fn content_scan(&self, rel_path: &str, text: &str) -> ContentScan {
        let Some(lang) = CodeLang::for_path(rel_path) else {
            return ContentScan::default();
        };
        let rules: Vec<ContentRule> = self
            .clauses
            .iter()
            .enumerate()
            .filter(|(_, compiled)| compiled.scope.matches(rel_path))
            .filter_map(
                |(index, compiled)| match pattern_of(&compiled.clause.check) {
                    Some(pattern) => Some(ContentRule::Pattern {
                        index,
                        pattern,
                        words: &compiled.words,
                    }),
                    None => compiled
                        .forbid
                        .as_ref()
                        .map(|forbid| ContentRule::Imports { index, forbid }),
                },
            )
            .collect();
        if rules.is_empty() {
            return ContentScan::default();
        }
        let scan = matcher::scan(lang, rel_path, text, &rules);
        let findings = scan
            .matches
            .into_iter()
            .map(|found| {
                let finding = Finding {
                    clause_id: self.clauses[found.index].clause.id.clone(),
                    file: rel_path.to_string(),
                    line: found.line,
                    matched: found.matched,
                };
                (finding, found.span)
            })
            .collect();
        ContentScan {
            findings,
            top_level: scan.top_level,
        }
    }
}

fn pattern_of(check: &Check) -> Option<&str> {
    match check {
        Check::Library(LibraryRule::ForbiddenPattern { pattern, .. })
        | Check::Pattern { pattern, .. } => Some(pattern),
        _ => None,
    }
}

fn name_ok(rel_path: &str, case: NamingCase, suffix: Option<&str>) -> bool {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    if name.starts_with('.') {
        return true;
    }
    if suffix.is_some_and(|suffix| !name.ends_with(suffix)) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name);
    let joined_by = |separator: char| {
        !stem.is_empty()
            && stem.split(separator).all(|part| {
                !part.is_empty()
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            })
    };
    match case {
        NamingCase::Kebab => joined_by('-'),
        NamingCase::Snake => joined_by('_'),
        NamingCase::Camel => {
            stem.starts_with(|c: char| c.is_ascii_lowercase())
                && stem.chars().all(|c| c.is_ascii_alphanumeric())
        }
        NamingCase::Pascal => {
            stem.starts_with(|c: char| c.is_ascii_uppercase())
                && stem.chars().all(|c| c.is_ascii_alphanumeric())
        }
    }
}

/// Problems that would make the lock misbehave. Empty means the lock is sound.
pub fn validate(lock: &Lock) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    for clause in &lock.clauses {
        let id = &clause.id;
        if !seen.insert(id.as_str()) {
            problems.push(format!("{id}: duplicate clause id"));
        }
        if let Err(error) = Scope::new(&clause.scope) {
            problems.push(format!("{id}: scope: {error:#}"));
        }
        let examples = match &clause.check {
            Check::Library(LibraryRule::ForbiddenPattern { examples, .. })
            | Check::Pattern { examples, .. } => Some(examples),
            Check::Library(LibraryRule::ImportBoundary { forbid }) => {
                if let Err(error) = Scope::new(forbid) {
                    problems.push(format!("{id}: forbid: {error:#}"));
                }
                None
            }
            _ => None,
        };
        if let (Some(pattern), Some(examples)) = (pattern_of(&clause.check), examples) {
            problems.extend(
                pattern_problems(pattern, examples)
                    .into_iter()
                    .map(|problem| format!("{id}: {problem}")),
            );
        }
    }
    problems
}

/// Problems with a pattern check: it does not compile, misses its bad example, or matches its
/// good one. Empty means the pattern behaves.
pub fn pattern_problems(pattern: &str, examples: &Examples) -> Vec<String> {
    if let Some(error) = matcher::compile_error(pattern, CodeLang::TypeScript) {
        return vec![format!("the pattern does not compile: {error}")];
    }
    let words: Vec<String> = Vec::new();
    let count = |code: &str| {
        let rule = [ContentRule::Pattern {
            index: 0,
            pattern,
            words: &words,
        }];
        matcher::run(CodeLang::TypeScript, "example.ts", code, &rule).len()
    };
    let mut problems = Vec::new();
    if count(&examples.bad) == 0 {
        problems.push("the pattern does not match its bad example".to_string());
    }
    if count(&examples.good) > 0 {
        problems.push("the pattern matches its good example".to_string());
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Anchor;

    fn clause(id: &str, scope: &[&str], check: Check) -> LockedClause {
        LockedClause {
            id: id.into(),
            anchor: Anchor {
                path: "AGENTS.md".into(),
                heading_path: vec![],
                para_hash: format!("p-{id}"),
                lines: [1, 1],
                quote: String::new(),
            },
            text: format!("rule {id}"),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            check,
            fix: "fix it".into(),
            origin: None,
            frontmatter: None,
        }
    }

    fn pattern(pattern: &str, bad: &str, good: &str) -> Check {
        Check::Library(LibraryRule::ForbiddenPattern {
            pattern: pattern.into(),
            examples: Examples {
                bad: bad.into(),
                good: good.into(),
            },
        })
    }

    fn lock(clauses: Vec<LockedClause>) -> CompiledLock {
        CompiledLock::compile(&Lock {
            format: 1,
            clauses,
            paragraphs: vec![],
        })
        .unwrap()
    }

    #[test]
    fn protected_paths_fire_only_for_edits() {
        let compiled = lock(vec![clause(
            "p",
            &["drizzle/**"],
            Check::Library(LibraryRule::ProtectedPaths),
        )]);
        assert_eq!(
            compiled
                .path_findings("drizzle/0001.sql", FileOp::Modify, Mode::Edit)
                .len(),
            1
        );
        assert!(
            compiled
                .path_findings("drizzle/0001.sql", FileOp::Modify, Mode::Diff)
                .is_empty()
        );
        assert!(
            compiled
                .path_findings("src/a.ts", FileOp::Modify, Mode::Edit)
                .is_empty()
        );
    }

    #[test]
    fn file_naming_checks_created_files_and_audits() {
        let naming = Check::Library(LibraryRule::FileNaming {
            case: NamingCase::Kebab,
            suffix: None,
        });
        let compiled = lock(vec![clause("n", &["src/**"], naming)]);
        assert_eq!(
            compiled
                .path_findings("src/TaskView.tsx", FileOp::Create, Mode::Edit)
                .len(),
            1
        );
        assert!(
            compiled
                .path_findings("src/TaskView.tsx", FileOp::Modify, Mode::Edit)
                .is_empty()
        );
        assert_eq!(
            compiled
                .path_findings("src/TaskView.tsx", FileOp::Modify, Mode::Audit)
                .len(),
            1
        );
        assert!(
            compiled
                .path_findings("src/task-view.test.tsx", FileOp::Create, Mode::Edit)
                .is_empty()
        );
        assert!(
            compiled
                .path_findings("src/.eslintrc.js", FileOp::Create, Mode::Edit)
                .is_empty()
        );
    }

    #[test]
    fn content_findings_respect_scope_and_language() {
        let compiled = lock(vec![clause(
            "r",
            &["**/*.ts", "!**/legacy/**"],
            pattern("require($$$A)", "require('a')", "import a from 'a'"),
        )]);
        let text = "const a = require('a');\n";
        assert_eq!(
            compiled.content_findings("src/a.ts", text)[0].matched,
            "require('a')"
        );
        assert!(
            compiled
                .content_findings("src/legacy/a.ts", text)
                .is_empty()
        );
        assert!(compiled.content_findings("src/a.md", text).is_empty());
    }

    #[test]
    fn checks_content_only_where_a_content_rule_applies() {
        let compiled = lock(vec![
            clause(
                "r",
                &["src/**"],
                pattern("require($$$A)", "require('a')", "import a from 'a'"),
            ),
            clause("p", &[], Check::Library(LibraryRule::ProtectedPaths)),
        ]);
        assert!(compiled.checks_content("src/a.ts"));
        assert!(!compiled.checks_content("src/a.md"), "not a code language");
        assert!(
            !compiled.checks_content("lib/a.ts"),
            "only a path rule here"
        );
    }

    #[test]
    fn validate_reports_a_pattern_that_does_not_compile() {
        let malformed = clause(
            "m",
            &[],
            pattern("$$$A", "require('a');", "import a from 'a';"),
        );
        let problems = validate(&Lock {
            format: 1,
            paragraphs: vec![],
            clauses: vec![malformed],
        });
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].starts_with("m: the pattern does not compile: "),
            "{problems:?}"
        );
    }

    #[test]
    fn validate_reports_bad_examples_globs_and_duplicates() {
        let good = clause(
            "a",
            &[],
            pattern(
                "require($$$A)",
                "const x = require('a');",
                "import x from 'a';",
            ),
        );
        assert!(
            validate(&Lock {
                format: 1,
                paragraphs: vec![],
                clauses: vec![good.clone()]
            })
            .is_empty()
        );
        let wrong = clause(
            "b",
            &["[bad"],
            pattern("require($$$A)", "import x from 'a';", "require('a');"),
        );
        let problems = validate(&Lock {
            format: 1,
            paragraphs: vec![],
            clauses: vec![good.clone(), good, wrong],
        });
        assert_eq!(problems.len(), 4, "{problems:?}");
    }
}
