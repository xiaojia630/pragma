//! A proposed clause's final rung, scope and check (§5.2 steps 3 and 4), and the order of
//! precedence between docs (step 5).
//!
//! Scope: a nested AGENTS.md (or CLAUDE.md, README.md, ...) scopes to its directory (except
//! directly in `.claude/`, `.github/` or `docs/`, where it is repo-wide), and the
//! globs the model read from the text are taken relative to it (excludes alone stay inside the
//! directory); frontmatter wins over both (`.cursor/rules/*.mdc` `globs`, `.pragma/specs`
//! `scope`). Rung: a library check that validates is blocking; a generated pattern is judged
//! until promoted (S8); a proposed check that fails validation leaves its clause judged without
//! it; a clause proposed without a check keeps the model's judged or context, and a blocking
//! proposal without one is judged. `.pragma/specs` frontmatter may set the rung, but blocking
//! still needs a library check.
//!
//! Precedence: closer doc over repo-root doc, by `depth`. Contradictions between clauses are not
//! detected here; they are S7's (the Rulebook's rewording suggestions).

use std::cmp::Ordering;
use std::path::Path;

use serde_norway::Value as Yaml;

use super::Rung;
use super::prompt::RawClause;
use super::validate;
use crate::docs::KNOWN_NAMES;
use crate::frontmatter;
use crate::hashing::short_hash;
use crate::lock::{Check, Scope};

/// What a doc contributes to every clause it yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocContext {
    pub rel: String,
    /// The directory a nested doc scopes to; None for docs that apply repo-wide.
    pub dir: Option<String>,
    /// Scope set by frontmatter; it wins over everything the text says.
    pub frontmatter_scope: Option<Vec<String>>,
    /// Rung set by `.pragma/specs` frontmatter.
    pub frontmatter_rung: Option<Rung>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub rung: Rung,
    pub scope: Vec<String>,
    pub check: Option<Check>,
    /// Why a proposed check was dropped.
    pub dropped: Option<String>,
}

/// Folders whose own copies of the known names speak for the whole repo: Claude Code's project
/// memory (`.claude/CLAUDE.md`), GitHub's community files (`.github/CONTRIBUTING.md`) and the
/// docs folder (`docs/CONTRIBUTING.md`).
const REPO_WIDE_DIRS: &[&str] = &[".claude", ".github", "docs"];

impl DocContext {
    pub fn new(rel: &str, text: &str) -> DocContext {
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
        let nested =
            !dir.is_empty() && KNOWN_NAMES.contains(&name) && !REPO_WIDE_DIRS.contains(&dir);
        let yaml: Option<Yaml> = frontmatter::split(text)
            .0
            .and_then(|yaml| serde_norway::from_str(yaml).ok());
        let field = |key: &str| yaml.as_ref().and_then(|yaml| yaml.get(key)).and_then(globs);
        let (frontmatter_scope, frontmatter_rung) = if rel.ends_with(".mdc") {
            // Cursor writes `globs: *.tsx`, which is not valid YAML, so read the line itself.
            let line = || {
                frontmatter::split(text)
                    .0?
                    .lines()
                    .find_map(|line| globs(&Yaml::String(line.strip_prefix("globs:")?.to_string())))
            };
            (field("globs").or_else(line), None)
        } else if rel.starts_with(".pragma/specs/") {
            let rung = yaml
                .as_ref()
                .and_then(|yaml| yaml.get("rung"))
                .and_then(Yaml::as_str)
                .and_then(|rung| match rung.trim() {
                    "blocking" => Some(Rung::Blocking),
                    "judged" => Some(Rung::Judged),
                    "context" => Some(Rung::Context),
                    _ => None,
                });
            (field("scope"), rung)
        } else {
            (None, None)
        };
        DocContext {
            rel: rel.to_string(),
            dir: nested.then(|| dir.to_string()),
            frontmatter_scope,
            frontmatter_rung,
        }
    }

