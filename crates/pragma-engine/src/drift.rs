//! Drift: docs checked against the repo. A doc that names a file, script or code name the repo no
//! longer has misleads every agent and person who reads it. No model is involved.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::docs::DocFile;
use crate::hashing::short_hash;
use crate::markdown::paragraphs;
use crate::paths::join_relative;
use crate::refs::{self, RawRef, RefSource};
use crate::repo_index::RepoIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriftKind {
    MissingPath,
    UnknownScript,
    UnknownName,
    /// A path that starts with `/` and matches no file: more likely a URL of a deployed site
    /// than a file of this repo. Always `Confidence::Low`.
    MaybeUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    High,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftFinding {
    pub id: String,
    pub doc: String,
    /// 1-based.
    pub line: u32,
    pub para_hash: String,
    pub kind: DriftKind,
    /// The reference as written.
    pub reference: String,
    /// What to write instead, when the file was moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    pub confidence: Confidence,
    /// Byte range of the path inside the doc, for `--diff`. Not saved.
    #[serde(skip)]
    pub fix_span: Option<(usize, usize)>,
    /// The doc text the span was taken from; `--diff` skips the edit if the doc has changed.
    #[serde(skip)]
    pub fix_original: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DriftReport {
    pub docs_checked: usize,
    pub references_checked: usize,
    pub findings: Vec<DriftFinding>,
}

enum Target {
    /// A path, with its byte range in the doc (after trimming `./`, `#anchor` and `:line`).
    Path {
        path: String,
        span: (usize, usize),
    },
    Script(String),
    Make(String),
    Name(String),
}

const KNOWN_EXTENSIONS: &[&str] = &[
    "md", "mdc", "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "json", "jsonc", "rs",
    "toml", "yaml", "yml", "sh", "sql", "css", "scss", "html", "py", "go", "lock", "txt",
];

/// Decides what a reference claims about the repo, if anything checkable.
fn classify(raw: &RawRef, doc_dir: &str, index: &RepoIndex) -> Option<Target> {
    let text = raw.text.trim();
    match raw.source {
        RefSource::Link => {
            if text.contains("://") || text.starts_with("mailto:") || text.starts_with('#') {
                return None;
            }
            path_target(raw, text)
        }
        RefSource::Import => path_target(raw, text),
        RefSource::CodeSpan => {
            if let Some(script) = script_name(text) {
                return Some(Target::Script(script));
            }
            if let Some(target) = text
                .strip_prefix("make ")
                .and_then(|rest| rest.split_whitespace().next())
                .filter(|target| is_script_name(target))
            {
                return Some(Target::Make(target.to_string()));
            }
            if looks_like_path(text, doc_dir, index) {
                return path_target(raw, text);
            }
            looks_like_name(text).then(|| {
                Target::Name(
                    text.trim_start_matches('$')
                        .trim_end_matches("()")
                        .to_string(),
                )
            })
        }
    }
}

/// The path inside a reference, with `./`, `#anchor`, `?query` and `:line` trimmed off, and its
/// byte range in the doc.
fn path_target(raw: &RawRef, text: &str) -> Option<Target> {
    let lead = raw.text.len() - raw.text.trim_start().len();
    let mut start = raw.start + lead;
    let mut path = text;
    if let Some(rest) = path.strip_prefix("./") {
        path = rest;
        start += 2;
    }
    path = path.split(['#', '?']).next().unwrap_or(path);
    path = path.trim_end_matches(['.', ',', ';', ')']);
    if let Some((before, after)) = path.rsplit_once(':')
        && !after.is_empty()
        && after.chars().all(|c| c.is_ascii_digit())
    {
        let column = before
            .rsplit_once(':')
            .filter(|(_, n)| n.chars().all(|c| c.is_ascii_digit()));
        path = column.map(|(file, _)| file).unwrap_or(before);
    }
    if path.is_empty() {
        return None;
    }
    let span = (start, start + path.len());
    Some(Target::Path {
        path: path.replace("%20", " "),
        span,
    })
}

fn script_name(text: &str) -> Option<String> {
    let mut words = text.split_whitespace();
    let runner = words.next()?;
    if !matches!(runner, "pnpm" | "npm" | "yarn" | "bun") {
        return None;
    }
    let mut words = words.skip_while(|word| *word != "run");
    words.next()?;
    words
        .find(|word| !word.starts_with('-'))
        .map(|word| word.trim_matches(['"', '\'']).to_string())
        .filter(|name| is_script_name(name))
}

/// Script and make-target names; rules out placeholders such as `<script>`.
fn is_script_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-'))
}

fn looks_like_path(text: &str, doc_dir: &str, index: &RepoIndex) -> bool {
    let banned = |c: char| c.is_whitespace() || "*?[]{}<>|()=,'\"`$~".contains(c);
    if text.is_empty()
        || text.contains("://")
        || text.starts_with(['-', '@', '#'])
        || text.starts_with("//")
        || text.chars().any(banned)
    {
        return false;
    }
    let cleaned = text
        .trim_start_matches("./")
        .split(['#', '?'])
        .next()
        .unwrap_or("");
    let cleaned = cleaned.trim_end_matches(['.', ',', ';', ':']);
    let trimmed = cleaned.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let extension = last
        .rsplit_once('.')
        .map(|(stem, ext)| (stem, ext.split(':').next().unwrap_or(ext)));
    let has_extension = extension.is_some_and(|(_, ext)| KNOWN_EXTENSIONS.contains(&ext));
    if trimmed.starts_with('/') && !has_extension {
        // `/rpc` is a route, not a file.
        return false;
    }
    let multi_segment = trimmed.contains('/');
    if !multi_segment {
        // A bare name like `dist/` is generic. A bare file name counts when it has a real stem:
        // not `.mdc` or `.d.ts` (extensions), and not framework names such as `Node.js`.
        let parts: Vec<&str> = last.trim_start_matches('.').split('.').collect();
        let only_extension = last.starts_with('.') && parts.len() == 1;
        let framework = extension.is_some_and(|(stem, ext)| {
            ext == "js"
                && stem.starts_with(|c: char| c.is_ascii_uppercase())
                && stem.chars().any(|c| c.is_ascii_lowercase())
        });
        return has_extension && !framework && !only_extension && parts[0].len() >= 3;
    }
    if has_extension {
        return true;
    }
    // Extensionless: a path only if its first segment is a real directory somewhere it could be
    // resolved from. Otherwise it is likely a branch, ref or package name.
    let first = trimmed.split('/').next().unwrap_or("");
    let mut bases = vec![doc_dir.to_string(), String::new()];
    bases.extend(index.package_roots.iter().cloned());
    bases
        .iter()
        .filter_map(|base| join_relative(base, first))
        .any(|dir| index.dirs.contains(&dir))
}

fn looks_like_name(text: &str) -> bool {
    let name = text.trim_end_matches("()");
    let identifier = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    let code_like = name.chars().skip(1).any(|c| c.is_ascii_uppercase())
        || name.contains('_')
        || text.ends_with("()");
    identifier && code_like && name.len() >= 4
}

/// The bases a doc's relative path can be written against: the doc's directory, the repo root,
/// and every package root.
fn bases(doc_dir: &str, index: &RepoIndex) -> Vec<String> {
    let mut bases = vec![doc_dir.to_string(), String::new()];
    bases.extend(
        index
            .package_roots
            .iter()
            .filter(|root| !root.is_empty())
            .cloned(),
    );
    bases
}

/// A path exists when it resolves from the doc's directory, the repo root or a package root, or
/// when it is the tail of a real path: docs often write a path relative to a directory the prose
/// names ("the slice's `contributions/browser.ts`"). A last segment with no extension also
/// resolves to a file of that name that has one (`design/Foo` for `design/Foo.dc.html`), unless
/// the doc wrote a trailing `/` to say it is a folder.
fn path_exists(path: &str, doc_dir: &str, index: &RepoIndex) -> bool {
    let trimmed = path.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let extended: &[String] = if path.ends_with('/') {
        &[]
    } else {
        index.named_with_extension(name)
    };
    if let Some(rooted) = trimmed.strip_prefix('/') {
        return index.exists(rooted) || extended.iter().any(|stemmed| stemmed == rooted);
    }
    let tail = format!("/{trimmed}");
    if index
        .named(name)
        .iter()
        .chain(extended)
        .any(|real| real == trimmed || real.ends_with(&tail))
    {
        return true;
    }
    bases(doc_dir, index)
        .iter()
        .filter_map(|base| join_relative(base, trimmed))
        .any(|candidate| index.exists(&candidate) || extended.contains(&candidate))
}

/// Paths the repo's .gitignore files cover are build output or local files: absent by design.
fn path_ignored(path: &str, doc_dir: &str, index: &RepoIndex) -> bool {
    let is_dir = path.ends_with('/');
    let trimmed = path.trim_end_matches('/');
    let trimmed = trimmed.strip_prefix('/').unwrap_or(trimmed);
    bases(doc_dir, index)
        .iter()
        .filter_map(|base| join_relative(base, trimmed))
        .any(|candidate| index.is_ignored(&candidate, is_dir))
}

/// Where a missing path went: a rename in git history, or the one same-named file whose path
/// shares the longest tail with the old one. Written in the doc's style: relative to the same
/// package root when the old path was.
fn suggest(
    path: &str,
    doc_dir: &str,
    index: &RepoIndex,
    renames: &HashMap<String, String>,
) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let renamed = bases(doc_dir, index)
        .iter()
        .filter_map(|base| join_relative(base, trimmed))
        .find_map(|candidate| {
            renames
                .get(&candidate)
                .filter(|new| index.exists(new))
                .cloned()
        });
    let found = renamed.or_else(|| {
        let name = trimmed.rsplit('/').next()?;
        let old: Vec<&str> = trimmed.split('/').collect();
        let tail = |candidate: &str| {
            candidate
                .split('/')
                .rev()
                .zip(old.iter().rev())
                .take_while(|(a, b)| a == *b)
                .count()
        };
        let candidates = index.named(name);
        let best = candidates.iter().map(|c| tail(c)).max()?;
        let mut winners = candidates.iter().filter(|c| tail(c) == best);
        let winner = winners.next()?;
        winners.next().is_none().then(|| winner.clone())
    })?;
    let first = trimmed.split('/').next().unwrap_or("");
    let styled = index
        .package_roots
        .iter()
        .filter(|root| !root.is_empty())
        .filter_map(|root| found.strip_prefix(&format!("{root}/")))
        .find(|rest| {
            rest.split('/').next() == Some(first) && !found.starts_with(&format!("{first}/"))
        })
        .map(str::to_string);
    let mut suggestion = styled.unwrap_or(found);
    if path.ends_with('/') {
        suggestion.push('/');
    }
    Some(suggestion)
}

