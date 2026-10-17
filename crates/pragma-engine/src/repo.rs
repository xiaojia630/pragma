//! Finds the repo around a directory by reading `.git` files directly. Starting `git` costs more
//! than the edit check's whole budget, so this module never spawns a process.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// This worktree's top-level directory.
    pub toplevel: PathBuf,
    /// This worktree's git dir: `.git`, or `.git/worktrees/<name>` for a linked worktree.
    pub git_dir: PathBuf,
    /// The git dir every worktree of the repo shares.
    pub common_dir: PathBuf,
    /// The main worktree's top-level directory, when the repo has one (it is not bare).
    pub root_checkout: Option<PathBuf>,
}

impl Repo {
    /// The repo containing `start`, or None outside a repo.
    pub fn discover(start: &Path) -> Option<Repo> {
        let start = fs::canonicalize(start).ok()?;
        let mut dir = start.as_path();
        loop {
            let dotgit = dir.join(".git");
            if dotgit.is_dir() {
                return Some(Repo::from_git_dir(dir, dotgit));
            }
            if dotgit.is_file() {
                return Some(Repo::from_git_dir(dir, read_gitdir_file(&dotgit)?));
            }
            dir = dir.parent()?;
        }
    }

    fn from_git_dir(toplevel: &Path, git_dir: PathBuf) -> Repo {
        let git_dir = fs::canonicalize(&git_dir).unwrap_or(git_dir);
        let common_dir = fs::read_to_string(git_dir.join("commondir"))
            .ok()
            .and_then(|text| fs::canonicalize(git_dir.join(text.trim())).ok())
            .unwrap_or_else(|| git_dir.clone());
        let root_checkout = if common_dir.file_name().is_some_and(|name| name == ".git") {
            common_dir.parent().map(Path::to_path_buf)
        } else {
            None
        };
        Repo {
            toplevel: toplevel.to_path_buf(),
            git_dir,
            common_dir,
            root_checkout,
        }
    }

    /// The `.pragma/` that applies here: the root checkout's when it has one, else this
    /// worktree's own.
    pub fn effective_pragma_dir(&self) -> Option<PathBuf> {
        self.root_checkout
            .iter()
            .chain(std::iter::once(&self.toplevel))
            .map(|dir| dir.join(".pragma"))
            .find(|dir| dir.is_dir())
    }

    /// Where pragma writes `.pragma/`: only ever the root checkout (or this worktree when the
    /// repo has no root checkout).
    pub fn writable_pragma_dir(&self) -> PathBuf {
        self.root_checkout
            .as_ref()
            .unwrap_or(&self.toplevel)
            .join(".pragma")
    }

    /// This worktree's own state: `<git dir>/pragma`.
    pub fn state_dir(&self) -> PathBuf {
        self.git_dir.join("pragma")
    }

    /// State shared by every worktree of the repo: `<common dir>/pragma`.
    pub fn shared_state_dir(&self) -> PathBuf {
        self.common_dir.join("pragma")
    }
}

/// Reads a `.git` file (`gitdir: <path>`), resolving a relative path against its directory.
fn read_gitdir_file(dotgit: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(dotgit).ok()?;
    let target = Path::new(
        text.lines()
            .find_map(|line| line.strip_prefix("gitdir:"))?
            .trim(),
    );
    if target.is_absolute() {
        Some(target.to_path_buf())
    } else {
        Some(dotgit.parent()?.join(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{git, repo};

    #[test]
    fn discovers_a_plain_repo_from_a_subdirectory() {
        let (_temp, root) = repo();
        fs::create_dir_all(root.join("src/deep")).unwrap();
        let found = Repo::discover(&root.join("src/deep")).unwrap();
        assert_eq!(found.toplevel, root);
        assert_eq!(found.git_dir, root.join(".git"));
        assert_eq!(found.common_dir, root.join(".git"));
        assert_eq!(found.root_checkout, Some(root.clone()));
    }

    #[test]
    fn discovers_a_linked_worktree_and_its_root_checkout() {
        let (temp, root) = repo();
        git(&root, &["worktree", "add", "-q", "../task", "-b", "task"]);
        let task = temp.path().canonicalize().unwrap().join("task");
        let found = Repo::discover(&task).unwrap();
        assert_eq!(found.toplevel, task);
        assert_eq!(found.git_dir, root.join(".git/worktrees/task"));
        assert_eq!(found.common_dir, root.join(".git"));
        assert_eq!(found.root_checkout, Some(root.clone()));
        assert_eq!(found.state_dir(), root.join(".git/worktrees/task/pragma"));
        assert_eq!(found.shared_state_dir(), root.join(".git/pragma"));
    }

    #[test]
    fn effective_pragma_dir_prefers_the_root_checkout() {
        let (temp, root) = repo();
        git(&root, &["worktree", "add", "-q", "../task", "-b", "task"]);
        let task = temp.path().canonicalize().unwrap().join("task");
        fs::create_dir_all(task.join(".pragma")).unwrap();
        let found = Repo::discover(&task).unwrap();
        assert_eq!(found.effective_pragma_dir(), Some(task.join(".pragma")));
        fs::create_dir_all(root.join(".pragma")).unwrap();
        assert_eq!(found.effective_pragma_dir(), Some(root.join(".pragma")));
        assert_eq!(found.writable_pragma_dir(), root.join(".pragma"));
    }

    #[test]
    fn returns_none_outside_a_repo() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(Repo::discover(temp.path()), None);
    }
}