    /// A short hash of the scope and rung the doc's frontmatter sets, None when it sets
    /// neither: part of each of its paragraphs' identity for the lock, since frontmatter wins.
    pub fn fingerprint(&self) -> Option<String> {
        if self.frontmatter_scope.is_none() && self.frontmatter_rung.is_none() {
            return None;
        }
        let basis = serde_json::json!({
            "scope": self.frontmatter_scope,
            "rung": self.frontmatter_rung,
        });
        Some(short_hash(basis.to_string().as_bytes()))
    }

    /// How close the doc is to the code it covers: the depth of its directory, 0 for repo-wide
    /// docs. A closer doc takes precedence.
    pub fn depth(&self) -> usize {
        self.dir.as_ref().map_or(0, |dir| dir.split('/').count())
    }

    /// The model's globs, read relative to a nested doc's directory.
    fn text_scope(&self, globs: &[String]) -> Vec<String> {
        let Some(dir) = &self.dir else {
            return globs.to_vec();
        };
        let mut scope: Vec<String> = globs
            .iter()
            .map(|glob| {
                let (bang, glob) = match glob.strip_prefix('!') {
                    Some(rest) => ("!", rest),
                    None => ("", glob.as_str()),
                };
                let glob = glob.trim_start_matches("./").trim_start_matches('/');
                if glob.starts_with(&format!("{dir}/")) {
                    format!("{bang}{glob}")
                } else {
                    format!("{bang}{dir}/{glob}")
                }
            })
            .collect();
        // Only excludes would mean the whole repo but those paths: the doc's directory is the
        // include.
        if !scope.iter().any(|glob| !glob.starts_with('!')) {
            scope.insert(0, format!("{dir}/**"));
        }
        scope
    }
}

/// A frontmatter value as globs: a list, or one string of comma-separated globs (`.mdc`).
fn globs(value: &Yaml) -> Option<Vec<String>> {
    let globs: Vec<String> = match value {
        Yaml::String(text) => split_globs(text),
        Yaml::Sequence(items) => items
            .iter()
            .filter_map(Yaml::as_str)
            .map(str::to_string)
            .collect(),
        _ => return None,
    };
    let globs: Vec<String> = globs
        .into_iter()
        .map(|glob| glob.trim().to_string())
        .filter(|glob| !glob.is_empty())
        .collect();
    (!globs.is_empty()).then_some(globs)
}

/// Splits `a, b` into globs at the commas outside braces: `*.{ts,tsx}` is one glob.
fn split_globs(text: &str) -> Vec<String> {
    let mut globs = Vec::new();
    let (mut depth, mut start) = (0usize, 0);
    for (at, char) in text.char_indices() {
        match char {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                globs.push(text[start..at].to_string());
                start = at + 1;
            }
            _ => {}
        }
    }
    globs.push(text[start..].to_string());
    globs
}

pub fn derive(raw: &RawClause, doc: &DocContext) -> Derived {
    let proposed = match &doc.frontmatter_scope {
        Some(scope) => scope.clone(),
        None => doc.text_scope(&raw.scope),
    };
    let (scope, scope_problems) = validate::scope(&proposed);
    let checked = raw.check.as_ref().map(|proposal| {
        if scope_problems.is_empty() {
            validate::check(proposal, &scope)
        } else {
            Err(scope_problems.join("; "))
        }
    });
    let (check, dropped) = match checked {
        Some(Ok(check)) => (Some(check), None),
        Some(Err(reason)) => (None, Some(reason)),
        None => (None, None),
    };
    let library = matches!(check, Some(Check::Library(_)));
    let derived = match (&check, raw.rung) {
        (Some(Check::Library(_)), _) => Rung::Blocking,
        (Some(Check::Pattern { .. }), _) => Rung::Judged,
        // A check was proposed, so the clause is a norm a diff can show, whatever rung the
        // model gave it: judged, without the check.
        (None, _) if dropped.is_some() => Rung::Judged,
        (None, Rung::Blocking) => Rung::Judged,
        (None, rung) => rung,
    };
    let rung = match doc.frontmatter_rung {
        Some(Rung::Blocking) if library => Rung::Blocking,
        Some(Rung::Blocking) => Rung::Judged,
        Some(rung) => rung,
        None => derived,
    };
    Derived {
        rung,
        scope,
        check,
        dropped,
    }
}

