//! Stops `pragma extract`'s model calls when pragma is told to stop (SIGTERM, SIGINT, SIGHUP)
//! or its parent goes away (the app quit or crashed), instead of leaving each `claude -p` to run
//! out its own timeout. The run then ends as usual: progress records the stop, the run lock is
//! released.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
#[cfg(unix)]
use std::{thread, time::Duration};

use pragma_engine::extract::model::Cancel;

/// Set by the signal handler; read by the watcher thread.
#[cfg(unix)]
static SIGNALLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
const POLL: Duration = Duration::from_millis(50);

/// Watches for a stop while it lives; dropping it ends the watch.
pub struct Watch {
    done: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.done.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The parent's pid, taken when the run starts: a parent that is gone before the watch starts
/// is still noticed, because pragma is reparented by then.
#[cfg(unix)]
pub fn parent_id() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}

/// Windows has no parent watch: a call there still ends at its own timeout.
#[cfg(not(unix))]
pub fn parent_id() -> Option<u32> {
    None
}

/// Cancels `cancel` when pragma gets a stop signal or loses `parent` (from `parent_id`, taken
/// at the start of the run). A run started with SIGHUP ignored (nohup, a detached run) is meant
/// to outlive its parent, so it has no parent watch.
#[cfg(unix)]
pub fn watch(cancel: Arc<Cancel>, parent: Option<u32>) -> Watch {
    let watch_parent = !hangups_ignored();
    install_handlers();
    let done = Arc::new(AtomicBool::new(false));
    let thread = {
        let done = done.clone();
        thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let orphaned = watch_parent && parent_id() != parent;
                if SIGNALLED.load(Ordering::SeqCst) || orphaned {
                    cancel.cancel();
                    return;
                }
                thread::sleep(POLL);
            }
        })
    };
    Watch {
        done,
        thread: Some(thread),
    }
}

/// Windows catches no stop signal and has no parent watch yet (R15), so there is nothing to
/// watch: a call there ends at its own timeout.
#[cfg(not(unix))]
pub fn watch(_cancel: Arc<Cancel>, _parent: Option<u32>) -> Watch {
    Watch {
        done: Arc::new(AtomicBool::new(false)),
        thread: None,
    }
}

/// Whether pragma was started with SIGHUP ignored.
#[cfg(unix)]
fn hangups_ignored() -> bool {
    // SAFETY: querying a signal's action writes only the zeroed struct passed in.
    unsafe {
        let mut current: libc::sigaction = std::mem::zeroed();
        libc::sigaction(libc::SIGHUP, std::ptr::null(), &mut current) == 0
            && current.sa_sigaction == libc::SIG_IGN
    }
}

