//! Backtest: recent history checked against today's lock. Counts the violations each commit on
//! HEAD's first-parent line introduced, so "pragma would have stopped N edits" is measured, not
//! guessed. Runs git as child processes: the slow path, never in a hook.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anyhow::{Context, Result, bail};
use globset::GlobSet;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::audit::{MAX_FILE_BYTES, generated_patterns, glob_set};
use crate::docs::in_skipped_dir;
use crate::hashing::short_hash;
use crate::introduced::introduced;
use crate::lock::{Check, LibraryRule};
use crate::overrides::{Exception, excepted};
use crate::rules::{CompiledLock, FileOp, Finding, Mode};
use crate::writes::write_atomic;

/// First-parent commits of HEAD checked when no depth is given (spec 5.7).
pub const DEFAULT_DEPTH: usize = 200;
/// The per-commit cache, in `<git common dir>/pragma/`: every worktree shares it.
pub const CACHE_FILE: &str = "backtest-cache.json";
/// The latest full report, in a checkout's own `<git dir>/pragma/`.
pub const REPORT_FILE: &str = "backtest.json";

/// Commits whose blobs are held in memory at once, so a long history stays bounded.
const CHUNK: usize = 32;
/// Cached commits kept beyond the ones a run walked, so the cache cannot grow without bound.
const CACHE_LIMIT: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestReport {
    /// The first-parent commits asked for.
    pub depth: usize,
    /// Commits diffed against their first parent: the walk without the root commit.
    pub commits_checked: usize,
    /// Of those, the commits that could not be fully checked: a file they changed is not in this
    /// clone (a partial clone), or git could not tell a rename from a new file. What they found
    /// could be wrong either way, so none of it is in `total`, `counts` or `hits`.
    #[serde(default)]
    pub incomplete_commits: usize,
    /// The full sha the walk started from; empty in a repo with no commits.
    pub head: String,
    /// Hits after exceptions.
    pub total: usize,
    /// Hits per clause id.
    pub counts: BTreeMap<String, usize>,
    /// Newest commit first, then by file and line.
    pub hits: Vec<BacktestHit>,
}

/// A violation a commit introduced that today's lock would have stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestHit {
    pub commit: String,
    /// git's abbreviation of `commit`.
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Author time, in seconds since the Unix epoch.
    pub at_s: u64,
    #[serde(flatten)]
    pub finding: Finding,
}

/// How many of the lock's clauses a backtest runs. Protected paths apply only to an edit, so
/// history never runs them: a lock of only those checks nothing.
pub fn rules_checked(lock: &CompiledLock) -> usize {
    lock.clauses()
        .filter(|clause| !matches!(clause.check, Check::Library(LibraryRule::ProtectedPaths)))
        .count()
}

/// Checks the last `depth` first-parent commits of HEAD in `root` against `lock`, through the
/// per-commit cache at `cache_path`. Exceptions apply after the cache, since they change
/// without the lock changing. A cache that cannot be written costs only the next run's time.
///
/// A commit that could not be fully checked is only counted, in `incomplete_commits`: it adds no
/// hit, and it is checked again next run instead of being cached.
pub fn run(
    root: &Path,
    lock: &CompiledLock,
    exceptions: &[Exception],
    cache_path: &Path,
    depth: usize,
) -> Result<BacktestReport> {
    let commits = first_parent_log(root, depth)?;
    // The root commit has nothing to diff against: it would count the whole initial import.
    let checked: Vec<&Commit> = commits
        .iter()
        .filter(|commit| commit.parent.is_some())
        .collect();
    let patterns = generated_patterns(root);
    let generated = glob_set(&patterns);
    let mut cache = Cache::load(cache_path, rules_key(lock, &patterns));
    let todo: Vec<&Commit> = checked
        .iter()
        .copied()
        .filter(|commit| !cache.commits.contains_key(&commit.sha))
        .collect();
    let mut incomplete_commits = 0;
    let checked_now = check_commits(root, lock, &generated, &todo)?;
    for (commit, found) in todo.iter().zip(checked_now) {
        match found {
            Some(findings) => {
                cache.commits.insert(commit.sha.clone(), findings);
            }
            None => incomplete_commits += 1,
        }
    }

    let para_of = |clause_id: &str| {
        lock.clause(clause_id)
            .map(|clause| clause.anchor.para_hash.clone())
            .unwrap_or_default()
    };
    let mut hits = Vec::new();
    for commit in &checked {
        let Some(findings) = cache.commits.get(&commit.sha) else {
            continue;
        };
        let mut findings: Vec<&Finding> = findings
            .iter()
            .filter(|finding| !excepted(exceptions, finding, &para_of(&finding.clause_id)))
            .collect();
        findings.sort_by(|a, b| {
            (&a.file, a.line, &a.clause_id, &a.matched).cmp(&(
                &b.file,
                b.line,
                &b.clause_id,
                &b.matched,
            ))
        });
        hits.extend(findings.into_iter().map(|finding| BacktestHit {
            commit: commit.sha.clone(),
            short: commit.short.clone(),
            subject: commit.subject.clone(),
            author: commit.author.clone(),
            at_s: commit.at_s,
            finding: finding.clone(),
        }));
    }
    let mut counts = BTreeMap::new();
    for hit in &hits {
        *counts.entry(hit.finding.clause_id.clone()).or_default() += 1;
    }
    if !todo.is_empty() {
        let _ = cache.save(cache_path, &checked);
    }
    Ok(BacktestReport {
        depth,
        commits_checked: checked.len(),
        incomplete_commits,
        head: commits
            .first()
            .map(|commit| commit.sha.clone())
            .unwrap_or_default(),
        total: hits.len(),
        counts,
        hits,
    })
}

