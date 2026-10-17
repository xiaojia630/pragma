//! Audit: the code checked against the docs. Runs every clause that has a code check over the
//! whole tree instead of a diff. Its findings are the repo's baseline.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::events::now_ms;
use crate::hashing::short_hash;
use crate::rules::{CompiledLock, FileOp, Finding, Mode};

/// Files over this size are skipped: they are almost always generated.
pub const MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditFinding {
    pub id: String,
    #[serde(flatten)]
    pub finding: Finding,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReport {
    pub files_checked: usize,
    pub findings: Vec<AuditFinding>,
    /// Findings per clause id.
    pub counts: BTreeMap<String, usize>,
}

/// Files the audit reads: tracked or untracked-but-not-ignored, inside `area`, covered by some
/// clause and not generated. Huge files are kept: they still get path rules, only content rules
/// skip them.
pub fn audit_files(root: &Path, lock: &CompiledLock, area: Option<&str>) -> Vec<String> {
    let generated = generated_globs(root);
    crate::docs::walk_files(root)
        .into_iter()
        .filter(|rel| area.is_none_or(|area| in_area(rel, area)))
        .filter(|rel| lock.covers(rel) && !generated.is_match(rel))
        .collect()
}

/// True when `rel` is the file or directory `area`, or inside it.
pub fn in_area(rel: &str, area: &str) -> bool {
    let area = area.trim_end_matches('/');
    rel == area || rel.starts_with(&format!("{area}/"))
}

/// The files the audit would read that differ between the working tree and the merge base of
/// `base` and HEAD, plus untracked files that are not ignored: what a branch changed, committed
/// or not. Deleted files are dropped. Runs git, so it is for the slow path only.
pub fn changed_files(root: &Path, lock: &CompiledLock, base: &str) -> Result<Vec<String>> {
    if base.starts_with('-') {
        bail!("{base} is not a ref");
    }
    let commit = git_text(
        root,
        &["rev-parse", "--verify", "-q", &format!("{base}^{{commit}}")],
    )
    .with_context(|| format!("{base} is not a commit in this repo"))?;
    let merge_base = git_text(root, &["merge-base", commit.trim(), "HEAD"])
        .with_context(|| format!("{base} shares no history with HEAD"))?;
    let tracked = git_text(
        root,
        &["diff", "--name-only", "-z", merge_base.trim(), "--"],
    )?;
    let untracked = git_text(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let generated = generated_globs(root);
    let mut files: Vec<String> = tracked
        .split('\0')
        .chain(untracked.split('\0'))
        // Regular files only, as the audit's walk sees them: no symlinks, nothing deleted.
        .filter(|rel| {
            !rel.is_empty() && fs::symlink_metadata(root.join(rel)).is_ok_and(|meta| meta.is_file())
        })
        .filter(|rel| !crate::docs::in_skipped_dir(rel))
        .filter(|rel| lock.covers(rel) && !generated.is_match(rel))
        .map(str::to_string)
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// A git command's stdout, or an error with its stderr. It runs inside a task's worktree beside
/// the agent's own git, so it never takes an optional lock (like the index refresh a `git diff`
/// would write).
fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(["-c", "core.quotePath=false"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("running git {}", args[0]))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        match stderr.trim() {
            "" => bail!("git {} failed", args[0]),
            stderr => bail!("git {} failed: {stderr}", args[0]),
        }
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Patterns marked `linguist-generated` in the root `.gitattributes`.
pub(crate) fn generated_globs(root: &Path) -> GlobSet {
    glob_set(&generated_patterns(root))
}

/// The globs behind `generated_globs`, sorted and without repeats: what decides which files
/// count as generated, so anything cached from files it kept out is valid only for the same list.
pub(crate) fn generated_patterns(root: &Path) -> Vec<String> {
    let mut patterns = Vec::new();
    let text = fs::read_to_string(root.join(".gitattributes")).unwrap_or_default();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(pattern) = parts.next().filter(|pattern| !pattern.starts_with('#')) else {
            continue;
        };
        if !parts.any(|attr| attr == "linguist-generated" || attr == "linguist-generated=true") {
            continue;
        }
        let pattern = match pattern.strip_prefix('/') {
            Some(anchored) => anchored.to_string(),
            None if !pattern.contains('/') => format!("**/{pattern}"),
            None => pattern.to_string(),
        };
        if Glob::new(&pattern).is_ok() {
            patterns.push(pattern);
        }
    }
    patterns.sort();
    patterns.dedup();
    patterns
}

/// The matcher for `generated_patterns`.
pub(crate) fn glob_set(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

pub fn run(root: &Path, lock: &CompiledLock, area: Option<&str>) -> Result<AuditReport> {
    run_files(root, lock, &audit_files(root, lock, area))
}

/// Checks `files`, repo-relative paths from `audit_files` or `changed_files`.
pub fn run_files(root: &Path, lock: &CompiledLock, files: &[String]) -> Result<AuditReport> {
    let mut findings: Vec<Finding> = files
        .par_iter()
        .flat_map_iter(|rel| {
            let mut found = lock.path_findings(rel, FileOp::Modify, Mode::Audit);
            let small = fs::metadata(root.join(rel)).is_ok_and(|meta| meta.len() <= MAX_FILE_BYTES);
            if small && let Ok(text) = fs::read_to_string(root.join(rel)) {
                found.extend(lock.content_findings(rel, &text));
            }
            found
        })
        .collect();
    findings.sort_by(|a, b| (&a.file, a.line, &a.clause_id).cmp(&(&b.file, b.line, &b.clause_id)));
    let mut seen: HashMap<(String, String, String), usize> = HashMap::new();
    let mut counts = BTreeMap::new();
    let findings = findings
        .into_iter()
        .map(|finding| {
            let nth = seen
                .entry((
                    finding.clause_id.clone(),
                    finding.file.clone(),
                    finding.matched.clone(),
                ))
                .or_default();
            let id = short_hash(
                format!(
                    "{}|{}|{}|{nth}",
                    finding.clause_id, finding.file, finding.matched
                )
                .as_bytes(),
            );
            *nth += 1;
            *counts.entry(finding.clause_id.clone()).or_default() += 1;
            AuditFinding { id, finding }
        })
        .collect();
    Ok(AuditReport {
        files_checked: files.len(),
        findings,
        counts,
    })
}

/// Appends this run's counts to `audit-history.jsonl`, so the trend shows.
pub fn append_history(state_dir: &Path, report: &AuditReport) -> io::Result<()> {
    fs::create_dir_all(state_dir)?;
    let line = serde_json::json!({ "at_ms": now_ms(), "total": report.findings.len(), "counts": report.counts });
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("audit-history.jsonl"))?;
    writeln!(file, "{line}")
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{Check, Examples, LibraryRule, Lock, LockedClause, NamingCase};
    use crate::markdown::Anchor;

    fn clause(id: &str, scope: &[&str], check: Check) -> LockedClause {
        LockedClause {
            id: id.into(),
            anchor: Anchor {
                path: "AGENTS.md".into(),
                heading_path: vec![],
                para_hash: "p".into(),
                lines: [1, 1],
                quote: String::new(),
            },
            text: id.into(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            check,
            fix: String::new(),
            origin: None,
            frontmatter: None,
        }
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn compiled() -> CompiledLock {
        let require = Check::Library(LibraryRule::ForbiddenPattern {
            pattern: "require($$$A)".into(),
            examples: Examples {
                bad: "require('a')".into(),
                good: "import a from 'a'".into(),
            },
        });
        let naming = Check::Library(LibraryRule::FileNaming {
            case: NamingCase::Kebab,
            suffix: None,
        });
        let protected = Check::Library(LibraryRule::ProtectedPaths);
        CompiledLock::compile(&Lock {
            format: 1,
            paragraphs: vec![],
            clauses: vec![
                clause("require", &["**/*.ts"], require),
                clause("naming", &["src/**"], naming),
                clause("protected", &["src/**"], protected),
            ],
        })
        .unwrap()
    }

    #[test]
    fn finds_existing_violations_with_stable_ids_and_counts() {
        let (_temp, root) = crate::testutil::repo();
        write(
            &root,
            "src/a.ts",
            "const a = require('x');\nconst b = require('x');\n",
        );
        write(&root, "src/BadName.ts", "export {};\n");
        write(&root, "src/gen.ts", "const g = require('x');\n");
        write(&root, ".gitattributes", "src/gen.ts linguist-generated\n");
        let report = run(&root, &compiled(), None).unwrap();
        assert_eq!(report.files_checked, 2);
        assert_eq!(
            report.counts,
            BTreeMap::from([("naming".to_string(), 1), ("require".to_string(), 2)])
        );
        let ids: Vec<&str> = report.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids.len(), 3);
        assert_ne!(
            ids[1], ids[2],
            "two identical matches in one file get different ids"
        );
        assert_eq!(
            run(&root, &compiled(), None).unwrap(),
            report,
            "ids are stable across runs"
        );
    }

    #[test]
    fn limits_to_an_area_and_skips_large_files() {
        let (_temp, root) = crate::testutil::repo();
        write(&root, "src/a.ts", "require('x');\n");
        write(&root, "lib/b.ts", "require('x');\n");
        write(&root, "src/big.ts", &"// filler\n".repeat(60_000));
        let report = run(&root, &compiled(), Some("lib/")).unwrap();
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].finding.file, "lib/b.ts");
    }

    #[test]
    fn large_files_still_get_path_rules_but_not_content_rules() {
        let (_temp, root) = crate::testutil::repo();
        let body = format!("require('x');\n{}", "// filler\n".repeat(60_000));
        write(&root, "src/BigName.ts", &body);
        let report = run(&root, &compiled(), None).unwrap();
        assert_eq!(report.files_checked, 1);
        assert_eq!(
            report.counts,
            BTreeMap::from([("naming".to_string(), 1)]),
            "the naming rule fires, the content rule does not"
        );
    }

    #[test]
    fn appends_history_lines() {
        let temp = tempfile::tempdir().unwrap();
        let report = AuditReport {
            files_checked: 1,
            findings: vec![],
            counts: BTreeMap::from([("c".into(), 0)]),
        };
        append_history(temp.path(), &report).unwrap();
        append_history(temp.path(), &report).unwrap();
        let text = fs::read_to_string(temp.path().join("audit-history.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("\"total\":0"));
    }
}
