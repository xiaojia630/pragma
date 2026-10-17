//! What exists in a repo, for drift: files, directories, names, package roots, scripts, make
//! targets, and (built on first use) every identifier in its code and config.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::gitignore::Gitignore;

use serde_json::Value;

const MANIFESTS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "go.mod",
    "pyproject.toml",
    "composer.json",
    "Gemfile",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "deno.json",
    "pubspec.yaml",
];
const IDENTIFIER_SOURCES: &[&str] = &[
    "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "vue", "svelte", "rs", "py", "go",
    "java", "kt", "swift", "php", "rb", "cs", "dart", "scala", "ex", "exs", "c", "h", "cpp", "hpp",
    "sql", "json", "yml", "yaml", "toml", "sh",
];
const MAX_SCANNED_BYTES: u64 = 1024 * 1024;

pub struct RepoIndex {
    root: PathBuf,
    pub files: HashSet<String>,
    pub dirs: HashSet<String>,
    by_name: HashMap<String, Vec<String>>,
    /// Files by the start of their name, up to its first dot: `Foo` holds `design/Foo` for
    /// `design/Foo.dc.html`.
    by_stem: HashMap<String, Vec<String>>,
    /// Directories holding a package manifest; "" is the repo root.
    pub package_roots: Vec<String>,
    pub scripts: HashSet<String>,
    pub make_targets: HashSet<String>,
    identifiers: OnceCell<HashSet<String>>,
    gitignores: OnceCell<Vec<Gitignore>>,
}

impl RepoIndex {
    pub fn build(root: &Path) -> RepoIndex {
        let files = crate::docs::walk_files(root);
        let mut dirs = HashSet::new();
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        let mut by_stem: HashMap<String, Vec<String>> = HashMap::new();
        let mut scripts = HashSet::new();
        let mut make_targets = HashSet::new();
        for rel in &files {
            let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
            by_name
                .entry(name.to_string())
                .or_default()
                .push(rel.clone());
            if let Some(stem) = stem(name) {
                let without_extension = rel.strip_suffix(&name[stem.len()..]).unwrap_or(rel);
                by_stem
                    .entry(stem.to_string())
                    .or_default()
                    .push(without_extension.to_string());
            }
            let mut parent = dir;
            while !parent.is_empty() && dirs.insert(parent.to_string()) {
                let dir_name = parent.rsplit('/').next().unwrap_or(parent);
                by_name
                    .entry(dir_name.to_string())
                    .or_default()
                    .push(parent.to_string());
                parent = parent.rsplit_once('/').map(|(up, _)| up).unwrap_or("");
            }
            if name == "package.json" {
                let text = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
                if let Ok(Value::Object(manifest)) = serde_json::from_str::<Value>(&text)
                    && let Some(Value::Object(names)) = manifest.get("scripts")
                {
                    scripts.extend(names.keys().cloned());
                }
            }
            if name == "Makefile" {
                let text = std::fs::read_to_string(root.join(rel)).unwrap_or_default();
                make_targets.extend(text.lines().filter_map(make_target));
            }
        }
        let package_roots = package_roots(&files);
        RepoIndex {
            root: root.to_path_buf(),
            files: files.into_iter().collect(),
            dirs,
            by_name,
            by_stem,
            package_roots,
            scripts,
            make_targets,
            identifiers: OnceCell::new(),
            gitignores: OnceCell::new(),
        }
    }