struct Commit {
    sha: String,
    short: String,
    /// The first parent; None for a root commit.
    parent: Option<String>,
    author: String,
    at_s: u64,
    subject: String,
}

/// A file a commit added, changed or renamed, as of that commit.
struct Change {
    path: String,
    /// None for an added file.
    old_blob: Option<String>,
    new_blob: String,
    /// Added or renamed: the path itself is new, so path rules apply.
    created: bool,
    /// The path before a rename.
    old_path: Option<String>,
}

enum Blob {
    Text(String),
    /// Not UTF-8, not a blob, or over `MAX_FILE_BYTES`: content rules skip it.
    Skipped,
    /// Not in the local object store (a partial clone): the commit is not fully checked.
    Missing,
}

/// `git` in `root`, never fetching a missing object from a promisor remote (history is checked
/// as far as it is local, with no network) and never taking an optional lock: it runs inside a
/// task's worktree beside the agent's own git.
fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "core.quotePath=false",
            "-c",
            "log.showSignature=false",
        ])
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root);
    command
}

fn first_parent_log(root: &Path, depth: usize) -> Result<Vec<Commit>> {
    if depth == 0 {
        return Ok(Vec::new());
    }
    let output = git(root)
        .args(["log", "--first-parent", "-n", &depth.to_string()])
        .arg("--format=%H%x1f%h%x1f%P%x1f%an%x1f%at%x1f%s%x1e")
        .args(["HEAD", "--"])
        .output()
        .context("running git log")?;
    if !output.status.success() {
        let has_head = git(root)
            .args(["rev-parse", "--verify", "-q", "HEAD^{commit}"])
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !has_head {
            return Ok(Vec::new());
        }
        bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .split('\x1e')
        .map(|record| record.trim_start_matches('\n'))
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let fields: Vec<&str> = record.splitn(6, '\x1f').collect();
            let [sha, short, parents, author, at_s, subject] = fields.as_slice() else {
                return None;
            };
            Some(Commit {
                sha: sha.to_string(),
                short: short.to_string(),
                parent: parents.split_whitespace().next().map(str::to_string),
                author: author.to_string(),
                at_s: at_s.parse().unwrap_or(0),
                subject: subject.to_string(),
            })
        })
        .collect())
}

/// The regular files `sha` added, changed or renamed against `parent`, and whether they are
/// all there is to know. Deletions count nothing.
///
/// Scoring renames reads blobs, which a partial clone may not have, and with no lazy fetch the
/// diff then fails. It is tried again without renames, which needs only trees: a rename shows as
/// a new file, so that commit is not exact. If that fails too the commit has no files. Either way
/// the commit is not complete, and one commit never fails the run.
fn commit_changes(root: &Path, parent: &str, sha: &str) -> (Vec<Change>, bool) {
    if let Ok(changes) = diff_tree(root, parent, sha, true) {
        return (changes, true);
    }
    (
        diff_tree(root, parent, sha, false).unwrap_or_default(),
        false,
    )
}