/// Renames in recent history, old path to new path, newest first. Empty when git is missing.
pub fn git_renames(root: &Path) -> HashMap<String, String> {
    let output = Command::new("git")
        .args([
            "-c",
            "core.quotePath=false",
            "log",
            "-M",
            "--diff-filter=R",
            "--name-status",
            "--format=",
            "-n",
            "2000",
        ])
        .current_dir(root)
        .output();
    let mut renames = HashMap::new();
    let Ok(output) = output else { return renames };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if let [status, old, new] = parts.as_slice()
            && status.starts_with('R')
        {
            renames
                .entry(old.to_string())
                .or_insert_with(|| new.to_string());
        }
    }
    renames
}

/// Checks one doc. Returns how many references it checked and what it found.
pub fn check_doc(
    rel: &str,
    text: &str,
    index: &RepoIndex,
    renames: &HashMap<String, String>,
) -> (usize, Vec<DriftFinding>) {
    let doc_dir = rel.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let paras = paragraphs(text);
    let mut checked = 0;
    let mut findings = Vec::new();
    for raw in refs::extract(text) {
        let Some(target) = classify(&raw, doc_dir, index) else {
            continue;
        };
        checked += 1;
        let (kind, confidence, suggestion, fix_span) = match &target {
            Target::Path { path, span } => {
                if path_exists(path, doc_dir, index) || path_ignored(path, doc_dir, index) {
                    continue;
                }
                let (kind, confidence) = if path.starts_with('/') {
                    // `/sw.js` may be a URL of the deployed site, not a path from the repo root.
                    (DriftKind::MaybeUrl, Confidence::Low)
                } else if path.trim_end_matches('/').contains('/') {
                    (DriftKind::MissingPath, Confidence::High)
                } else {
                    // A bare file name may mean a kind of file (`settings.json`), not this repo's.
                    (DriftKind::MissingPath, Confidence::Low)
                };
                (
                    kind,
                    confidence,
                    suggest(path, doc_dir, index, renames),
                    Some(*span),
                )
            }
            Target::Script(name) => {
                if index.scripts.contains(name) {
                    continue;
                }
                (DriftKind::UnknownScript, Confidence::High, None, None)
            }
            Target::Make(name) => {
                if index.make_targets.contains(name) {
                    continue;
                }
                (DriftKind::UnknownScript, Confidence::High, None, None)
            }
            Target::Name(name) => {
                if index.has_identifier(name) {
                    continue;
                }
                (DriftKind::UnknownName, Confidence::Low, None, None)
            }
        };
        let para_hash = paras
            .iter()
            .find(|para| para.start <= raw.start && raw.start < para.end)
            .map(|para| para.hash.clone())
            .unwrap_or_else(|| {
                short_hash(
                    text.lines()
                        .nth(raw.line as usize - 1)
                        .unwrap_or("")
                        .as_bytes(),
                )
            });
        let id = short_hash(format!("{rel}|{para_hash}|{kind:?}|{}", raw.text).as_bytes());
        findings.push(DriftFinding {
            id,
            doc: rel.to_string(),
            line: raw.line,
            para_hash,
            kind,
            reference: raw.text.clone(),
            fix_original: suggestion
                .as_ref()
                .and(fix_span)
                .and_then(|(start, end)| text.get(start..end))
                .map(str::to_string),
            fix_span: suggestion.as_ref().and(fix_span),
            suggestion,
            confidence,
        });
    }
    (checked, findings)
}