/// R8: a blocking clause whose scope covers no file in the repo protects nothing, and its
/// globs are most likely read from the wrong root (a root doc naming an app's paths). It drops
/// to judged with the reason, and keeps its check for when the scope is corrected. For every
/// library rule the scope is the set of paths it guards; import-boundary's `forbid` globs are
/// import specifiers, not paths, so they are not looked up.
///
/// `files` is the doc-discovery walk, which skips vendored, `node_modules` and gitignored
/// directories, so a scope also counts when an include glob's literal leading path (`vendor`
/// in `vendor/**`, or the whole of `dist/app.js`) exists under `root` and the walk listed
/// nothing under it: "never hand-edit vendor/**" guards real files the walk does not list,
/// while `src/**/*.go` in a walked `src/` with no Go file guards nothing.
pub fn require_files(derived: &mut Derived, files: &[String], root: &Path) {
    if derived.rung != Rung::Blocking {
        return;
    }
    // `derive` keeps only globs that compile (`validate::scope`), so this never fails; were
    // it to, the clause is left as derived rather than judged on a scope that cannot be read.
    let Ok(scope) = Scope::new(&derived.scope) else {
        return;
    };
    if files.iter().any(|file| scope.matches(file)) {
        return;
    }
    // Only where the walk listed nothing: under a directory it walked, no match is real.
    let walked = |prefix: &str| {
        files.iter().any(|file| {
            file.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        })
    };
    let on_disk = derived
        .scope
        .iter()
        .filter(|glob| !glob.starts_with('!'))
        .filter_map(|glob| literal_prefix(glob))
        .any(|prefix| !walked(&prefix) && root.join(prefix).exists());
    if on_disk {
        return;
    }
    derived.rung = Rung::Judged;
    derived.dropped = Some(format!(
        "the scope {} matches no file in the repo, so the clause would block nothing",
        derived.scope.join(", ")
    ));
}

/// The leading path components of `glob` with no glob syntax in them: `vendor` for
/// `vendor/**/*.go`, the whole glob when it has none. None when there is no such component, or
/// when the path could leave the repo.
fn literal_prefix(glob: &str) -> Option<String> {
    if glob.starts_with('/') {
        return None;
    }
    let literal: Vec<&str> = glob
        .split('/')
        .take_while(|part| !part.contains(['*', '?', '[', '{']))
        .collect();
    let leaves = literal.iter().any(|part| *part == ".." || part.is_empty());
    (!literal.is_empty() && !leaves).then(|| literal.join("/"))
}

