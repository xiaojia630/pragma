//! Shared helpers for tests that run the real `pragma` binary against temporary git repos.

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

/// The built binary, with pragma's own environment variables cleared.
pub fn pragma() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_pragma"));
    for key in [
        "PRAGMA_JUDGE",
        "PRAGMA_DISABLE",
        "PRAGMA_HOME",
        "CLAUDE_CONFIG_DIR",
    ] {
        command.env_remove(key);
    }
    // Tests never call a real model: a test that wants one points this at a fake.
    command.env("PRAGMA_CLAUDE_BIN", "/nonexistent/pragma-test-claude");
    command
}

pub struct TestRepo {
    temp: tempfile::TempDir,
    pub root: PathBuf,
}

impl TestRepo {
    /// A repo at `<temp>/main` with one commit.
    pub fn new() -> TestRepo {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("main");
        fs::create_dir_all(&root).unwrap();
        let repo = TestRepo { temp, root };
        repo.git(&["init", "-q"]);
        repo.write("README.md", "# test\n");
        repo.commit();
        repo
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.temp.path().canonicalize().unwrap()
    }

    pub fn git(&self, args: &[&str]) {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=pragma",
                "-c",
                "user.email=pragma@example.com",
            ])
            .args(["-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(&self.root)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    pub fn commit(&self) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", "change"]);
    }

    pub fn write(&self, rel: &str, text: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).unwrap()
    }

    /// Writes `.pragma/pragma.lock` holding `clauses`.
    pub fn lock(&self, clauses: Value) {
        self.write(
            ".pragma/pragma.lock",
            &json!({ "format": 1, "clauses": clauses }).to_string(),
        );
    }

    pub fn run(&self, args: &[&str]) -> Output {
        pragma()
            .args(args)
            .current_dir(&self.root)
            .output()
            .unwrap()
    }

    /// Runs `pragma hook pre-tool-use` with `input` on stdin, as Claude Code would.
    pub fn hook(&self, input: Value, env: &[(&str, &str)]) -> Output {
        let mut child = pragma()
            .args(["hook", "pre-tool-use"])
            .envs(env.iter().copied())
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// Hook input for an Edit tool call on `rel`.
    pub fn edit_input(&self, rel: &str, old: &str, new: &str) -> Value {
        json!({
            "session_id": "session-1",
            "cwd": self.root,
            "hook_event_name": "PreToolUse",
            "tool_name": "Edit",
            "tool_input": { "file_path": self.root.join(rel), "old_string": old, "new_string": new }
        })
    }

    /// Hook input for a Write tool call on `rel`.
    pub fn write_input(&self, rel: &str, content: &str) -> Value {
        json!({
            "session_id": "session-1",
            "cwd": self.root,
            "hook_event_name": "PreToolUse",
            "tool_name": "Write",
            "tool_input": { "file_path": self.root.join(rel), "content": content }
        })
    }
}

/// Holds the repo's extraction lock as a live `pragma extract` would: the OS lock on
/// `extract.guard` and the holder's record in `extract.lock`. Dropping the file lets go.
pub fn hold_extract_lock(repo: &TestRepo, pid: u32, started_at: u64) -> fs::File {
    let state = repo.root.join(".git/pragma");
    fs::create_dir_all(&state).unwrap();
    let guard = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(state.join("extract.guard"))
        .unwrap();
    guard.try_lock().unwrap();
    fs::write(
        state.join("extract.lock"),
        json!({ "pid": pid, "started_at": started_at }).to_string(),
    )
    .unwrap();
    guard
}

pub fn anchor(line: u32) -> Value {
    json!({ "path": "AGENTS.md", "heading_path": ["Rules"], "para_hash": format!("p{line}"),
            "lines": [line, line], "quote": "" })
}

/// A forbidden-pattern clause.
pub fn pattern_clause(id: &str, pattern: &str, scope: &[&str], bad: &str, good: &str) -> Value {
    json!({
        "id": id,
        "anchor": anchor(10),
        "text": format!("Never write {pattern}."),
        "scope": scope,
        "check": { "kind": "library", "rule": "forbidden-pattern", "pattern": pattern,
                   "examples": { "bad": bad, "good": good } },
        "fix": "Use an explicit null check."
    })
}