pub fn run(root: &Path, docs: &[DocFile]) -> Result<DriftReport> {
    let index = RepoIndex::build(root);
    let renames = git_renames(root);
    let mut report = DriftReport::default();
    for doc in docs.iter().filter(|doc| !doc.historical) {
        let text = std::fs::read_to_string(root.join(&doc.rel))
            .with_context(|| format!("reading {}", doc.rel))?;
        let (checked, findings) = check_doc(&doc.rel, &text, &index, &renames);
        report.docs_checked += 1;
        report.references_checked += checked;
        report.findings.extend(findings);
    }
    Ok(report)
}

/// A doc with its suggested fixes applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocFix {
    pub doc: String,
    pub old: String,
    pub new: String,
    /// The findings whose fix is in `new`.
    pub applied: Vec<String>,
}

/// The docs with every suggested fix applied, one entry per doc that changes. A fix whose span
/// no longer holds the text it was taken from is left out, so nothing lands in the wrong place.
pub fn apply_fixes(root: &Path, findings: &[DriftFinding]) -> Result<Vec<DocFix>> {
    let mut by_doc: Vec<(&str, Vec<&DriftFinding>)> = Vec::new();
    for finding in findings
        .iter()
        .filter(|f| f.suggestion.is_some() && f.fix_span.is_some())
    {
        match by_doc.iter_mut().find(|(doc, _)| *doc == finding.doc) {
            Some((_, list)) => list.push(finding),
            None => by_doc.push((&finding.doc, vec![finding])),
        }
    }
    let mut fixes = Vec::new();
    for (doc, mut list) in by_doc {
        let old =
            std::fs::read_to_string(root.join(doc)).with_context(|| format!("reading {doc}"))?;
        let mut new = old.clone();
        let mut applied = Vec::new();
        list.sort_by_key(|finding| std::cmp::Reverse(finding.fix_span.map(|span| span.0)));
        for finding in list {
            let (start, end) = finding.fix_span.expect("filtered above");
            let matches = match (old.get(start..end), finding.fix_original.as_deref()) {
                (Some(current), Some(original)) => current == original,
                _ => false,
            };
            if !matches {
                continue;
            }
            new.replace_range(
                start..end,
                finding.suggestion.as_deref().expect("filtered above"),
            );
            applied.push(finding.id.clone());
        }
        if !applied.is_empty() {
            fixes.push(DocFix {
                doc: doc.to_string(),
                old,
                new,
                applied,
            });
        }
    }
    Ok(fixes)
}

