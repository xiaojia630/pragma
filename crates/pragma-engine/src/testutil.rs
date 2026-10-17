//! Helpers for tests that need a real git repo.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=pragma",
            "-c",
            "user.email=pragma@example.com",
        ])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

/// A temp dir holding a repo at `<temp>/main` with one commit.
pub fn repo() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp
        .path()
        .canonicalize()
        .expect("canonical temp")
        .join("main");
    fs::create_dir_all(&root).expect("repo dir");
    git(&root, &["init", "-q"]);
    fs::write(root.join("README.md"), "# test\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "init"]);
    (temp, root)
}

/// `git --version` as (major, minor); None when git does not run or answers in another form.
pub fn git_version() -> Option<(u32, u32)> {
    let output = Command::new("git").arg("--version").output().ok()?;
    parse_git_version(&String::from_utf8_lossy(&output.stdout))
}

/// The first two numbers of "git version 2.43.0", "git version 2.39.5 (Apple Git-154)" and
/// "git version 2.45.1.windows.1".
fn parse_git_version(text: &str) -> Option<(u32, u32)> {
    let version = text.trim().strip_prefix("git version ")?;
    let mut numbers = version.split(|c: char| !c.is_ascii_digit());
    Some((numbers.next()?.parse().ok()?, numbers.next()?.parse().ok()?))
}

/// Whether a test of a partial clone can run here: false, after printing why, when git is older
/// than 2.44. Backtest keeps a partial clone from fetching its missing objects with
/// `GIT_NO_LAZY_FETCH`, which git honors only from 2.44 on; an older git fetches them all, and
/// then nothing is missing for the test to find.
pub fn supports_no_lazy_fetch(test: &str) -> bool {
    match git_version() {
        Some(version) if version < (2, 44) => {
            // Straight to stderr: the harness swallows `eprintln!` from a passing test, and a
            // skipped test should show in the log.
            let _ = writeln!(
                std::io::stderr(),
                "skipping {test}: git {}.{} is older than 2.44, the first to honor GIT_NO_LAZY_FETCH",
                version.0,
                version.1
            );
            false
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_first_two_numbers_of_the_versions_git_prints() {
        for (text, version) in [
            ("git version 2.43.0\n", (2, 43)),
            ("git version 2.54.0 (Apple Git-157)\n", (2, 54)),
            ("git version 2.39.5 (Apple Git-154)", (2, 39)),
            ("git version 2.45.1.windows.1", (2, 45)),
            ("git version 2.44.0-rc1", (2, 44)),
        ] {
            assert_eq!(parse_git_version(text), Some(version), "{text}");
        }
        for text in [
            "",
            "2.43.0",
            "git version",
            "git version x.y",
            "git version 2",
        ] {
            assert_eq!(parse_git_version(text), None, "{text}");
        }
    }
}