fn diff_tree(root: &Path, parent: &str, sha: &str, renames: bool) -> Result<Vec<Change>> {
    let output = git(root)
        .args(["diff-tree", "-r", "-z", "--raw", "--no-abbrev"])
        .arg(if renames { "-M" } else { "--no-renames" })
        .args(["--no-commit-id", parent, sha])
        .output()
        .context("running git diff-tree")?;
    if !output.status.success() {
        bail!(
            "git diff-tree {sha} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_raw(&String::from_utf8_lossy(&output.stdout)))
}

/// Parses `--raw -z` output: `:<old mode> <new mode> <old> <new> <status>`, then the path, or
/// the old and new paths for a rename or copy.
fn parse_raw(text: &str) -> Vec<Change> {
    let mut tokens = text.split('\0');
    let mut changes = Vec::new();
    while let Some(meta) = tokens.next() {
        let Some(meta) = meta.strip_prefix(':') else {
            continue;
        };
        let fields: Vec<&str> = meta.split(' ').collect();
        let [_, new_mode, old_blob, new_blob, status] = fields.as_slice() else {
            continue;
        };
        let kind = status.chars().next().unwrap_or(' ');
        let mut path = tokens.next().unwrap_or_default();
        let mut old_path = None;
        if matches!(kind, 'R' | 'C') {
            old_path = Some(path.to_string());
            path = tokens.next().unwrap_or_default();
        }
        let (old_blob, created) = match kind {
            'A' => (None, true),
            'M' => (Some(old_blob.to_string()), false),
            'R' => (Some(old_blob.to_string()), true),
            _ => continue,
        };
        // Symlinks and submodules have no text to check.
        if !matches!(*new_mode, "100644" | "100755") {
            continue;
        }
        changes.push(Change {
            path: path.to_string(),
            old_blob,
            new_blob: new_blob.to_string(),
            created,
            old_path,
        });
    }
    changes
}

/// Each commit's findings before exceptions, or None when it could not be fully checked: a blob
/// it needed was not local, or its renames could not be scored. What such a commit found could
/// be wrong either way, so nothing of it is kept, and it is checked again next run.
fn check_commits(
    root: &Path,
    lock: &CompiledLock,
    generated: &GlobSet,
    commits: &[&Commit],
) -> Result<Vec<Option<Vec<Finding>>>> {
    // What the audit would read: nothing under its skipped directories, nothing generated.
    let changes: Vec<(Vec<Change>, bool)> = commits
        .par_iter()
        .map(|commit| {
            let parent = commit.parent.as_deref().unwrap_or_default();
            let (changes, exact) = commit_changes(root, parent, &commit.sha);
            let kept = changes
                .into_iter()
                .filter(|change| {
                    !in_skipped_dir(&change.path)
                        && lock.covers(&change.path)
                        && !generated.is_match(&change.path)
                })
                .collect();
            (kept, exact)
        })
        .collect();
    let mut reader: Option<BlobReader> = None;
    let mut results = Vec::with_capacity(commits.len());
    for chunk in changes.chunks(CHUNK) {
        let mut blobs: HashMap<String, Blob> = HashMap::new();
        for change in chunk.iter().flat_map(|(changes, _)| changes) {
            if !lock.checks_content(&change.path) {
                continue;
            }
            for sha in change.old_blob.iter().chain([&change.new_blob]) {
                if !blobs.contains_key(sha) {
                    let reader = match &mut reader {
                        Some(reader) => reader,
                        None => reader.insert(BlobReader::start(root)?),
                    };
                    blobs.insert(sha.clone(), reader.read(sha)?);
                }
            }
        }
        let files: Vec<(usize, &Change)> = chunk
            .iter()
            .enumerate()
            .flat_map(|(index, (changes, _))| changes.iter().map(move |change| (index, change)))
            .collect();
        let checked: Vec<(usize, Vec<Finding>, bool)> = files
            .par_iter()
            .map(|(index, change)| {
                let (found, complete) = check_change(lock, change, &blobs);
                (*index, found, complete)
            })
            .collect();
        let mut chunk_results: Vec<(Vec<Finding>, bool)> = chunk
            .iter()
            .map(|(_, exact)| (Vec::new(), *exact))
            .collect();
        for (index, found, complete) in checked {
            let (findings, all_local) = &mut chunk_results[index];
            findings.extend(found);
            *all_local &= complete;
        }
        results.extend(
            chunk_results
                .into_iter()
                .map(|(findings, complete)| complete.then_some(findings)),
        );
    }
    Ok(results)
}

/// What one changed file introduced: path rules for a new path (audit mode, so edit-only rules
/// never run), and content findings the new version has beyond the old one's.
fn check_change(
    lock: &CompiledLock,
    change: &Change,
    blobs: &HashMap<String, Blob>,
) -> (Vec<Finding>, bool) {
    let mut found = if change.created {
        // A rename introduces only the path rules its old path did not already break.
        let broken: HashSet<String> = change
            .old_path
            .iter()
            .flat_map(|old| lock.path_findings(old, FileOp::Create, Mode::Audit))
            .map(|finding| finding.clause_id)
            .collect();
        lock.path_findings(&change.path, FileOp::Create, Mode::Audit)
            .into_iter()
            .filter(|finding| !broken.contains(&finding.clause_id))
            .collect()
    } else {
        Vec::new()
    };
    if !lock.checks_content(&change.path) {
        return (found, true);
    }
    let text = |sha: &str| match blobs.get(sha) {
        Some(Blob::Text(text)) => Ok(Some(text.as_str())),
        Some(Blob::Skipped) => Ok(None),
        Some(Blob::Missing) | None => Err(()),
    };
    let old = match &change.old_blob {
        None => Ok(Some("")),
        Some(sha) => text(sha),
    };
    let (Ok(new), Ok(old)) = (text(&change.new_blob), old) else {
        return (found, false);
    };
    let (Some(new), Some(old)) = (new, old) else {
        return (found, true);
    };
    let after = lock.content_findings(&change.path, new);
    if !after.is_empty() {
        let before = lock.content_findings(&change.path, old);
        found.extend(introduced(&before, &after, old, new));
    }
    (found, true)
}

/// One `git cat-file --batch` process for a whole run: a request is a sha and a newline, an
/// answer is `<sha> <type> <size>`, the bytes and a newline.
struct BlobReader {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl BlobReader {
    fn start(root: &Path) -> Result<BlobReader> {
        let mut child = git(root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("running git cat-file")?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().context("git cat-file has no stdout")?;
        Ok(BlobReader {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    fn read(&mut self, sha: &str) -> Result<Blob> {
        let stdin = self.stdin.as_mut().context("git cat-file is closed")?;
        writeln!(stdin, "{sha}").context("writing to git cat-file")?;
        stdin.flush().context("writing to git cat-file")?;
        let mut header = String::new();
        if self.stdout.read_line(&mut header)? == 0 {
            bail!("git cat-file stopped answering");
        }
        let fields: Vec<&str> = header.split_whitespace().collect();
        // `<sha> missing` (or `ambiguous`) has no size, and no body follows.
        let [_, kind, size] = fields.as_slice() else {
            return Ok(Blob::Missing);
        };
        let size: u64 = size.parse().context("reading git cat-file's answer")?;
        if *kind != "blob" || size > MAX_FILE_BYTES {
            io::copy(&mut (&mut self.stdout).take(size + 1), &mut io::sink())?;
            return Ok(Blob::Skipped);
        }
        let mut bytes = vec![0; size as usize + 1];
        self.stdout.read_exact(&mut bytes)?;
        bytes.pop();
        Ok(String::from_utf8(bytes).map_or(Blob::Skipped, Blob::Text))
    }
}

impl Drop for BlobReader {
    fn drop(&mut self) {
        // Closing stdin ends the batch; waiting reaps the process.
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

/// Raw findings per commit, valid only for the rules they were found with.
#[derive(Serialize, Deserialize)]
struct Cache {
    rules: String,
    commits: BTreeMap<String, Vec<Finding>>,
}

impl Cache {
    /// The cache at `path`; empty when it is missing, unreadable or for other rules.
    fn load(path: &Path, rules: String) -> Cache {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Cache>(&text).ok())
            .filter(|cache| cache.rules == rules)
            .unwrap_or(Cache {
                rules,
                commits: BTreeMap::new(),
            })
    }

    /// Writes the cache, keeping every commit `walked` and as many others as fit.
    fn save(&mut self, path: &Path, walked: &[&Commit]) -> Result<()> {
        let keep: HashSet<&str> = walked.iter().map(|commit| commit.sha.as_str()).collect();
        let mut room = CACHE_LIMIT.saturating_sub(keep.len());
        self.commits.retain(|sha, _| {
            if keep.contains(sha.as_str()) {
                return true;
            }
            let fits = room > 0;
            room = room.saturating_sub(1);
            fits
        });
        write_atomic(path, serde_json::to_string(self)?.as_bytes())
            .with_context(|| format!("writing {}", path.display()))
    }
}

/// What the cached findings depend on: each clause's id, scope and check, the patterns that
/// kept generated files out, and the engine version, whose matcher found them. A clause's
/// wording and anchor change no finding, so they are not in the key. An extracted clause's id
/// comes from its paragraph's hash, though: editing the paragraph gives it a new id and the
/// cache is recomputed. Only rewording a clause through `overrides.md`, which keeps its id,
/// keeps the cache.
fn rules_key(lock: &CompiledLock, generated: &[String]) -> String {
    let mut clauses: Vec<_> = lock
        .clauses()
        .map(|clause| (&clause.id, &clause.scope, &clause.check))
        .collect();
    clauses.sort_by(|a, b| a.0.cmp(b.0));
    let json = serde_json::to_string(&(clauses, generated)).expect("clauses always serialize");
    short_hash(format!("{}\n{json}", env!("CARGO_PKG_VERSION")).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{Check, Examples, LibraryRule, Lock, LockedClause, NamingCase};
    use crate::markdown::Anchor;
    use crate::overrides::Reason;
    use crate::testutil::{git, repo, supports_no_lazy_fetch};
    use std::path::PathBuf;

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
            text: id.into(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            check,
            fix: String::new(),
            origin: None,
            frontmatter: None,
        }
    }

    fn require_clause(scope: &[&str]) -> LockedClause {
        clause(
            "require",
            scope,
            Check::Library(LibraryRule::ForbiddenPattern {
                pattern: "require($$$A)".into(),
                examples: Examples {
                    bad: "require('a')".into(),
                    good: "import a from 'a'".into(),
                },
            }),
        )
    }

    /// No `require()` in TypeScript, kebab-case names under `src/`, and `src/` protected from
    /// edits (an edit-only rule, which history never runs).
    fn lock() -> CompiledLock {
        compile(vec![
            require_clause(&["**/*.ts"]),
            clause(
                "naming",
                &["src/**"],
                Check::Library(LibraryRule::FileNaming {
                    case: NamingCase::Kebab,
                    suffix: None,
                }),
            ),
            clause(
                "protected",
                &["src/**"],
                Check::Library(LibraryRule::ProtectedPaths),
            ),
        ])
    }

    fn compile(clauses: Vec<LockedClause>) -> CompiledLock {
        CompiledLock::compile(&Lock {
            format: 1,
            paragraphs: vec![],
            clauses,
        })
        .unwrap()
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn commit(root: &Path, files: &[(&str, &str)], message: &str) {
        for (rel, text) in files {
            write(root, rel, text);
        }
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "--allow-empty", "-m", message]);
    }

    fn cache_path(root: &Path) -> PathBuf {
        root.join(".git/pragma").join(CACHE_FILE)
    }

    fn backtest(root: &Path, lock: &CompiledLock, depth: usize) -> BacktestReport {
        run(root, lock, &[], &cache_path(root), depth).unwrap()
    }

    /// Each hit as (subject, clause, file, line).
    fn hits(report: &BacktestReport) -> Vec<(String, String, String, u32)> {
        report
            .hits
            .iter()
            .map(|hit| {
                (
                    hit.subject.clone(),
                    hit.finding.clause_id.clone(),
                    hit.finding.file.clone(),
                    hit.finding.line,
                )
            })
            .collect()
    }

    fn hit(subject: &str, clause: &str, file: &str, line: u32) -> (String, String, String, u32) {
        (subject.into(), clause.into(), file.into(), line)
    }

    fn rev_parse(root: &Path, rev: &str) -> String {
        let output = Command::new("git")
            .args(["rev-parse", rev])
            .current_dir(root)
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn a_commit_adding_a_violation_counts_once() {
        let (_temp, root) = repo();
        commit(&root, &[("src/a.ts", "const a = require('x');\n")], "add a");
        let report = backtest(&root, &lock(), 200);
        assert_eq!(report.depth, 200);
        assert_eq!(report.commits_checked, 1, "the root commit is not checked");
        assert_eq!(report.incomplete_commits, 0);
        assert_eq!(report.head, rev_parse(&root, "HEAD"));
        assert_eq!(hits(&report), vec![hit("add a", "require", "src/a.ts", 1)]);
        assert_eq!(report.total, 1);
        assert_eq!(report.counts, BTreeMap::from([("require".to_string(), 1)]));
        let first = &report.hits[0];
        assert_eq!(first.commit, report.head);
        assert!(report.head.starts_with(&first.short));
        assert_eq!(first.author, "pragma");
        assert!(first.at_s > 1_600_000_000);
        assert_eq!(first.finding.matched, "require('x')");
    }

    #[test]
    fn moving_a_violation_within_a_file_counts_nothing() {
        let (_temp, root) = repo();
        let f = "export function f() {\n  return require('x');\n}\n";
        let g = "export function g() {\n  return 1;\n}\n";
        commit(&root, &[("src/a.ts", &format!("{f}{g}"))], "add");
        commit(&root, &[("src/a.ts", &format!("{g}{f}"))], "move");
        assert_eq!(
            hits(&backtest(&root, &lock(), 200)),
            vec![hit("add", "require", "src/a.ts", 2)]
        );
    }

    #[test]
    fn removing_a_violation_counts_nothing() {
        let (_temp, root) = repo();
        commit(&root, &[("src/a.ts", "const a = require('x');\n")], "add");
        commit(&root, &[("src/a.ts", "import a from 'x';\n")], "fix");
        fs::remove_file(root.join("src/a.ts")).unwrap();
        commit(&root, &[], "delete");
        assert_eq!(
            hits(&backtest(&root, &lock(), 200)),
            vec![hit("add", "require", "src/a.ts", 1)]
        );
    }

    #[test]
    fn a_merge_counts_its_branch_once_against_its_first_parent() {
        let (_temp, root) = repo();
        git(&root, &["checkout", "-q", "-b", "feature"]);
        commit(
            &root,
            &[("src/feature.ts", "require('x');\n")],
            "feature work",
        );
        git(&root, &["checkout", "-q", "main"]);
        commit(&root, &[("src/other.ts", "export {};\n")], "main work");
        git(
            &root,
            &["merge", "-q", "--no-ff", "feature", "-m", "merge feature"],
        );
        let report = backtest(&root, &lock(), 200);
        assert_eq!(
            report.commits_checked, 2,
            "the branch's own commit is not walked"
        );
        assert_eq!(
            hits(&report),
            vec![hit("merge feature", "require", "src/feature.ts", 1)]
        );
    }

    #[test]
    fn the_root_commit_counts_nothing() {
        let (_temp, root) = repo();
        write(&root, "src/a.ts", "require('x');\n");
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "--amend", "-m", "initial import"]);
        let report = backtest(&root, &lock(), 200);
        assert_eq!(report.commits_checked, 0);
        assert_eq!(report.total, 0);
        assert_eq!(report.head, rev_parse(&root, "HEAD"));
    }

    #[test]
    fn generated_and_non_utf8_files_count_nothing() {
        let (_temp, root) = repo();
        write(&root, ".gitattributes", "src/gen.ts linguist-generated\n");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/latin1.ts"), b"require('x'); // caf\xe9\n").unwrap();
        commit(
            &root,
            &[
                ("src/gen.ts", "require('x');\n"),
                ("src/real.ts", "require('x');\n"),
            ],
            "add",
        );
        assert_eq!(
            hits(&backtest(&root, &lock(), 200)),
            vec![hit("add", "require", "src/real.ts", 1)]
        );
    }

    #[test]
    fn a_rename_counts_only_a_path_rule_its_new_name_breaks() {
        let (_temp, root) = repo();
        commit(&root, &[("src/a.ts", "require('x');\n")], "add");
        git(&root, &["mv", "src/a.ts", "src/b.ts"]);
        commit(&root, &[], "rename");
        git(&root, &["mv", "src/b.ts", "src/BadName.ts"]);
        commit(&root, &[], "rename badly");
        assert_eq!(
            hits(&backtest(&root, &lock(), 200)),
            vec![
                hit("rename badly", "naming", "src/BadName.ts", 0),
                hit("add", "require", "src/a.ts", 1),
            ]
        );
    }

    #[test]
    fn a_rename_counts_a_path_rule_only_when_the_old_name_kept_it() {
        let (_temp, root) = repo();
        commit(&root, &[("src/BadName.ts", "export {};\n")], "add badly");
        git(&root, &["mv", "src/BadName.ts", "src/OtherBad.ts"]);
        commit(&root, &[], "rename bad to bad");
        commit(
            &root,
            &[("src/good-name.ts", "export const a = 1;\n")],
            "add well",
        );
        git(&root, &["mv", "src/good-name.ts", "src/NowBad.ts"]);
        commit(&root, &[], "rename good to bad");
        assert_eq!(
            hits(&backtest(&root, &lock(), 200)),
            vec![
                hit("rename good to bad", "naming", "src/NowBad.ts", 0),
                hit("add badly", "naming", "src/BadName.ts", 0),
            ]
        );
    }

    #[test]
    fn excepted_hits_are_dropped() {
        let (_temp, root) = repo();
        commit(
            &root,
            &[("src/a.ts", "require('x');\nrequire('y');\n")],
            "add",
        );
        let exception = Exception::Violation {
            clause: "require".into(),
            para_hash: "elsewhere".into(),
            file: "src/a.ts".into(),
            matched: "require('x')".into(),
            reason: Reason::Deliberate,
            note: None,
        };
        let report = run(&root, &lock(), &[exception], &cache_path(&root), 200).unwrap();
        assert_eq!(hits(&report), vec![hit("add", "require", "src/a.ts", 2)]);
        assert_eq!(report.total, 1);
        assert_eq!(report.counts, BTreeMap::from([("require".to_string(), 1)]));
    }

    #[test]
    fn the_cache_is_reused_until_the_rules_change() {
        let (_temp, root) = repo();
        commit(&root, &[("src/a.ts", "require('x');\n")], "add");
        let first = backtest(&root, &lock(), 200);
        assert_eq!(first.total, 1);
        // Plant a finding no check could produce: it shows only if the cache is read.
        let path = cache_path(&root);
        let mut cache: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        cache["commits"][&first.head]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "clause_id": "require", "file": "src/planted.ts", "line": 7,
                "matched": "require('planted')"
            }));
        fs::write(&path, cache.to_string()).unwrap();
        assert_eq!(
            hits(&backtest(&root, &lock(), 200)),
            vec![
                hit("add", "require", "src/a.ts", 1),
                hit("add", "require", "src/planted.ts", 7),
            ]
        );
        let changed = compile(vec![require_clause(&["src/**/*.ts"])]);
        assert_eq!(
            hits(&backtest(&root, &changed, 200)),
            vec![hit("add", "require", "src/a.ts", 1)],
            "a different lock recomputes"
        );
    }

    #[test]
    fn depth_limits_the_walk() {
        let (_temp, root) = repo();
        for name in ["one", "two", "three"] {
            commit(
                &root,
                &[(&format!("src/{name}.ts"), "require('x');\n")],
                name,
            );
        }
        let report = backtest(&root, &lock(), 2);
        assert_eq!(report.depth, 2);
        assert_eq!(report.commits_checked, 2);
        assert_eq!(
            hits(&report),
            vec![
                hit("three", "require", "src/three.ts", 1),
                hit("two", "require", "src/two.ts", 1),
            ]
        );
        assert_eq!(backtest(&root, &lock(), 200).total, 3);
    }

    #[test]
    fn parses_raw_output_with_renames_and_skips_deletions_and_symlinks() {
        let (old, new) = ("a".repeat(40), "b".repeat(40));
        let raw = format!(
            ":000000 100644 {z} {new} A\0src/new.ts\0\
             :100644 100644 {old} {new} M\0src/mod.ts\0\
             :100644 100644 {old} {new} R087\0src/from.ts\0src/to.ts\0\
             :100644 000000 {old} {z} D\0src/gone.ts\0\
             :000000 120000 {z} {new} A\0src/link.ts\0",
            z = "0".repeat(40)
        );
        let changes = parse_raw(&raw);
        let summary: Vec<(&str, bool, bool, Option<&str>)> = changes
            .iter()
            .map(|change| {
                (
                    change.path.as_str(),
                    change.old_blob.is_some(),
                    change.created,
                    change.old_path.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("src/new.ts", false, true, None),
                ("src/mod.ts", true, false, None),
                ("src/to.ts", true, true, Some("src/from.ts")),
            ]
        );
    }

    /// The shas of the commits the cache holds.
    fn cached(root: &Path) -> Vec<String> {
        let cache: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(cache_path(root)).unwrap()).unwrap();
        cache["commits"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// A clone of `source` with every commit and tree but only the blobs HEAD needs, or with
    /// none at all when nothing is checked out.
    fn blobless_clone(source: &Path, checkout: bool) -> PathBuf {
        git(source, &["config", "uploadpack.allowFilter", "true"]);
        let parent = source.parent().unwrap();
        let url = format!("file://{}", source.display());
        let mut args = vec!["clone", "-q", "--filter=blob:none"];
        if !checkout {
            args.push("--no-checkout");
        }
        args.extend([url.as_str(), "clone"]);
        git(parent, &args);
        parent.join("clone")
    }

    #[test]
    fn a_blobless_clone_is_checked_as_far_as_it_goes_and_those_commits_are_not_cached() {
        if !supports_no_lazy_fetch(
            "a_blobless_clone_is_checked_as_far_as_it_goes_and_those_commits_are_not_cached",
        ) {
            return;
        }
        let (_temp, root) = repo();
        let body = (1..=20)
            .map(|n| format!("const v{n} = {n};\n"))
            .collect::<String>();
        commit(
            &root,
            &[("src/a.ts", &format!("{body}require('x');\n"))],
            "add a",
        );
        // A rename that changes the file is scored on its content, which the clone lacks.
        git(&root, &["mv", "src/a.ts", "src/BadName.ts"]);
        commit(
            &root,
            &[("src/BadName.ts", &format!("{body}require('y');\n"))],
            "rename badly",
        );
        commit(&root, &[("src/c.ts", "require('z');\n")], "add c");
        let clone = blobless_clone(&root, false);

        let report = backtest(&clone, &lock(), 200);
        assert_eq!(report.commits_checked, 3);
        assert_eq!(report.incomplete_commits, 3, "no commit had all its blobs");
        assert_eq!(
            hits(&report),
            Vec::new(),
            "a commit's hits can be wrong either way once its files are missing: the rename \
             shows as a new file, so its path is checked as if it had no old name"
        );
        assert_eq!(report.total, 0);
        assert!(report.counts.is_empty());
        assert_eq!(
            cached(&clone),
            Vec::<String>::new(),
            "no commit had all its blobs"
        );
        let again = backtest(&clone, &lock(), 200);
        assert_eq!(again, report, "checked again, with the same answer");
    }

    #[test]
    fn a_rename_git_cannot_score_in_a_blobless_clone_is_not_a_new_violation() {
        if !supports_no_lazy_fetch(
            "a_rename_git_cannot_score_in_a_blobless_clone_is_not_a_new_violation",
        ) {
            return;
        }
        let (_temp, root) = repo();
        let body = (1..=20)
            .map(|n| format!("const v{n} = {n};\n"))
            .collect::<String>();
        commit(
            &root,
            &[("src/old.ts", &format!("{body}require('x');\n"))],
            "add old",
        );
        git(&root, &["mv", "src/old.ts", "src/new.ts"]);
        commit(
            &root,
            &[(
                "src/new.ts",
                &format!("{body}require('x');\nconst extra = 1;\n"),
            )],
            "rename and edit",
        );
        assert_eq!(
            hits(&backtest(&root, &lock(), 200)),
            vec![hit("add old", "require", "src/old.ts", 21)],
            "the rename moves the violation, it does not add one"
        );

        // The checkout gives the clone `src/new.ts`; scoring the rename needs `src/old.ts` too.
        let clone = blobless_clone(&root, true);
        let report = backtest(&clone, &lock(), 200);
        assert_eq!(report.commits_checked, 2);
        assert_eq!(
            report.incomplete_commits, 2,
            "`add old` lacks its blob, `rename and edit` could not be scored"
        );
        assert_eq!(
            hits(&report),
            Vec::new(),
            "without the rename the new file would be all violation"
        );
        assert_eq!(cached(&clone), Vec::<String>::new());
    }

    #[test]
    fn a_missing_blob_leaves_its_commit_uncached_until_it_is_back() {
        let (_temp, root) = repo();
        commit(&root, &[("src/a.ts", "require('a');\n")], "add a");
        commit(&root, &[("src/b.ts", "require('b');\n")], "add b");
        let blob = rev_parse(&root, "HEAD~1:src/a.ts");
        let object = root.join(".git/objects").join(&blob[..2]).join(&blob[2..]);
        let saved = fs::read(&object).unwrap();
        fs::remove_file(&object).unwrap();

        let report = backtest(&root, &lock(), 200);
        assert_eq!(hits(&report), vec![hit("add b", "require", "src/b.ts", 1)]);
        assert_eq!(report.incomplete_commits, 1);
        assert_eq!(cached(&root), vec![rev_parse(&root, "HEAD")]);

        fs::write(&object, saved).unwrap();
        let report = backtest(&root, &lock(), 200);
        assert_eq!(
            hits(&report),
            vec![
                hit("add b", "require", "src/b.ts", 1),
                hit("add a", "require", "src/a.ts", 1),
            ],
            "checked again once the blob is there"
        );
        assert_eq!(report.incomplete_commits, 0);
        assert_eq!(cached(&root).len(), 2);
    }

    #[test]
    fn a_commit_that_could_not_be_fully_checked_counts_none_of_its_hits() {
        let (_temp, root) = repo();
        // `BadName.ts` breaks the naming rule by its path alone, and its blob is the one that
        // goes missing; `b.ts` is still readable, and `c.ts` is in another commit.
        commit(
            &root,
            &[
                ("src/BadName.ts", "export {};\n"),
                ("src/b.ts", "require('b');\n"),
            ],
            "add both",
        );
        commit(&root, &[("src/c.ts", "require('c');\n")], "add c");
        let blob = rev_parse(&root, "HEAD~1:src/BadName.ts");
        let object = root.join(".git/objects").join(&blob[..2]).join(&blob[2..]);
        let saved = fs::read(&object).unwrap();
        fs::remove_file(&object).unwrap();

        let report = backtest(&root, &lock(), 200);
        assert_eq!(report.commits_checked, 2);
        assert_eq!(report.incomplete_commits, 1);
        assert_eq!(
            hits(&report),
            vec![hit("add c", "require", "src/c.ts", 1)],
            "neither the naming hit nor the readable file's require hit of `add both`"
        );
        assert_eq!(report.total, 1);
        assert_eq!(report.counts, BTreeMap::from([("require".to_string(), 1)]));

        fs::write(&object, saved).unwrap();
        let report = backtest(&root, &lock(), 200);
        assert_eq!(report.incomplete_commits, 0);
        assert_eq!(
            hits(&report),
            vec![
                hit("add c", "require", "src/c.ts", 1),
                hit("add both", "naming", "src/BadName.ts", 0),
                hit("add both", "require", "src/b.ts", 1),
            ]
        );
        assert_eq!(report.total, 3);
    }

    #[test]
    fn a_walk_longer_than_one_chunk_attributes_every_hit_to_its_commit() {
        let (_temp, root) = repo();
        let count = CHUNK * 2 + 6;
        for n in 1..=count {
            commit(
                &root,
                &[(&format!("src/f{n}.ts"), &format!("require('x{n}');\n"))],
                &format!("c{n}"),
            );
        }
        let report = backtest(&root, &lock(), 200);
        assert_eq!(report.commits_checked, count);
        let expected: Vec<_> = (1..=count)
            .rev()
            .map(|n| hit(&format!("c{n}"), "require", &format!("src/f{n}.ts"), 1))
            .collect();
        assert_eq!(hits(&report), expected);
        assert_eq!(cached(&root).len(), count);
    }

    #[test]
    fn files_under_the_audits_skipped_directories_count_nothing() {
        let (_temp, root) = repo();
        commit(
            &root,
            &[
                ("vendor/lib.ts", "require('x');\n"),
                ("src/node_modules/dep.ts", "require('x');\n"),
                ("third_party/y.ts", "require('x');\n"),
                ("src/real.ts", "require('x');\n"),
            ],
            "add",
        );
        assert_eq!(
            hits(&backtest(&root, &lock(), 200)),
            vec![hit("add", "require", "src/real.ts", 1)]
        );
    }

    #[test]
    fn changing_what_is_generated_recomputes_the_cache() {
        let (_temp, root) = repo();
        commit(&root, &[("src/gen.ts", "require('x');\n")], "add");
        assert_eq!(backtest(&root, &lock(), 200).total, 1);
        write(&root, ".gitattributes", "src/gen.ts linguist-generated\n");
        assert_eq!(
            backtest(&root, &lock(), 200).total,
            0,
            "the cached hit was for a file that is generated now"
        );
        write(&root, ".gitattributes", "# nothing generated\n");
        assert_eq!(backtest(&root, &lock(), 200).total, 1);
    }

    #[test]
    fn a_cache_that_cannot_be_written_does_not_fail_the_run() {
        let (_temp, root) = repo();
        commit(&root, &[("src/a.ts", "require('x');\n")], "add");
        // A file where the cache's directory should be.
        fs::write(root.join(".git/pragma"), "").unwrap();
        let report = backtest(&root, &lock(), 200);
        assert_eq!(hits(&report), vec![hit("add", "require", "src/a.ts", 1)]);
    }

    #[test]
    fn a_backtest_runs_every_clause_but_the_edit_only_ones() {
        assert_eq!(lock().clauses().count(), 3);
        assert_eq!(rules_checked(&lock()), 2, "protected paths never run");
        assert_eq!(rules_checked(&compile(vec![])), 0);
    }

    #[test]
    fn a_repo_with_no_commits_has_an_empty_report() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-q"]);
        let report = backtest(temp.path(), &lock(), 200);
        assert_eq!(report.commits_checked, 0);
        assert!(report.head.is_empty());
    }
}