/// A protected-paths clause.
pub fn protected_clause(id: &str, scope: &[&str]) -> Value {
    json!({
        "id": id,
        "anchor": anchor(20),
        "text": "Do not hand-edit migrations.",
        "scope": scope,
        "check": { "kind": "library", "rule": "protected-paths" },
        "fix": "Run the generator."
    })
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

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A repo whose CONTRIBUTING.md names one moved file and one deleted file.
pub fn drift_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.write("package.json", r#"{"scripts": {"dev": "x"}}"#);
    repo.write("apps/app/package.json", "{}");
    repo.write(
        "apps/app/src/tasks/api/task-selectors.ts",
        "export function getTaskManagerStore() {}\n",
    );
    repo.write(
        "CONTRIBUTING.md",
        "# Contributing\n\nRun `pnpm run dev`. Selectors live in `src/tasks/browser/task-selectors.ts`.\n\n\
         The old IPC lives in `src/renderer/lib/ipc.ts`.\n",
    );
    repo.commit();
    repo
}

/// A repo with a `require()` rule and two violations of it.
pub fn audit_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.lock(json!([pattern_clause(
        "no-require",
        "require($$$A)",
        &["**/*.ts"],
        "require('a');",
        "import a from 'a';"
    )]));
    repo.write(
        "src/a.ts",
        "const a = require('a');\n// require() in a comment is fine\n",
    );
    repo.write(
        "src/codegen.ts",
        "const kit = require('drizzle-kit/api');\n",
    );
    repo.commit();
    repo
}

/// Writes an executable fake `claude` to `dir` that appends a line to `dir/calls` per call and
/// then runs `body` (sh), which sees the prompt on stdin.
#[cfg(unix)]
pub fn fake_claude(dir: &std::path::Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("claude");
    let script = format!(
        "#!/bin/sh\necho call >> \"{}/calls\"\n{body}\n",
        dir.display()
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A fake `claude` with canned clauses per paragraph text: "migrations" gives a protected
/// path, "require" a generated pattern, "everywhere" a protected path with no scope (its check
/// does not validate), "Background" a context clause, anything else none.
#[cfg(unix)]
pub fn canned_claude(dir: &std::path::Path) -> PathBuf {
    fake_claude(
        dir,
        r#"out=''
sep=''
while IFS= read -r line; do
  case "$line" in '{"id":"'*) ;; *) continue ;; esac
  id=$(printf '%s' "$line" | sed 's/^{"id":"\([^"]*\)".*/\1/')
  case "$line" in
    *migrations*) c='{"text":"Do not hand-edit migrations.","rung":"blocking","scope":["drizzle/**"],"check":{"rule":"protected-paths"},"fix":"Run the generator."}' ;;
    *require*) c='{"text":"Do not use require().","rung":"blocking","scope":["**/*.ts"],"check":{"rule":"forbidden-pattern","pattern":"require($$$A)","bad":"const a = require(\"a\");","good":"import a from \"a\";"},"fix":"Use import."}' ;;
    *everywhere*) c='{"text":"Protect everything.","rung":"blocking","scope":[],"check":{"rule":"protected-paths"},"fix":"Do not."}' ;;
    *Background*) c='{"text":"This is an Electron app.","rung":"context","scope":[],"check":null,"fix":"None."}' ;;
    *) c='' ;;
  esac
  out="$out$sep{\"id\":\"$id\",\"clauses\":[$c]}"
  sep=','
done
printf '{"type":"result","is_error":false,"result":"","structured_output":{"paragraphs":[%s]}}\n' "$out""#,
    )
}

/// Puts a `git` in front of the real one on `command`'s PATH that logs each call as
/// `GIT_OPTIONAL_LOCKS=<value or unset> <args>` to `dir/git.log` before running the real git.
#[cfg(unix)]
pub fn log_git_calls(command: &mut Command, dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var("PATH").unwrap_or_default();
    let real = path
        .split(':')
        .map(|entry| std::path::Path::new(entry).join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git on PATH");
    fs::create_dir_all(dir).unwrap();
    let shim = dir.join("git");
    let script = format!(
        "#!/bin/sh\necho \"GIT_OPTIONAL_LOCKS=${{GIT_OPTIONAL_LOCKS:-unset}} $*\" >> \"{}/git.log\"\n\
         exec \"{}\" \"$@\"\n",
        dir.display(),
        real.display()
    );
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    command.env("PATH", format!("{}:{path}", dir.display()));
}

/// The git calls `log_git_calls` logged in `dir`, one per line; empty when there were none.
#[cfg(unix)]
pub fn logged_git_calls(dir: &std::path::Path) -> Vec<String> {
    fs::read_to_string(dir.join("git.log"))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A fake `claude` that answers as claude 2.1.301 does when no account is logged in.
#[cfg(unix)]
pub fn logged_out_claude(dir: &std::path::Path) -> PathBuf {
    fake_claude(
        dir,
        r#"cat > /dev/null
echo '{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}'
exit 1"#,
    )
}

/// Model calls the fake in `dir` has answered.
pub fn fake_calls(dir: &std::path::Path) -> usize {
    fs::read_to_string(dir.join("calls"))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}
