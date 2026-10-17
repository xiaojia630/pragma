#![cfg(unix)]

//! Stopping `pragma extract` stops the `claude -p` calls it started, releases the run lock and
//! records the run as stopped.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use common::{TestRepo, fake_claude};
use serde_json::Value;

/// A fake `claude` that never answers: it records its pid and its child's pid, then waits.
fn hanging_claude(dir: &Path) -> PathBuf {
    fake_claude(
        dir,
        &format!(
            "cat > /dev/null\nsleep 60 &\necho $! >> \"{dir}/pids\"\necho $$ >> \"{dir}/pids\"\nwait",
            dir = dir.display()
        ),
    )
}

struct Setup {
    repo: TestRepo,
    home: PathBuf,
    fake_dir: PathBuf,
}

/// 25 paragraphs: three calls in flight at once.
fn setup() -> Setup {
    let repo = TestRepo::new();
    let text: String = (0..25)
        .map(|n| format!("Rule number {n} says something.\n\n"))
        .collect();
    repo.write("AGENTS.md", &format!("# Rules\n\n{text}"));
    repo.commit();
    let home = repo.temp_dir().join("home");
    let fake_dir = repo.temp_dir().join("fake");
    Setup {
        repo,
        home,
        fake_dir,
    }
}

/// Running, and not a zombie waiting for init to reap it.
fn alive(pid: &str) -> bool {
    Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .is_ok_and(|output| {
            let stat = String::from_utf8_lossy(&output.stdout);
            output.status.success() && !stat.trim().is_empty() && !stat.trim().starts_with('Z')
        })
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        if Instant::now() >= deadline {
            let ps = Command::new("ps")
                .args(["-axo", "pid,ppid,stat,command"])
                .output();
            let ps = ps.map(|output| String::from_utf8_lossy(&output.stdout).into_owned());
            let related: Vec<String> = ps
                .unwrap_or_default()
                .lines()
                .filter(|line| line.contains("sleep 60") || line.contains("pragma"))
                .map(String::from)
                .collect();
            panic!(
                "timed out waiting until {what}; related processes:\n{}",
                related.join("\n")
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// The pids the fake CLIs recorded, once three calls (two pids each) are in flight.
fn calls_in_flight(fake_dir: &Path) -> Vec<String> {
    let pids = fake_dir.join("pids");
    wait_until("three calls are in flight", || {
        std::fs::read_to_string(&pids).is_ok_and(|text| text.lines().count() >= 6)
    });
    let pids: Vec<String> = std::fs::read_to_string(&pids)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert!(pids.iter().all(|pid| alive(pid)));
    pids
}

fn assert_stopped(setup: &Setup, pids: &[String]) {
    for pid in pids {
        wait_until(&format!("pid {pid} is gone"), || !alive(pid));
    }
    assert!(!setup.repo.root.join(".git/pragma/extract.lock").exists());
    let progress: Value = serde_json::from_str(
        &std::fs::read_to_string(setup.repo.root.join(".git/pragma/extract-progress.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(progress["error"]["kind"], "cancelled", "{progress}");
    assert!(progress["finished_at"].as_u64().is_some());
    assert!(!setup.repo.root.join(".git/pragma/index.json").exists());
}

fn spawn_extract(setup: &Setup, claude: &Path) -> Child {
    common::pragma()
        .args(["extract", "--json"])
        .env("PRAGMA_HOME", &setup.home)
        .env("PRAGMA_CLAUDE_BIN", claude)
        .current_dir(&setup.repo.root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[test]
fn sigterm_stops_the_model_calls_and_releases_the_run() {
    for signal in ["-TERM", "-INT"] {
        let setup = setup();
        let claude = hanging_claude(&setup.fake_dir);
        let child = spawn_extract(&setup, &claude);
        let pids = calls_in_flight(&setup.fake_dir);

        let started = Instant::now();
        Command::new("kill")
            .args([signal, &child.id().to_string()])
            .status()
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(output.status.code(), Some(1), "{signal}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["kind"], "cancelled");
        assert_stopped(&setup, &pids);

        // The status says the run stopped, and the next run starts cleanly.
        let status = common::pragma()
            .args(["status", "--json"])
            .env("PRAGMA_HOME", &setup.home)
            .current_dir(&setup.repo.root)
            .output()
            .unwrap();
        let status: Value = serde_json::from_slice(&status.stdout).unwrap();
        assert_eq!(status["extraction"]["state"], "paused");
        assert_eq!(status["extraction"]["reason"]["kind"], "cancelled");
    }
}

#[test]
fn a_run_whose_parent_dies_stops_its_model_calls() {
    let setup = setup();
    let claude = hanging_claude(&setup.fake_dir);
    // A parent shell that starts pragma and is then killed outright, like an app that crashed.
    let parent = Command::new("sh")
        .args([
            "-c",
            "\"$0\" extract --json > /dev/null 2>&1 & echo $! > \"$1\"; wait",
            env!("CARGO_BIN_EXE_pragma"),
            &setup.fake_dir.join("pragma-pid").display().to_string(),
        ])
        .env_remove("PRAGMA_JUDGE")
        .env_remove("CLAUDE_CONFIG_DIR")
        .env("PRAGMA_HOME", &setup.home)
        .env("PRAGMA_CLAUDE_BIN", &claude)
        .current_dir(&setup.repo.root)
        .spawn();
    let mut parent = parent.unwrap();
    let pids = calls_in_flight(&setup.fake_dir);
    let pragma_pid = std::fs::read_to_string(setup.fake_dir.join("pragma-pid")).unwrap();
    let pragma_pid = pragma_pid.trim().to_string();

    parent.kill().unwrap();
    parent.wait().unwrap();
    wait_until("pragma exits", || !alive(&pragma_pid));
    assert_stopped(&setup, &pids);
}