    /// The repo's .gitignore files would ignore `rel` if it existed.
    pub fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        let path = self.root.join(rel);
        let gitignores = self.gitignores.get_or_init(|| {
            self.files
                .iter()
                .filter(|file| file.rsplit('/').next() == Some(".gitignore"))
                .map(|file| Gitignore::new(self.root.join(file)).0)
                .collect()
        });
        gitignores.iter().any(|gitignore| {
            path.starts_with(gitignore.path())
                && gitignore
                    .matched_path_or_any_parents(&path, is_dir)
                    .is_ignore()
        })
    }

    /// A file or directory exists at `rel`. Gitignored paths are checked on disk.
    pub fn exists(&self, rel: &str) -> bool {
        let rel = rel.trim_end_matches('/');
        rel.is_empty()
            || self.files.contains(rel)
            || self.dirs.contains(rel)
            || self.root.join(rel).exists()
    }

    /// Every file and directory with this name.
    pub fn named(&self, name: &str) -> &[String] {
        self.by_name.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every file named `name` plus an extension, as its path without the extension: for a doc
    /// that leaves the extension off, `design/Foo` for `design/Foo.dc.html`. Empty when `name`
    /// has an extension of its own, since `a.ts` does not stand for `a.ts.map`.
    pub fn named_with_extension(&self, name: &str) -> &[String] {
        if stem(name).is_some() {
            return &[];
        }
        self.by_stem.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn has_identifier(&self, name: &str) -> bool {
        self.identifiers
            .get_or_init(|| self.scan_identifiers())
            .contains(name)
    }

    fn scan_identifiers(&self) -> HashSet<String> {
        let mut found = HashSet::new();
        for rel in &self.files {
            let ext = rel.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
            if !IDENTIFIER_SOURCES.contains(&ext) {
                continue;
            }
            let path = self.root.join(rel);
            if std::fs::metadata(&path)
                .map(|meta| meta.len() > MAX_SCANNED_BYTES)
                .unwrap_or(true)
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let mut current = String::new();
            for ch in text.chars().chain(std::iter::once(' ')) {
                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                    current.push(ch);
                } else if !current.is_empty() {
                    found.insert(std::mem::take(&mut current));
                }
            }
        }
        found
    }
}

/// A file name up to its first dot, not counting a leading one: `Foo` for `Foo.dc.html` and
/// `.env` for `.env.example`. None when the name has no extension, like `Makefile`.
fn stem(name: &str) -> Option<&str> {
    let dot = name.char_indices().skip(1).find(|&(_, c)| c == '.')?.0;
    Some(&name[..dot])
}

fn make_target(line: &str) -> Option<String> {
    if line.starts_with(['\t', ' ', '.', '#']) {
        return None;
    }
    let (name, rest) = line.split_once(':')?;
    let ok = !name.is_empty()
        && !rest.starts_with('=')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'));
    ok.then(|| name.to_string())
}

