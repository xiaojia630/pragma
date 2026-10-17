//! One extraction per repo at a time. A run holds an OS lock on
//! `<common dir>/pragma/extract.guard` for as long as it lives (`flock` on unix, `LockFileEx` on
//! Windows, through std's `File::try_lock`). Taking it is atomic, and the OS releases it when
//! the process ends, however it ends, so a crashed run never leaves the repo locked and no
//! pid or age has to be trusted. `extract.lock` beside it is the holder's record (pid and
//! start time) for status: written once the guard is held, removed by the run that wrote it.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::events::now_ms;
use crate::writes::write_atomic;

pub const EXTRACT_LOCK: &str = "extract.lock";
/// The file the OS lock is taken on. Never removed: deleting it would let a new run lock a new
/// file while an older run still holds the old one.
pub const EXTRACT_GUARD: &str = "extract.guard";
/// How long `acquire` keeps trying a held guard: `holder` (status, which the app polls) holds
/// it for an instant to see whether anyone else does.
const PROBE_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
}

/// Held while extraction runs; dropping it releases the lock.
#[derive(Debug)]
pub struct RunLock {
    path: PathBuf,
    me: Holder,
    /// Open for the whole run: closing it, after `drop` removes the record, releases the lock.
    _guard: File,
}

impl Drop for RunLock {
    fn drop(&mut self) {
        // Only this run's record: one another run wrote is left alone.
        if read_record(&self.path).as_ref() == Some(&self.me) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Takes the lock, or returns the live run that holds it.
pub fn acquire(shared_state_dir: &Path) -> io::Result<Result<RunLock, Holder>> {
    fs::create_dir_all(shared_state_dir)?;
    let guard = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(shared_state_dir.join(EXTRACT_GUARD))?;
    let path = shared_state_dir.join(EXTRACT_LOCK);
    let tried = Instant::now();
    loop {
        match guard.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if tried.elapsed() < PROBE_GRACE => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => {
                // The holder writes its record right after locking; pid 0 if it has not yet.
                let holder = read_record(&path).unwrap_or(Holder {
                    pid: 0,
                    started_at: now_ms(),
                });
                return Ok(Err(holder));
            }
            Err(TryLockError::Error(error)) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "could not lock {}: {error}; the file system may not support file locks",
                        shared_state_dir.join(EXTRACT_GUARD).display()
                    ),
                ));
            }
        }
    }
    let me = Holder {
        pid: std::process::id(),
        started_at: now_ms(),
    };
    // Replaces a record a crashed run left behind.
    write_atomic(&path, serde_json::to_string(&me)?.as_bytes())?;
    Ok(Ok(RunLock {
        path,
        me,
        _guard: guard,
    }))
}

/// The run holding the lock, when there is one: a record whose run still holds the guard.
pub fn holder(shared_state_dir: &Path) -> Option<Holder> {
    let record = read_record(&shared_state_dir.join(EXTRACT_LOCK))?;
    let guard = File::open(shared_state_dir.join(EXTRACT_GUARD)).ok()?;
    match guard.try_lock_shared() {
        // Nobody holds it (closing `guard` lets go again): the record is a crashed run's.
        Ok(()) => None,
        Err(TryLockError::WouldBlock) => Some(record),
        Err(TryLockError::Error(_)) => None,
    }
}

fn read_record(path: &Path) -> Option<Holder> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn one_run_at_a_time_and_released_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let lock = acquire(temp.path()).unwrap().unwrap();
        let held = acquire(temp.path()).unwrap().unwrap_err();
        assert_eq!(held.pid, std::process::id());
        assert_eq!(holder(temp.path()), Some(held));
        drop(lock);
        assert!(!temp.path().join(EXTRACT_LOCK).exists());
        assert_eq!(holder(temp.path()), None);
        assert!(acquire(temp.path()).unwrap().is_ok());
    }

    /// A dead process's pid, for a record a crashed run left behind.
    #[cfg(unix)]
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    #[cfg(unix)]
    #[test]
    fn one_run_wins_however_many_start_at_once() {
        for _ in 0..5 {
            let temp = tempfile::tempdir().unwrap();
            // A record a crashed run left: every starter may try to take it over at once.
            let stale = Holder {
                pid: dead_pid(),
                started_at: now_ms(),
            };
            fs::write(
                temp.path().join(EXTRACT_LOCK),
                serde_json::to_string(&stale).unwrap(),
            )
            .unwrap();
            let start = Barrier::new(8);
            let won: Vec<RunLock> = std::thread::scope(|scope| {
                let starters: Vec<_> = (0..8)
                    .map(|_| {
                        scope.spawn(|| {
                            start.wait();
                            acquire(temp.path()).ok().and_then(Result::ok)
                        })
                    })
                    .collect();
                starters
                    .into_iter()
                    .filter_map(|starter| starter.join().unwrap())
                    .collect()
            });
            assert_eq!(won.len(), 1, "{} runs took the lock", won.len());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_record_left_by_a_crashed_run_holds_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(EXTRACT_LOCK);
        let crashed = Holder {
            pid: dead_pid(),
            started_at: now_ms(),
        };
        fs::write(&path, serde_json::to_string(&crashed).unwrap()).unwrap();
        assert_eq!(holder(temp.path()), None);
        let lock = acquire(temp.path()).unwrap().unwrap();
        assert_eq!(holder(temp.path()).unwrap().pid, std::process::id());
        drop(lock);
        fs::write(&path, "garbage").unwrap();
        assert!(acquire(temp.path()).unwrap().is_ok());
    }

    #[test]
    fn a_long_run_that_still_holds_the_lock_keeps_it() {
        let temp = tempfile::tempdir().unwrap();
        let _lock = acquire(temp.path()).unwrap().unwrap();
        // Three hours in: pids are not trusted, the lock the run holds is.
        let long = Holder {
            pid: std::process::id(),
            started_at: now_ms() - 3 * 60 * 60 * 1000,
        };
        fs::write(
            temp.path().join(EXTRACT_LOCK),
            serde_json::to_string(&long).unwrap(),
        )
        .unwrap();
        assert_eq!(holder(temp.path()), Some(long.clone()));
        assert_eq!(acquire(temp.path()).unwrap().unwrap_err(), long);
    }

    #[test]
    fn a_run_removes_only_its_own_record() {
        let temp = tempfile::tempdir().unwrap();
        let lock = acquire(temp.path()).unwrap().unwrap();
        // Another run's record in its place (an older pragma that took the lock over).
        let other = Holder {
            pid: std::process::id() + 1,
            started_at: 1,
        };
        let text = serde_json::to_string(&other).unwrap();
        fs::write(temp.path().join(EXTRACT_LOCK), &text).unwrap();
        drop(lock);
        assert_eq!(
            fs::read_to_string(temp.path().join(EXTRACT_LOCK)).unwrap(),
            text
        );
    }
}