/// Orders docs closer first, then by path: the order clauses are listed and looked up in.
pub fn closer_first(a: &DocContext, b: &DocContext) -> Ordering {
    b.depth().cmp(&a.depth()).then_with(|| a.rel.cmp(&b.rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::prompt::RawCheck;
    use crate::lock::LibraryRule;

    fn clause(rung: Rung, scope: &[&str], check: Option<RawCheck>) -> RawClause {
        RawClause {
            text: "Do the thing.".into(),
            rung,
            scope: scope.iter().map(ToString::to_string).collect(),
            check,
            fix: "Do it.".into(),
        }
    }

    fn rule(rule: &str) -> RawCheck {
        RawCheck {
            rule: rule.into(),
            pattern: None,
            bad: None,
            good: None,
            forbid: None,
            case: None,
            suffix: None,
        }
    }

    fn require_pattern() -> RawCheck {
        RawCheck {
            pattern: Some("require($$$A)".into()),
            bad: Some("const a = require('a');".into()),
            good: Some("import a from 'a';".into()),
            ..rule("forbidden-pattern")
        }
    }

    fn root() -> DocContext {
        DocContext::new("AGENTS.md", "# Rules\n")
    }

    #[test]
    fn a_library_match_is_blocking() {
        let derived = derive(
            &clause(
                Rung::Context,
                &["drizzle/**"],
                Some(rule("protected-paths")),
            ),
            &root(),
        );
        assert_eq!(derived.rung, Rung::Blocking);
        assert_eq!(
            derived.check,
            Some(Check::Library(LibraryRule::ProtectedPaths))
        );
        assert_eq!(derived.dropped, None);
    }

    #[test]
    fn a_generated_pattern_is_judged_with_its_check() {
        let derived = derive(
            &clause(Rung::Blocking, &["**/*.ts"], Some(require_pattern())),
            &root(),
        );
        assert_eq!(derived.rung, Rung::Judged);
        assert!(matches!(derived.check, Some(Check::Pattern { .. })));
    }

    #[test]
    fn a_check_that_fails_validation_drops_to_judged_with_no_check() {
        let derived = derive(
            &clause(Rung::Blocking, &[], Some(rule("protected-paths"))),
            &root(),
        );
        assert_eq!(derived.rung, Rung::Judged);
        assert_eq!(derived.check, None);
        assert!(derived.dropped.unwrap().contains("needs a scope"));
        let bad_glob = derive(
            &clause(
                Rung::Blocking,
                &["[bad", "src/**"],
                Some(rule("protected-paths")),
            ),
            &root(),
        );
        assert_eq!(bad_glob.rung, Rung::Judged);
        assert_eq!(bad_glob.check, None);
        assert_eq!(bad_glob.scope, vec!["src/**"]);
        assert!(bad_glob.dropped.unwrap().starts_with("scope:"));
    }

    #[test]
    fn a_check_over_the_whole_repo_is_dropped_and_its_clause_judged() {
        let derived = derive(
            &clause(Rung::Blocking, &["**"], Some(rule("protected-paths"))),
            &root(),
        );
        assert_eq!(derived.rung, Rung::Judged);
        assert_eq!(derived.check, None);
        assert!(derived.dropped.unwrap().contains("whole repo"));
    }

    #[test]
    fn a_dropped_check_makes_even_a_context_clause_judged() {
        // The model proposed a check, so the clause is a norm a diff can show, not background.
        let derived = derive(
            &clause(Rung::Context, &[], Some(rule("protected-paths"))),
            &root(),
        );
        assert_eq!(derived.rung, Rung::Judged);
        assert_eq!(derived.check, None);
        assert!(derived.dropped.is_some());
        // Frontmatter still wins.
        let context = DocContext::new(
            ".pragma/specs/background.md",
            "---\nrung: context\n---\nBackground.\n",
        );
        assert_eq!(
            derive(
                &clause(Rung::Context, &[], Some(rule("protected-paths"))),
                &context
            )
            .rung,
            Rung::Context
        );
    }

    #[test]
    fn without_a_check_the_model_rung_stands_but_never_blocks() {
        for (proposed, expected) in [
            (Rung::Blocking, Rung::Judged),
            (Rung::Judged, Rung::Judged),
            (Rung::Context, Rung::Context),
        ] {
            let derived = derive(&clause(proposed, &[], None), &root());
            assert_eq!(derived.rung, expected);
            assert_eq!(derived.dropped, None);
        }
    }

    #[test]
    fn a_nested_doc_scopes_to_its_directory() {
        let doc = DocContext::new("apps/web/AGENTS.md", "# Web\n");
        assert_eq!(doc.dir.as_deref(), Some("apps/web"));
        assert_eq!(doc.depth(), 2);
        assert_eq!(
            derive(&clause(Rung::Judged, &[], None), &doc).scope,
            vec!["apps/web/**"]
        );
        assert_eq!(
            derive(
                &clause(
                    Rung::Judged,
                    &["src/**/*.ts", "!./legacy/**", "apps/web/x/**"],
                    None
                ),
                &doc
            )
            .scope,
            vec![
                "apps/web/src/**/*.ts",
                "!apps/web/legacy/**",
                "apps/web/x/**"
            ]
        );
        // Excluding paths still keeps a nested doc's clause inside its directory.
        assert_eq!(
            derive(&clause(Rung::Judged, &["!legacy/**"], None), &doc).scope,
            vec!["apps/web/**", "!apps/web/legacy/**"]
        );
        // A doc another doc links to is not nested: it applies where its text says.
        let linked = DocContext::new("agents/conventions/ipc.md", "# IPC\n");
        assert_eq!(linked.dir, None);
        assert_eq!(
            derive(&clause(Rung::Judged, &["src/**"], None), &linked).scope,
            vec!["src/**"]
        );
    }

    #[test]
    fn known_names_in_tool_folders_apply_repo_wide() {
        // Claude Code's project memory, GitHub's community files and docs/CONTRIBUTING.md speak
        // for the whole repo, not for the folder they sit in.
        for rel in [
            ".claude/CLAUDE.md",
            ".github/CONTRIBUTING.md",
            ".github/copilot-instructions.md",
            "docs/CONTRIBUTING.md",
            "docs/AGENTS.md",
        ] {
            let doc = DocContext::new(rel, "# Rules\n");
            assert_eq!(doc.dir, None, "{rel}");
            assert_eq!(doc.depth(), 0, "{rel}");
            let derived = derive(
                &clause(
                    Rung::Context,
                    &["drizzle/**"],
                    Some(rule("protected-paths")),
                ),
                &doc,
            );
            assert_eq!(derived.scope, vec!["drizzle/**"], "{rel}");
            assert_eq!(
                derive(&clause(Rung::Judged, &[], None), &doc).scope,
                Vec::<String>::new()
            );
        }
        // Deeper than the folder itself, a known name is a nested doc again.
        let nested = DocContext::new("docs/api/AGENTS.md", "# API\n");
        assert_eq!(nested.dir.as_deref(), Some("docs/api"));
    }

    #[test]
    fn mdc_globs_win_over_the_text() {
        let doc = DocContext::new(
            ".cursor/rules/style.mdc",
            "---\ndescription: style\nglobs: src/**/*.ts, src/**/*.tsx\n---\nUse kebab-case.\n",
        );
        let derived = derive(&clause(Rung::Judged, &["lib/**"], None), &doc);
        assert_eq!(derived.scope, vec!["src/**/*.ts", "src/**/*.tsx"]);
        let always = DocContext::new(
            ".cursor/rules/all.mdc",
            "---\nalwaysApply: true\n---\nBe kind.\n",
        );
        assert_eq!(always.frontmatter_scope, None);
        let cursor = DocContext::new(".cursor/rules/tsx.mdc", "---\nglobs: *.tsx\n---\nX.\n");
        assert_eq!(cursor.frontmatter_scope, Some(vec!["*.tsx".to_string()]));
        // A comma inside braces is alternation, not a second glob.
        let braces = DocContext::new(
            ".cursor/rules/web.mdc",
            "---\nglobs: src/**/*.{ts,tsx}, docs/*.md\n---\nX.\n",
        );
        assert_eq!(
            braces.frontmatter_scope,
            Some(vec![
                "src/**/*.{ts,tsx}".to_string(),
                "docs/*.md".to_string()
            ])
        );
        let quoted = DocContext::new(
            ".pragma/specs/web.md",
            "---\nscope: \"apps/{web,desktop}/**\"\n---\nX.\n",
        );
        assert_eq!(
            quoted.frontmatter_scope,
            Some(vec!["apps/{web,desktop}/**".to_string()])
        );
    }

    #[test]
    fn pragma_specs_frontmatter_sets_scope_and_rung() {
        let doc = DocContext::new(
            ".pragma/specs/naming.md",
            "---\nscope: [\"src/**\"]\nrung: context\n---\nName files in kebab-case.\n",
        );
        let naming = RawCheck {
            case: Some("kebab".into()),
            ..rule("file-naming")
        };
        let derived = derive(&clause(Rung::Blocking, &[], Some(naming.clone())), &doc);
        assert_eq!(derived.scope, vec!["src/**"]);
        assert_eq!(derived.rung, Rung::Context);
        let blocking = DocContext::new(
            ".pragma/specs/naming.md",
            "---\nscope: src/**\nrung: blocking\n---\nName files in kebab-case.\n",
        );
        assert_eq!(
            derive(&clause(Rung::Judged, &[], Some(naming)), &blocking).rung,
            Rung::Blocking
        );
        assert_eq!(
            derive(&clause(Rung::Judged, &[], None), &blocking).rung,
            Rung::Judged,
            "blocking needs a library check"
        );
        // Frontmatter outside .mdc and .pragma/specs is not a scope.
        let other = DocContext::new("docs/x.md", "---\nscope: src/**\n---\nText.\n");
        assert_eq!(other.frontmatter_scope, None);
    }

    fn blocking(scope: &[&str]) -> Derived {
        Derived {
            rung: Rung::Blocking,
            scope: scope.iter().map(ToString::to_string).collect(),
            check: Some(Check::Library(LibraryRule::ProtectedPaths)),
            dropped: None,
        }
    }

    #[test]
    fn a_blocking_clause_needs_a_file_or_a_directory_its_scope_names() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // The walk skips vendored and ignored directories, so these are on disk only.
        for rel in [
            "vendor/x.go",
            "dist/app.js",
            "apps/web/drizzle/0001.sql",
            "src/a.ts",
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x\n").unwrap();
        }
        let files = vec![
            "apps/web/drizzle/0001.sql".to_string(),
            "src/a.ts".to_string(),
        ];
        let kept = |scope: &[&str]| {
            let mut derived = blocking(scope);
            require_files(&mut derived, &files, root);
            (derived.rung, derived.dropped)
        };
        for scope in [
            &["apps/web/drizzle/**"][..],
            &["vendor/**"],
            &["vendor/**/*.go", "!vendor/keep/**"],
            &["dist/**"],
            &["dist/app.js"],
            &["nowhere/**", "vendor/**"],
        ] {
            assert_eq!(kept(scope), (Rung::Blocking, None), "{scope:?}");
        }
        for scope in [
            &["drizzle/**"][..],
            &["**/*.rs"],
            // The walk lists src/, so a missing match there is real: no Go file in a TS tree.
            &["src/**/*.go"],
            &["../outside/**"],
            &["/vendor/**"],
            &["dist/missing.js"],
        ] {
            let (rung, dropped) = kept(scope);
            assert_eq!(rung, Rung::Judged, "{scope:?}");
            assert!(dropped.unwrap().contains("matches no file"), "{scope:?}");
        }
    }

    #[test]
    fn only_frontmatter_scope_or_rung_has_a_fingerprint() {
        let plain = DocContext::new("AGENTS.md", "---\ntitle: x\n---\nText.\n");
        assert_eq!(plain.fingerprint(), None);
        let narrow = DocContext::new(".cursor/rules/db.mdc", "---\nglobs: drizzle/**\n---\nX.\n");
        let narrower = DocContext::new(
            ".cursor/rules/db.mdc",
            "---\nglobs: drizzle/0001.sql\n---\nX.\n",
        );
        let judged = DocContext::new(".pragma/specs/db.md", "---\nrung: judged\n---\nX.\n");
        let context = DocContext::new(".pragma/specs/db.md", "---\nrung: context\n---\nX.\n");
        let fingerprints = [&narrow, &narrower, &judged, &context].map(|doc| doc.fingerprint());
        assert!(fingerprints.iter().all(Option::is_some));
        let distinct: std::collections::HashSet<_> = fingerprints.iter().collect();
        assert_eq!(distinct.len(), 4);
        assert_eq!(
            narrow.fingerprint(),
            DocContext::new(".cursor/rules/db.mdc", "---\nglobs: drizzle/**\n---\nY.\n")
                .fingerprint()
        );
    }

    #[test]
    fn closer_docs_come_first() {
        let mut docs = [
            DocContext::new("AGENTS.md", ""),
            DocContext::new("apps/web/src/AGENTS.md", ""),
            DocContext::new("apps/web/AGENTS.md", ""),
            DocContext::new("CLAUDE.md", ""),
        ];
        docs.sort_by(closer_first);
        let order: Vec<&str> = docs.iter().map(|doc| doc.rel.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "apps/web/src/AGENTS.md",
                "apps/web/AGENTS.md",
                "AGENTS.md",
                "CLAUDE.md"
            ]
        );
    }
}