/// A unified diff applying every suggested fix, ready for `git apply`.
pub fn fix_diff(root: &Path, findings: &[DriftFinding]) -> Result<String> {
    let mut out = String::new();
    for fix in apply_fixes(root, findings)? {
        let diff = similar::TextDiff::from_lines(&fix.old, &fix.new);
        out.push_str(
            &diff
                .unified_diff()
                .context_radius(3)
                .header(&format!("a/{}", fix.doc), &format!("b/{}", fix.doc))
                .to_string(),
        );
    }
    Ok(out)
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

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let (temp, root) = crate::testutil::repo();
        write(&root, "package.json", r#"{"scripts": {"dev": "x"}}"#);
        write(
            &root,
            "apps/app/package.json",
            r#"{"scripts": {"doctor": "x"}}"#,
        );
        write(
            &root,
            "apps/app/src/core/tasks/api/task-selectors.ts",
            "export function getTaskManagerStore() {}\n",
        );
        write(&root, "apps/app/src/main/a.ts", "export const a = 1;\n");
        (temp, root)
    }

    fn kinds(findings: &[DriftFinding]) -> Vec<(DriftKind, &str)> {
        findings
            .iter()
            .map(|f| (f.kind, f.reference.as_str()))
            .collect()
    }

    #[test]
    fn existing_references_pass_from_any_base() {
        let (_temp, root) = fixture();
        let index = RepoIndex::build(&root);
        let doc = "Run `pnpm run dev` or `pnpm --dir apps/app run doctor`. See `src/main/a.ts:12`, \
                   `apps/app/src/main/`, [readme](../README.md#top), `package.json`, `getTaskManagerStore()`.\n";
        let (checked, findings) = check_doc("docs/guide.md", doc, &index, &HashMap::new());
        assert_eq!(checked, 7);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_path_that_is_the_tail_of_a_real_path_is_present() {
        let (_temp, root) = fixture();
        let index = RepoIndex::build(&root);
        let doc = "Each slice has `tasks/api/task-selectors.ts`; never `tasks/browser/task-selectors.ts`.\n";
        let (_, findings) = check_doc("AGENTS.md", doc, &index, &HashMap::new());
        assert_eq!(
            kinds(&findings),
            vec![(DriftKind::MissingPath, "tasks/browser/task-selectors.ts")]
        );
    }

    #[test]
    fn reports_missing_paths_scripts_and_names() {
        let (_temp, root) = fixture();
        let index = RepoIndex::build(&root);
        let doc =
            "Use `src/main/gone.ts`, `pnpm run nope`, `make deploy` and `getProjectStoreGone`.\n";
        let (_, findings) = check_doc("AGENTS.md", doc, &index, &HashMap::new());
        assert_eq!(
            kinds(&findings),
            vec![
                (DriftKind::MissingPath, "src/main/gone.ts"),
                (DriftKind::UnknownScript, "pnpm run nope"),
                (DriftKind::UnknownScript, "make deploy"),
                (DriftKind::UnknownName, "getProjectStoreGone"),
            ]
        );
        assert_eq!(findings[3].confidence, Confidence::Low);
    }

    #[test]
    fn ignores_generic_names_branches_urls_globs_and_code_blocks() {
        let (_temp, root) = fixture();
        let index = RepoIndex::build(&root);
        let doc = "Avoid `dist/`, `origin/main`, `refs/heads/`, `*.test.ts`, `Node.js`, `https://x.y/a.md`, \
                   `kind !== 'ready'` and `<feature>/node`.\n\n```sh\ncat src/gone.ts\n```\n";
        let (_, findings) = check_doc("AGENTS.md", doc, &index, &HashMap::new());
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn skips_placeholders_routes_extensions_and_ignored_paths() {
        let (_temp, root) = fixture();
        write(&root, ".gitignore", "dist/\n.scratch/\n");
        let index = RepoIndex::build(&root);
        let doc = "Use `pnpm run <script>`, `make <target>`, `/rpc`, `//server/share`, `.mdc`, `.d.ts`, \
                   `packages/x/dist/index.mjs` and [notes](.scratch/plan.md).\n";
        let (_, findings) = check_doc("AGENTS.md", doc, &index, &HashMap::new());
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn bare_file_names_and_env_vars_are_less_certain() {
        let (_temp, root) = fixture();
        let index = RepoIndex::build(&root);
        let doc = "See `local-fs.ts` and `$PRAGMA_GONE_DIR`.\n";
        let (_, findings) = check_doc("AGENTS.md", doc, &index, &HashMap::new());
        let summary: Vec<(&str, Confidence)> = findings
            .iter()
            .map(|f| (f.reference.as_str(), f.confidence))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("local-fs.ts", Confidence::Low),
                ("$PRAGMA_GONE_DIR", Confidence::Low)
            ]
        );
    }

    #[test]
    fn a_name_left_without_its_extension_is_present_when_a_file_has_one() {
        let (_temp, root) = fixture();
        write(
            &root,
            "design/screens/DS-Foundations.dc.html",
            "<html></html>\n",
        );
        write(&root, "design/tokens/colors.json", "{}\n");
        write(
            &root,
            "apps/app/src/screens/Home.dc.html",
            "<html></html>\n",
        );
        write(&root, "docker/Dockerfile.dev", "FROM scratch\n");
        let index = RepoIndex::build(&root);
        // From the repo root, in a code span and in links, with a leading slash, and as the tail
        // of a longer path.
        let doc = "Final: `design/screens/DS-Foundations`, `docker/Dockerfile`, \
                   [tokens](design/tokens/colors), [screen](/design/screens/DS-Foundations) and \
                   [again](screens/DS-Foundations).\n";
        let (checked, findings) = check_doc("docs/design.md", doc, &index, &HashMap::new());
        assert_eq!(checked, 5);
        assert!(findings.is_empty(), "{findings:?}");
        // The `..` stops the tail from matching, so only the doc's folder reaches it.
        let doc = "See `../tokens/colors`.\n";
        let (_, findings) = check_doc("design/screens/README.md", doc, &index, &HashMap::new());
        assert!(findings.is_empty(), "{findings:?}");
        // The same, with only the package root `apps/app` reaching it.
        let doc = "See `src/main/../screens/Home`.\n";
        let (_, findings) = check_doc("docs/guide.md", doc, &index, &HashMap::new());
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn an_extensionless_name_needs_a_file_of_that_name_plus_an_extension() {
        let (_temp, root) = fixture();
        write(
            &root,
            "design/screens/DS-Foundations.dc.html",
            "<html></html>\n",
        );
        write(&root, "apps/app/src/main/b.ts.map", "{}\n");
        let index = RepoIndex::build(&root);
        // A name with no such file; a folder, which a file does not stand for; and names that
        // already have an extension, which a longer one does not stand for.
        let doc = "Gone: `design/screens/DS-Missing`. Folder: `design/screens/DS-Foundations/`. \
                   Longer: `src/main/b.ts` and `design/screens/DS-Foundations.dc`.\n";
        let (_, findings) = check_doc("docs/design.md", doc, &index, &HashMap::new());
        assert_eq!(
            kinds(&findings),
            vec![
                (DriftKind::MissingPath, "design/screens/DS-Missing"),
                (DriftKind::MissingPath, "design/screens/DS-Foundations/"),
                (DriftKind::MissingPath, "src/main/b.ts"),
                (DriftKind::MissingPath, "design/screens/DS-Foundations.dc"),
            ]
        );
    }

    #[test]
    fn a_leading_slash_path_that_matches_nothing_may_be_a_url() {
        let (_temp, root) = fixture();
        let index = RepoIndex::build(&root);
        let doc = "The worker is `/sw.js`, the build is [here](/version.txt) and the code is \
                   `/apps/app/src/main/a.ts`. Gone: `src/main/gone.ts` and `/src/main/gone.ts`.\n";
        let (checked, findings) = check_doc("docs/deploy.md", doc, &index, &HashMap::new());
        assert_eq!(checked, 5);
        let summary: Vec<(&str, DriftKind, Confidence)> = findings
            .iter()
            .map(|f| (f.reference.as_str(), f.kind, f.confidence))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("/sw.js", DriftKind::MaybeUrl, Confidence::Low),
                ("/version.txt", DriftKind::MaybeUrl, Confidence::Low),
                ("src/main/gone.ts", DriftKind::MissingPath, Confidence::High),
                ("/src/main/gone.ts", DriftKind::MaybeUrl, Confidence::Low),
            ]
        );
    }

    #[test]
    fn docs_name_paths_inside_a_php_package_from_its_root() {
        let (_temp, root) = crate::testutil::repo();
        write(&root, "api/composer.json", "{}\n");
        write(&root, "api/routes/web.php", "<?php\n");
        // Ignored, so not in the index, but on disk.
        write(&root, "api/.gitignore", "/storage/app/*\n");
        write(&root, "api/storage/app/state.json", "{}\n");
        let index = RepoIndex::build(&root);
        assert_eq!(index.package_roots, vec!["api"]);
        let doc =
            "Routes: `routes/web.php`, not `routes/gone.php`. State: `storage/app/state.json`.\n";
        let (checked, findings) = check_doc("README.md", doc, &index, &HashMap::new());
        assert_eq!(checked, 3);
        assert_eq!(
            kinds(&findings),
            vec![(DriftKind::MissingPath, "routes/gone.php")]
        );
    }

    #[test]
    fn suggests_where_a_moved_file_went_in_the_docs_style() {
        let (_temp, root) = fixture();
        let index = RepoIndex::build(&root);
        let doc = "Selectors live in `src/core/tasks/browser/stores/task-selectors.ts`.\n";
        let (_, findings) = check_doc("CONTRIBUTING.md", doc, &index, &HashMap::new());
        assert_eq!(
            findings[0].suggestion.as_deref(),
            Some("src/core/tasks/api/task-selectors.ts")
        );
        let renames = HashMap::from([(
            "src/old.ts".to_string(),
            "apps/app/src/main/a.ts".to_string(),
        )]);
        let (_, findings) = check_doc("README.md", "See `src/old.ts`.\n", &index, &renames);
        assert_eq!(findings[0].suggestion.as_deref(), Some("src/main/a.ts"));
    }

    #[test]
    fn fix_diff_applies_with_git_apply() {
        let (_temp, root) = fixture();
        write(
            &root,
            "CONTRIBUTING.md",
            "# C\n\nSelectors: `src/core/tasks/browser/stores/task-selectors.ts:4`.\n",
        );
        crate::testutil::git(&root, &["add", "."]);
        crate::testutil::git(&root, &["commit", "-q", "-m", "fixture"]);
        let docs = vec![DocFile {
            rel: "CONTRIBUTING.md".into(),
            historical: false,
        }];
        let report = run(&root, &docs).unwrap();
        let diff = fix_diff(&root, &report.findings).unwrap();
        assert!(
            diff.contains("+Selectors: `src/core/tasks/api/task-selectors.ts:4`."),
            "{diff}"
        );
        let patch = root.join("fix.patch");
        fs::write(&patch, &diff).unwrap();
        crate::testutil::git(&root, &["apply", "fix.patch"]);
        assert!(
            fs::read_to_string(root.join("CONTRIBUTING.md"))
                .unwrap()
                .contains("api/task-selectors.ts:4")
        );
    }

    #[test]
    fn fix_diff_skips_a_span_that_no_longer_matches_the_doc() {
        let (_temp, root) = fixture();
        write(
            &root,
            "CONTRIBUTING.md",
            "# C\n\nSelectors: `src/core/tasks/browser/stores/task-selectors.ts:4`.\n",
        );
        crate::testutil::git(&root, &["add", "."]);
        crate::testutil::git(&root, &["commit", "-q", "-m", "fixture"]);
        let docs = vec![DocFile {
            rel: "CONTRIBUTING.md".into(),
            historical: false,
        }];
        let report = run(&root, &docs).unwrap();
        assert!(report.findings.iter().any(|f| f.fix_span.is_some()));
        // The doc shrinks after the report was made: the span now points past the end.
        write(&root, "CONTRIBUTING.md", "# C\n");
        assert_eq!(fix_diff(&root, &report.findings).unwrap(), "");
        // The doc changes under the span without shrinking.
        write(
            &root,
            "CONTRIBUTING.md",
            &format!("# C\n\n{}\n", "é".repeat(80)),
        );
        assert_eq!(fix_diff(&root, &report.findings).unwrap(), "");
    }

    #[test]
    fn apply_fixes_returns_new_text_and_the_ids_it_applied() {
        let (_temp, root) = fixture();
        write(
            &root,
            "CONTRIBUTING.md",
            "# C\n\nSelectors: `src/core/tasks/browser/stores/task-selectors.ts:4`.\n\nGone: `src/nowhere/x.ts`.\n",
        );
        crate::testutil::git(&root, &["add", "."]);
        crate::testutil::git(&root, &["commit", "-q", "-m", "fixture"]);
        let docs = vec![DocFile {
            rel: "CONTRIBUTING.md".into(),
            historical: false,
        }];
        let report = run(&root, &docs).unwrap();
        let fixes = apply_fixes(&root, &report.findings).unwrap();
        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].doc, "CONTRIBUTING.md");
        assert!(
            fixes[0]
                .new
                .contains("`src/core/tasks/api/task-selectors.ts:4`")
        );
        assert!(fixes[0].new.contains("`src/nowhere/x.ts`"));
        let fixable: Vec<&str> = report
            .findings
            .iter()
            .filter(|f| f.suggestion.is_some())
            .map(|f| f.id.as_str())
            .collect();
        assert_eq!(fixes[0].applied, fixable);
        // Nothing was written: the doc on disk is unchanged.
        assert_eq!(
            fs::read_to_string(root.join("CONTRIBUTING.md")).unwrap(),
            fixes[0].old
        );
        // A report whose fix has gone stale applies nothing.
        write(&root, "CONTRIBUTING.md", "# C\n");
        assert!(apply_fixes(&root, &report.findings).unwrap().is_empty());
    }

    #[test]
    fn git_renames_keeps_non_ascii_paths_unquoted() {
        let (_temp, root) = crate::testutil::repo();
        write(&root, "docs/café.md", "# a\n");
        crate::testutil::git(&root, &["add", "."]);
        crate::testutil::git(&root, &["commit", "-q", "-m", "add"]);
        crate::testutil::git(&root, &["mv", "docs/café.md", "docs/thé.md"]);
        crate::testutil::git(&root, &["commit", "-q", "-m", "rename"]);
        assert_eq!(
            git_renames(&root).get("docs/café.md").map(String::as_str),
            Some("docs/thé.md")
        );
    }

    #[test]
    fn skips_historical_docs() {
        let (_temp, root) = fixture();
        write(&root, "docs/adr/0001.md", "Uses `src/gone.ts`.\n");
        let docs = vec![DocFile {
            rel: "docs/adr/0001.md".into(),
            historical: true,
        }];
        let report = run(&root, &docs).unwrap();
        assert_eq!(report.docs_checked, 0);
        assert!(report.findings.is_empty());
    }
}