/// Catches SIGTERM, SIGINT and SIGHUP: the first stops the run cleanly. SIGINT is caught once
/// (`SA_RESETHAND`), so a second Ctrl-C ends pragma at once. SIGTERM and SIGHUP stay caught: a
/// process-tree kill (the app's) signals pragma's group and pragma itself, and that second
/// SIGTERM must not end pragma before it has stopped its `claude` calls. A signal the caller
/// set to be ignored (nohup, a shell's background job) stays ignored.
#[cfg(unix)]
fn install_handlers() {
    extern "C" fn on_signal(_signal: libc::c_int) {
        SIGNALLED.store(true, Ordering::SeqCst);
    }
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: both structs are plain C data that may be zeroed; the handler only stores to
        // an atomic, which is async-signal-safe; and it is installed before any other thread
        // of this run starts.
        unsafe {
            let mut current: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(signal, std::ptr::null(), &mut current) != 0
                || current.sa_sigaction == libc::SIG_IGN
            {
                continue;
            }
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = if signal == libc::SIGINT {
                libc::SA_RESETHAND | libc::SA_RESTART
            } else {
                libc::SA_RESTART
            };
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Output};

    /// Set in the copy of this test binary a test runs its signals in.
    const IN_CHILD: &str = "PRAGMA_CANCEL_TEST_CHILD";

    fn in_child() -> bool {
        std::env::var_os(IN_CHILD).is_some()
    }

    /// Runs `test` alone in a fresh copy of this test binary, so its signals reach only that copy.
    fn run_in_child(test: &str) -> Output {
        Command::new(std::env::current_exe().unwrap())
            .args([test, "--exact", "--nocapture", "--test-threads=1"])
            .env(IN_CHILD, "1")
            .output()
            .unwrap()
    }

    fn raise(signal: libc::c_int) {
        // SAFETY: raising a signal in this process has no memory-safety preconditions.
        unsafe {
            libc::raise(signal);
        }
    }

    #[test]
    fn the_first_ctrl_c_stops_the_run_and_a_second_one_ends_pragma_at_once() {
        if in_child() {
            install_handlers();
            raise(libc::SIGINT);
            assert!(SIGNALLED.load(Ordering::SeqCst));
            println!("first signal caught");
            raise(libc::SIGINT);
            println!("second signal caught");
            return;
        }
        let output = run_in_child(
            "cancel::tests::the_first_ctrl_c_stops_the_run_and_a_second_one_ends_pragma_at_once",
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("first signal caught"), "{stdout}");
        assert!(!stdout.contains("second signal caught"), "{stdout}");
        assert_eq!(output.status.signal(), Some(libc::SIGINT), "{stdout}");
    }

    #[test]
    fn a_repeated_sigterm_leaves_the_run_to_stop_its_calls() {
        // The app's terminator signals pragma's process group and pragma itself: SIGTERM twice.
        if in_child() {
            install_handlers();
            raise(libc::SIGTERM);
            raise(libc::SIGTERM);
            raise(libc::SIGHUP);
            assert!(SIGNALLED.load(Ordering::SeqCst));
            println!("still running");
            return;
        }
        let output =
            run_in_child("cancel::tests::a_repeated_sigterm_leaves_the_run_to_stop_its_calls");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(stdout.contains("still running"), "{stdout}");
    }

    /// Watches with a parent pid that is not pragma's parent, as when the parent died, and says
    /// whether the run was cancelled.
    fn watch_a_lost_parent() -> bool {
        let cancel = Arc::new(Cancel::default());
        let _watch = watch(cancel.clone(), Some(u32::MAX));
        thread::sleep(Duration::from_millis(300));
        cancel.is_cancelled()
    }

    #[test]
    fn a_run_whose_parent_goes_is_stopped() {
        if in_child() {
            assert!(watch_a_lost_parent());
            println!("stopped");
            return;
        }
        let output = run_in_child("cancel::tests::a_run_whose_parent_goes_is_stopped");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(stdout.contains("stopped"), "{stdout}");
    }

    #[test]
    fn a_run_started_with_hangups_ignored_outlives_its_parent() {
        // nohup (and a detached run) leaves SIGHUP ignored: the run is meant to outlive its
        // parent, so losing the parent does not stop it.
        if in_child() {
            // SAFETY: setting a signal to be ignored has no memory-safety preconditions.
            unsafe {
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
            }
            assert!(!watch_a_lost_parent());
            println!("kept running");
            return;
        }
        let output =
            run_in_child("cancel::tests::a_run_started_with_hangups_ignored_outlives_its_parent");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(stdout.contains("kept running"), "{stdout}");
    }

    #[test]
    fn a_signal_the_caller_ignores_stays_ignored() {
        if in_child() {
            // As nohup and a shell's background jobs leave pragma.
            // SAFETY: setting a signal to be ignored has no memory-safety preconditions.
            unsafe {
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
            }
            install_handlers();
            raise(libc::SIGHUP);
            assert!(!SIGNALLED.load(Ordering::SeqCst));
            println!("still running");
            return;
        }
        let output = run_in_child("cancel::tests::a_signal_the_caller_ignores_stays_ignored");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(stdout.contains("still running"), "{stdout}");
    }
}