/// Directories among `files` (repo-relative) holding a package manifest, sorted; "" is the repo
/// root.
pub fn package_roots(files: &[String]) -> Vec<String> {
    let mut roots: Vec<String> = files
        .iter()
        .filter_map(|rel| {
            let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
            MANIFESTS.contains(&name).then(|| dir.to_string())
        })
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn indexes_files_dirs_names_packages_scripts_and_targets() {
        let (_temp, root) = crate::testutil::repo();
        fs::create_dir_all(root.join("apps/web/src")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"scripts": {"dev": "x", "db:fixtures": "y"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("apps/web/package.json"),
            r#"{"scripts": {"doctor": "z"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("apps/web/src/task-selectors.ts"),
            "export function getTaskManagerStore() {}\n",
        )
        .unwrap();
        fs::write(
            root.join("Makefile"),
            "build:\n\tcargo build\nVAR := 1\n.PHONY: build\n",
        )
        .unwrap();
        let index = RepoIndex::build(&root);
        assert!(index.exists("apps/web/src/task-selectors.ts"));
        assert!(index.exists("apps/web/src/"));
        assert!(!index.exists("apps/web/lib"));
        assert_eq!(
            index.named("task-selectors.ts"),
            ["apps/web/src/task-selectors.ts"]
        );
        assert_eq!(index.named("src"), ["apps/web/src"]);
        assert_eq!(index.package_roots, vec!["", "apps/web"]);
        assert!(index.scripts.contains("db:fixtures") && index.scripts.contains("doctor"));
        assert_eq!(index.make_targets, HashSet::from(["build".to_string()]));
        assert!(index.has_identifier("getTaskManagerStore"));
        assert!(!index.has_identifier("getProjectStoreThatDoesNotExist"));
    }

    #[test]
    fn finds_code_names_in_php_ruby_csharp_and_vue_sources() {
        let (_temp, root) = crate::testutil::repo();
        fs::create_dir_all(root.join("api/app")).unwrap();
        fs::create_dir_all(root.join("web/src")).unwrap();
        fs::write(
            root.join("api/app/MatrixScenario.php"),
            "<?php\nclass MatrixScenario {}\n",
        )
        .unwrap();
        fs::write(
            root.join("api/app/booking.rb"),
            "class BookingPolicy\nend\n",
        )
        .unwrap();
        fs::write(
            root.join("api/app/Invoice.cs"),
            "public class InvoiceTotals {}\n",
        )
        .unwrap();
        fs::write(
            root.join("web/src/Cart.vue"),
            "<script>export const cartTotal = 1</script>\n",
        )
        .unwrap();
        let index = RepoIndex::build(&root);
        for name in [
            "MatrixScenario",
            "BookingPolicy",
            "InvoiceTotals",
            "cartTotal",
        ] {
            assert!(index.has_identifier(name), "{name} should be found");
        }
    }

    #[test]
    fn package_roots_are_the_dirs_with_a_manifest() {
        let files = [
            "crates/cli/Cargo.toml",
            "package.json",
            "apps/web/package.json",
            "apps/web/src/package.json.ts",
            "Cargo.toml",
        ]
        .map(String::from);
        assert_eq!(package_roots(&files), ["", "apps/web", "crates/cli"]);
    }

    #[test]
    fn package_roots_include_php_ruby_jvm_deno_and_dart_packages() {
        let files = [
            "api/composer.json",
            "svc/Gemfile",
            "jvm/pom.xml",
            "app/build.gradle",
            "kt/build.gradle.kts",
            "edge/deno.json",
            "mobile/pubspec.yaml",
            // Lock files and look-alikes do not make a package.
            "locks/composer.lock",
            "locks/Gemfile.lock",
            "locks/pubspec.lock",
            "notes/composer.json.md",
        ]
        .map(String::from);
        assert_eq!(
            package_roots(&files),
            ["api", "app", "edge", "jvm", "kt", "mobile", "svc"]
        );
    }

    #[test]
    fn finds_files_by_their_name_without_the_extension() {
        let (_temp, root) = crate::testutil::repo();
        fs::create_dir_all(root.join("design/screens/Foo")).unwrap();
        fs::write(root.join("design/screens/Foo.dc.html"), "x\n").unwrap();
        fs::write(root.join("design/screens/Foo/index.ts"), "x\n").unwrap();
        fs::write(root.join(".env.example"), "X=1\n").unwrap();
        fs::write(root.join("Makefile"), "build:\n").unwrap();
        let index = RepoIndex::build(&root);
        // The path comes back without the extension, however many dots the extension has.
        assert_eq!(index.named_with_extension("Foo"), ["design/screens/Foo"]);
        assert_eq!(
            index.named_with_extension("index"),
            ["design/screens/Foo/index"]
        );
        assert_eq!(index.named_with_extension(".env"), [".env"]);
        // A name that has an extension of its own does not stand for a longer one.
        assert!(index.named_with_extension("Foo.dc").is_empty());
        assert!(index.named_with_extension("index.ts").is_empty());
        // Neither does a file with no extension, a folder, or a name that is not there.
        assert!(index.named_with_extension("Makefile").is_empty());
        assert!(index.named_with_extension("screens").is_empty());
        assert!(index.named_with_extension("Missing").is_empty());
    }

    #[test]
    fn knows_what_gitignore_files_cover() {
        let (_temp, root) = crate::testutil::repo();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::write(root.join(".gitignore"), "dist/\n").unwrap();
        fs::write(root.join("apps/web/.gitignore"), "/out/\n").unwrap();
        let index = RepoIndex::build(&root);
        assert!(index.is_ignored("packages/x/dist/index.mjs", false));
        assert!(index.is_ignored("apps/web/out", true));
        assert!(!index.is_ignored("out/x.js", false));
        assert!(!index.is_ignored("src/a.ts", false));
    }

    #[test]
    fn checks_gitignored_paths_on_disk() {
        let (_temp, root) = crate::testutil::repo();
        fs::write(root.join(".gitignore"), ".env\n").unwrap();
        fs::write(root.join(".env"), "X=1\n").unwrap();
        let index = RepoIndex::build(&root);
        assert!(!index.files.contains(".env"));
        assert!(index.exists(".env"));
    }
}
