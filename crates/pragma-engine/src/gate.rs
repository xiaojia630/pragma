//! The gate every hook passes first: pragma acts only in enabled, unpaused repos, and never
//! inside its own model calls.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::repo::Repo;

#[derive(Debug)]
pub enum Gate {
    On { repo: Repo, pragma_dir: PathBuf },
    Off(&'static str),
}

pub fn gate(cwd: &Path, env: &impl Env) -> Gate {
    if env.var("PRAGMA_JUDGE").is_some() {
        return Gate::Off("PRAGMA_JUDGE is set");
    }
    if env.var("PRAGMA_DISABLE").is_some() {
        return Gate::Off("PRAGMA_DISABLE is set");
    }
    let Some(repo) = Repo::discover(cwd) else {
        return Gate::Off("not inside a git repository");
    };
    let Some(pragma_dir) = repo.effective_pragma_dir() else {
        return Gate::Off("this repo has no .pragma/");
    };
    if is_paused(&repo) {
        return Gate::Off("pragma is paused for this repo");
    }
    Gate::On { repo, pragma_dir }
}

fn paused_marker(repo: &Repo) -> PathBuf {
    repo.shared_state_dir().join("paused")
}

pub fn is_paused(repo: &Repo) -> bool {
    paused_marker(repo).exists()
}

/// Pauses or resumes pragma for this repo on this machine.
pub fn set_paused(repo: &Repo, paused: bool) -> io::Result<()> {
    let marker = paused_marker(repo);
    if paused {
        fs::create_dir_all(repo.shared_state_dir())?;
        fs::write(marker, "")
    } else {
        match fs::remove_file(marker) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::repo;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn reason(gate: Gate) -> &'static str {
        match gate {
            Gate::On { .. } => "on",
            Gate::Off(reason) => reason,
        }
    }

    #[test]
    fn is_on_in_an_enabled_repo() {
        let (_temp, root) = repo();
        fs::create_dir_all(root.join(".pragma")).unwrap();
        assert_eq!(reason(gate(&root, &env(&[]))), "on");
    }

    #[test]
    fn is_off_without_pragma_dir_or_outside_a_repo() {
        let (_temp, root) = repo();
        assert_eq!(reason(gate(&root, &env(&[]))), "this repo has no .pragma/");
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(
            reason(gate(outside.path(), &env(&[]))),
            "not inside a git repository"
        );
    }

    #[test]
    fn is_off_for_judge_calls_disable_and_pause() {
        let (_temp, root) = repo();
        fs::create_dir_all(root.join(".pragma")).unwrap();
        assert_eq!(
            reason(gate(&root, &env(&[("PRAGMA_JUDGE", "1")]))),
            "PRAGMA_JUDGE is set"
        );
        assert_eq!(
            reason(gate(&root, &env(&[("PRAGMA_DISABLE", "1")]))),
            "PRAGMA_DISABLE is set"
        );
        let found = Repo::discover(&root).unwrap();
        set_paused(&found, true).unwrap();
        assert_eq!(
            reason(gate(&root, &env(&[]))),
            "pragma is paused for this repo"
        );
        set_paused(&found, false).unwrap();
        set_paused(&found, false).unwrap();
        assert_eq!(reason(gate(&root, &env(&[]))), "on");
    }
}
