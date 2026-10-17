//! The model boundary. Extraction (and later judging) asks the user's own `claude` CLI, never
//! pragma's API keys, through the `Model` trait so tests can stand in a fake.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::env::Env;

/// Test and development only: the `claude` binary to run in place of the one on `PATH`.
pub const CLAUDE_BIN_ENV: &str = "PRAGMA_CLAUDE_BIN";
/// How long one model call may take before it is killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub trait Model: Sync {
    /// The model alias, part of the cache key.
    fn name(&self) -> &str;
    /// Sends `prompt` and returns the model's answer, which `schema` constrains.
    fn complete(&self, prompt: &str, schema: &Value) -> Result<Value, ModelError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelErrorKind {
    /// The model CLI is not installed or not on `PATH`.
    Missing,
    /// The model CLI is installed but not logged in.
    LoggedOut,
    /// A call took longer than its timeout.
    Timeout,
    /// The CLI ran but its answer could not be read.
    BadOutput,
    /// The run was stopped (pragma got a signal or its parent went away) before the call ended.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelError {
    pub kind: ModelErrorKind,
    pub message: String,
}

impl ModelError {
    fn new(kind: ModelErrorKind, message: impl Into<String>) -> ModelError {
        ModelError {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelError {}

/// Stops a run's model calls: once cancelled, a call in flight kills its CLI (with everything the
/// CLI started) and returns `Cancelled`, and a new call starts nothing.
#[derive(Debug, Default)]
pub struct Cancel {
    cancelled: AtomicBool,
}

impl Cancel {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// `claude -p`, isolated: no tools, hooks, settings, MCP servers or session history (S0c).
pub struct ClaudeCli {
    pub binary: PathBuf,
    pub model: String,
    pub timeout: Duration,
    /// Set by the command that owns the run, so a signal stops the calls in flight.
    pub cancel: Option<Arc<Cancel>>,
}

impl ClaudeCli {
    /// The CLI from `PRAGMA_CLAUDE_BIN`, else `claude` on `PATH`.
    pub fn from_env(
        env: &impl Env,
        model: &str,
        timeout: Duration,
    ) -> Result<ClaudeCli, ModelError> {
        let binary = resolve_binary(env).ok_or_else(|| {
            ModelError::new(
                ModelErrorKind::Missing,
                "the claude CLI was not found on PATH; install Claude Code",
            )
        })?;
        Ok(ClaudeCli {
            binary,
            model: model.to_string(),
            timeout,
            cancel: None,
        })
    }

    /// Calls stop when `cancel` is cancelled.
    pub fn with_cancel(self, cancel: Arc<Cancel>) -> ClaudeCli {
        ClaudeCli {
            cancel: Some(cancel),
            ..self
        }
    }

    /// The arguments of one call, in order.
    pub fn args(&self, schema: &Value) -> Vec<String> {
        [
            "-p",
            "--output-format",
            "json",
            "--tools",
            "",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--setting-sources",
            "",
            "--settings",
            r#"{"disableAllHooks":true}"#,
            "--model",
            &self.model,
            "--json-schema",
            &schema.to_string(),
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
}

impl Model for ClaudeCli {
    fn name(&self) -> &str {
        &self.model
    }

    fn complete(&self, prompt: &str, schema: &Value) -> Result<Value, ModelError> {
        let mut command = Command::new(&self.binary);
        command.args(self.args(schema)).env("PRAGMA_JUDGE", "1");
        let output = run_with_timeout(
            command,
            prompt.as_bytes(),
            self.timeout,
            self.cancel.as_deref(),
        )?;
        parse_output(&output.stdout, &output.stderr)
    }
}

fn resolve_binary(env: &impl Env) -> Option<PathBuf> {
    if let Some(binary) = env.var(CLAUDE_BIN_ENV) {
        return Some(PathBuf::from(binary));
    }
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    let path = env.var("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

struct Output {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn cancelled() -> ModelError {
    ModelError::new(
        ModelErrorKind::Cancelled,
        "reading the docs was stopped before it finished",
    )
}

/// Serializes `Command::spawn` across the model calls a run makes at once. Apple's std (like
/// every target without `pipe2`) creates a child's stdio pipes with `pipe()` and then sets
/// close-on-exec on each end, which is not atomic: a sibling call spawning in between inherits
/// this call's pipe ends. A CLI holding another call's stdin write end keeps that call from ever
/// seeing the end of its prompt, so it hangs until its timeout. Holding this lock from pipe
/// creation through the spawn means every pipe end is close-on-exec before another call forks.
static SPAWN: Mutex<()> = Mutex::new(());

/// How often a call checks on its CLI, its cancel token and its deadline.
const POLL: Duration = Duration::from_millis(20);

enum Stream {
    Stdout,
    Stderr,
}

/// Runs `command` with `input` on stdin, killing it at `timeout` or when `cancel` is cancelled.
/// On unix the CLI gets its own process group, so killing it also kills what it started. The
/// deadline covers the whole call, reading the CLI's output after it exits too: a child it left
/// behind holding the pipes open cannot keep the call waiting past its timeout.
fn run_with_timeout(
    mut command: Command,
    input: &[u8],
    timeout: Duration,
    cancel: Option<&Cancel>,
) -> Result<Output, ModelError> {
    if cancel.is_some_and(Cancel::is_cancelled) {
        return Err(cancelled());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let deadline = Instant::now() + timeout;
    let spawned = {
        let _spawning = SPAWN.lock().unwrap_or_else(PoisonError::into_inner);
        command.spawn()
    };
    let mut child = spawned.map_err(|error| missing(command.get_program().as_ref(), &error))?;
    if let Some(mut stdin) = child.stdin.take() {
        let input = input.to_vec();
        // Not joined: a CLI that exits early closes its stdin and the write fails, and a child
        // it left holding stdin is killed with its group below.
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let (sender, receiver) = mpsc::channel();
    let read = |pipe: Option<Box<dyn Read + Send>>, stream: Stream| {
        let sender = sender.clone();
        thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = sender.send((stream, bytes));
        });
    };
    read(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        Stream::Stdout,
    );
    read(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        Stream::Stderr,
    );
    let (mut exited, mut stdout, mut stderr) = (false, None, None);
    loop {
        while let Ok((stream, bytes)) = receiver.try_recv() {
            match stream {
                Stream::Stdout => stdout = Some(bytes),
                Stream::Stderr => stderr = Some(bytes),
            }
        }
        if !exited {
            match child.try_wait() {
                Ok(status) => exited = status.is_some(),
                Err(error) => {
                    kill_tree(&mut child);
                    return Err(ModelError::new(
                        ModelErrorKind::BadOutput,
                        format!("waiting for the claude CLI: {error}"),
                    ));
                }
            }
        }
        if exited && stdout.is_some() && stderr.is_some() {
            break;
        }
        if cancel.is_some_and(Cancel::is_cancelled) {
            kill_tree(&mut child);
            return Err(cancelled());
        }
        if Instant::now() >= deadline {
            // Readers still waiting are left behind; killing the group closes their pipes.
            kill_tree(&mut child);
            return Err(ModelError::new(
                ModelErrorKind::Timeout,
                format!("the model call took longer than {} s", timeout.as_secs()),
            ));
        }
        thread::sleep(POLL);
    }
    Ok(Output {
        stdout: stdout.unwrap_or_default(),
        stderr: stderr.unwrap_or_default(),
    })
}

/// Kills the CLI and, on unix, its whole process group (what it started, even after the CLI
/// itself exited), then reaps it.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `kill` has no memory-safety preconditions. The group id is the CLI's pid
        // (`process_group(0)`), and a group's id is not reused while any member is alive.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn missing(binary: &Path, error: &std::io::Error) -> ModelError {
    ModelError::new(
        ModelErrorKind::Missing,
        format!("could not run {}: {error}", binary.display()),
    )
}

/// The marker `claude -p` prints (in `result`, exit 1) when no account is logged in.
fn is_logged_out(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("not logged in") || text.contains("please run /login")
}

fn logged_out() -> ModelError {
    ModelError::new(
        ModelErrorKind::LoggedOut,
        "the claude CLI is not logged in; run claude and /login",
    )
}

/// Reads `claude -p --output-format json`: the validated answer is in `structured_output`.
/// Logged out is read only from an error envelope's `result` or from output that is no envelope
/// at all: an answer can quote the login message (a doc may say it), and stderr can warn while
/// the CLI answers or fails for another reason.
fn parse_output(stdout: &[u8], stderr: &[u8]) -> Result<Value, ModelError> {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let bad = |detail: String| ModelError::new(ModelErrorKind::BadOutput, detail);
    let Ok(envelope) = serde_json::from_str::<Value>(stdout.trim()) else {
        if is_logged_out(&stdout) || is_logged_out(&stderr) {
            return Err(logged_out());
        }
        return Err(bad(format!(
            "the claude CLI did not answer in JSON: {}",
            snippet(if stdout.trim().is_empty() {
                &stderr
            } else {
                &stdout
            })
        )));
    };
    if envelope["is_error"] == Value::Bool(true) {
        // The envelope says why; stderr may carry unrelated warnings.
        let result = envelope["result"].as_str().unwrap_or("no detail");
        if is_logged_out(result) {
            return Err(logged_out());
        }
        return Err(bad(format!(
            "the claude CLI reported an error: {}",
            snippet(result)
        )));
    }
    match envelope.get("structured_output") {
        Some(answer) if answer.is_object() => Ok(answer.clone()),
        _ => Err(bad("the claude CLI answer has no structured_output".into())),
    }
}

fn snippet(text: &str) -> String {
    text.trim().chars().take(200).collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    /// A fake `claude` that records its arguments and stdin, then runs `body`.
    fn fake(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("claude");
        let script = format!(
            "#!/bin/sh\nfor arg in \"$@\"; do printf '%s\\n' \"$arg\"; done > \"{dir}/args\"\n\
             cat > \"{dir}/stdin\"\nprintf '%s' \"$PRAGMA_JUDGE\" > \"{dir}/judge\"\n{body}\n",
            dir = dir.display()
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn cli(binary: PathBuf, timeout: Duration) -> ClaudeCli {
        ClaudeCli {
            binary,
            model: "haiku".into(),
            timeout,
            cancel: None,
        }
    }

    fn alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Waits up to 2 s for `pid` to be gone (a killed process may take a moment to be reaped).
    fn gone(pid: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_cancelled_call_kills_the_cli_and_what_it_started() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().display().to_string();
        // The CLI starts a child of its own (claude runs node), then hangs.
        let body =
            format!("sleep 30 &\necho $! > \"{dir}/grandchild\"\necho $$ > \"{dir}/pid\"\nwait");
        let cancel = Arc::new(Cancel::default());
        let model = ClaudeCli {
            cancel: Some(cancel.clone()),
            ..cli(fake(temp.path(), &body), DEFAULT_TIMEOUT)
        };
        let pid_file = temp.path().join("grandchild");
        let canceller = thread::spawn(move || {
            while !pid_file.exists() {
                thread::sleep(Duration::from_millis(10));
            }
            thread::sleep(Duration::from_millis(50));
            cancel.cancel();
        });
        let started = Instant::now();
        let error = model.complete("x", &schema()).unwrap_err();
        canceller.join().unwrap();
        assert_eq!(error.kind, ModelErrorKind::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid = fs::read_to_string(temp.path().join("pid")).unwrap();
        let grandchild = fs::read_to_string(temp.path().join("grandchild")).unwrap();
        assert!(gone(&pid), "the claude process {pid} is still running");
        assert!(gone(&grandchild), "its child {grandchild} is still running");
    }

    /// Extraction runs 4 calls at once. A call whose pipe leaked into a sibling CLI waits for
    /// that sibling to go before it sees the end of its stdin or stdout.
    #[test]
    fn parallel_calls_do_not_hold_each_others_pipes() {
        let temp = tempfile::tempdir().unwrap();
        // After answering, each CLI leaves a child behind for 3 s with its own stdio closed: a
        // sibling's pipe end it inherited stays open that long.
        let binary = fake(temp.path(), &format!("{OK}\n(sleep 3 <&- >&- 2>&- &)"));
        thread::scope(|scope| {
            for _ in 0..8 {
                let binary = binary.clone();
                scope.spawn(move || {
                    for _ in 0..25 {
                        let answer =
                            cli(binary.clone(), Duration::from_secs(2)).complete("x", &schema());
                        assert_eq!(answer, Ok(serde_json::json!({ "ok": true })));
                    }
                });
            }
        });
    }

    #[test]
    fn a_call_cancelled_before_it_starts_runs_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let cancel = Arc::new(Cancel::default());
        cancel.cancel();
        let model = ClaudeCli {
            cancel: Some(cancel),
            ..cli(fake(temp.path(), OK), DEFAULT_TIMEOUT)
        };
        let error = model.complete("x", &schema()).unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Cancelled);
        assert!(!temp.path().join("args").exists());
    }

    fn schema() -> Value {
        serde_json::json!({ "type": "object" })
    }

    const OK: &str = r#"echo '{"type":"result","is_error":false,"result":"{\"ok\":true}","structured_output":{"ok":true}}'"#;

    #[test]
    fn sends_the_isolating_flags_and_the_prompt_on_stdin() {
        let temp = tempfile::tempdir().unwrap();
        let model = cli(fake(temp.path(), OK), DEFAULT_TIMEOUT);
        let answer = model.complete("the prompt\n", &schema()).unwrap();
        assert_eq!(answer, serde_json::json!({ "ok": true }));
        let args = fs::read_to_string(temp.path().join("args")).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec![
                "-p",
                "--output-format",
                "json",
                "--tools",
                "",
                "--no-session-persistence",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--setting-sources",
                "",
                "--settings",
                r#"{"disableAllHooks":true}"#,
                "--model",
                "haiku",
                "--json-schema",
                r#"{"type":"object"}"#,
            ]
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("stdin")).unwrap(),
            "the prompt\n"
        );
        assert_eq!(fs::read_to_string(temp.path().join("judge")).unwrap(), "1");
    }

    #[test]
    fn a_logged_out_cli_is_a_typed_error() {
        // What claude 2.1.301 prints with an empty CLAUDE_CONFIG_DIR, exit 1.
        let temp = tempfile::tempdir().unwrap();
        let body = r#"echo '{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}'; exit 1"#;
        let error = cli(fake(temp.path(), body), DEFAULT_TIMEOUT)
            .complete("x", &schema())
            .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::LoggedOut);
    }

    #[test]
    fn an_answer_that_quotes_the_login_message_is_an_answer() {
        // Only an error envelope or output that is not an envelope can mean logged out: a doc
        // may well say "Not logged in", and a CLI may warn on stderr while it answers.
        let temp = tempfile::tempdir().unwrap();
        let body = r#"echo 'warning: not logged in to the plugin registry' >&2
echo '{"type":"result","is_error":false,"result":"","structured_output":{"text":"Show Not logged in · Please run /login when the token expires."}}'"#;
        let answer = cli(fake(temp.path(), body), DEFAULT_TIMEOUT)
            .complete("x", &schema())
            .unwrap();
        assert_eq!(
            answer["text"],
            "Show Not logged in · Please run /login when the token expires."
        );
    }

    #[test]
    fn an_error_envelope_is_read_for_itself_not_for_a_stderr_warning() {
        let temp = tempfile::tempdir().unwrap();
        let body = r#"echo 'warning: not logged in to the plugin registry' >&2
echo '{"type":"result","is_error":true,"result":"overloaded"}'; exit 1"#;
        let error = cli(fake(temp.path(), body), DEFAULT_TIMEOUT)
            .complete("x", &schema())
            .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::BadOutput, "{error}");
    }

    #[test]
    fn a_logged_out_cli_without_an_envelope_is_logged_out() {
        let temp = tempfile::tempdir().unwrap();
        let body = "echo 'Not logged in · Please run /login' >&2; exit 1";
        let error = cli(fake(temp.path(), body), DEFAULT_TIMEOUT)
            .complete("x", &schema())
            .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::LoggedOut);
    }

    #[test]
    fn a_child_that_keeps_the_output_open_cannot_outlast_the_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().display().to_string();
        // The CLI answers and exits, but a child it started holds its stdout open.
        let body = format!("sleep 30 &\necho $! > \"{dir}/grandchild\"\n{OK}");
        let started = Instant::now();
        let error = cli(fake(temp.path(), &body), Duration::from_secs(2))
            .complete("x", &schema())
            .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Timeout);
        assert!(started.elapsed() < Duration::from_secs(6));
        let grandchild = fs::read_to_string(temp.path().join("grandchild")).unwrap();
        assert!(gone(&grandchild), "its child {grandchild} is still running");
    }

    #[test]
    fn a_slow_call_times_out() {
        let temp = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let error = cli(
            fake(temp.path(), "exec sleep 5"),
            Duration::from_millis(300),
        )
        .complete("x", &schema())
        .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Timeout);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn unreadable_answers_are_bad_output() {
        let temp = tempfile::tempdir().unwrap();
        for body in [
            "echo 'not json'",
            r#"echo '{"is_error":true,"result":"overloaded"}'; exit 1"#,
            r#"echo '{"is_error":false,"result":"{}"}'"#,
        ] {
            let error = cli(fake(temp.path(), body), DEFAULT_TIMEOUT)
                .complete("x", &schema())
                .unwrap_err();
            assert_eq!(error.kind, ModelErrorKind::BadOutput, "{body}: {error}");
        }
    }

    #[test]
    fn finds_the_binary_from_the_override_or_path() {
        let temp = tempfile::tempdir().unwrap();
        let binary = fake(temp.path(), OK);
        let env = |pairs: &[(&str, String)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect()
        };
        let on_path = env(&[("PATH", format!("/nonexistent:{}", temp.path().display()))]);
        assert_eq!(
            ClaudeCli::from_env(&on_path, "haiku", DEFAULT_TIMEOUT)
                .unwrap()
                .binary,
            binary
        );
        let overridden = env(&[
            ("PATH", temp.path().display().to_string()),
            (CLAUDE_BIN_ENV, "/opt/fake-claude".into()),
        ]);
        assert_eq!(
            ClaudeCli::from_env(&overridden, "haiku", DEFAULT_TIMEOUT)
                .unwrap()
                .binary,
            PathBuf::from("/opt/fake-claude")
        );
        let nowhere = env(&[("PATH", "/nonexistent".into())]);
        let error = ClaudeCli::from_env(&nowhere, "haiku", DEFAULT_TIMEOUT)
            .err()
            .unwrap();
        assert_eq!(error.kind, ModelErrorKind::Missing);
    }

    #[test]
    fn a_binary_that_cannot_run_is_missing() {
        let error = cli(PathBuf::from("/nonexistent/claude"), DEFAULT_TIMEOUT)
            .complete("x", &schema())
            .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Missing);
    }
}
