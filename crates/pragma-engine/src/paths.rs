//! Path normalization, so a rule cannot be dodged by spelling a path differently.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Resolves `.` and `..` lexically, without touching the filesystem.
pub fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolves `input` against `cwd`, cleans `..`, and resolves symlinks in the longest prefix that
/// exists. The file itself may not exist yet, as with a Write that creates it.
pub fn normalize(cwd: &Path, input: &Path) -> PathBuf {
    let joined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        cwd.join(input)
    };
    let cleaned = lexical_clean(&joined);
    let mut existing = cleaned.clone();
    let mut missing: Vec<OsString> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&existing) {
            let mut out = real;
            for part in missing.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
            return cleaned;
        };
        missing.push(name);
        if !existing.pop() {
            return cleaned;
        }
    }
}

/// `abs` relative to `root` with `/` separators, or None when `abs` is not inside `root`.
pub fn repo_relative(root: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Joins a repo-relative `base` directory and a relative `path`, resolving `.` and `..`.
/// None when the result would leave the repo.
pub fn join_relative(base: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_clean_resolves_dots() {
        assert_eq!(
            lexical_clean(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(lexical_clean(Path::new("a/../../b")), PathBuf::from("../b"));
        assert_eq!(lexical_clean(Path::new("/../a")), PathBuf::from("/a"));
    }

    #[test]
    fn normalize_resolves_relative_paths_against_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let out = normalize(&root.join("src"), Path::new("../src/./new.ts"));
        assert_eq!(out, root.join("src/new.ts"));
    }

    #[cfg(unix)]
    #[test]
    fn normalize_resolves_symlinks_in_existing_parents() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("real/drizzle")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let out = normalize(&root, Path::new("link/drizzle/0001.sql"));
        assert_eq!(out, root.join("real/drizzle/0001.sql"));
    }

    #[test]
    fn repo_relative_uses_forward_slashes_and_rejects_outside_paths() {
        let root = Path::new("/repo");
        assert_eq!(
            repo_relative(root, Path::new("/repo/src/a.ts")).as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(repo_relative(root, Path::new("/elsewhere/a.ts")), None);
        assert_eq!(repo_relative(root, Path::new("/repo")), None);
    }

    #[test]
    fn join_relative_resolves_dots_and_refuses_to_leave_the_repo() {
        assert_eq!(
            join_relative("docs/guides", "../api/a.md").as_deref(),
            Some("docs/api/a.md")
        );
        assert_eq!(join_relative("", "./src/a.ts").as_deref(), Some("src/a.ts"));
        assert_eq!(join_relative("docs", "../../outside.md"), None);
    }
}
