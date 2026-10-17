//! `pragma`: makes coding agents follow the engineering specs in a repo's docs.

mod audit_cmd;
mod backtest_cmd;
mod cancel;
mod commit_cmd;
mod config_cmd;
mod debug_cmd;
mod docs_cmd;
mod drift_cmd;
mod events_cmd;
mod exception_cmd;
mod extract_cmd;
mod hook_cmd;
mod hooks_cmd;
mod init_cmd;
mod json;
mod lock_cmd;
mod output;
mod status_cmd;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pragma_engine::backtest::DEFAULT_DEPTH;
use pragma_engine::env::ProcessEnv;
use pragma_engine::gate::set_paused;
use pragma_engine::repo::Repo;

use crate::exception_cmd::ReasonArg;
use crate::hook_cmd::HookEvent;
use crate::json::PausedOutput;

#[derive(Parser)]
#[command(
    name = "pragma",
    version,
    about = "Makes coding agents follow your engineering specs."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the binary version and the lock format it reads.
    Version {
        #[arg(long)]
        json: bool,
    },
    /// Claude Code hook entry points. Not for direct use.
    Hook {
        #[command(subcommand)]
        event: HookEvent,
    },
    /// Install or remove pragma's hooks in Claude Code's settings.
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
    /// Show what pragma is doing in this repo.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Create .pragma/ in this repo's root checkout.
    Init {
        #[arg(long)]
        json: bool,
    },
    /// Stop pragma for this repo on this machine.
    Pause {
        #[arg(long)]
        json: bool,
    },
    /// Start pragma again for this repo on this machine.
    Resume {
        #[arg(long)]
        json: bool,
    },
    /// Update the lock from this checkout's clause index (root checkout only), or work with it.
    #[command(args_conflicts_with_subcommands = true)]
    Lock {
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<LockAction>,
    },
    /// Check the docs against the repo: files, scripts and code names that no longer exist.
    #[command(args_conflicts_with_subcommands = true)]
    Drift {
        #[arg(long)]
        json: bool,
        /// Print the suggested fixes as a diff for `git apply`.
        #[arg(long)]
        diff: bool,
        #[command(subcommand)]
        action: Option<DriftAction>,
    },
    /// Read the spec docs into clauses with your Claude CLI and write this checkout's index.
    Extract {
        /// Count the docs, paragraphs and model calls a run needs, without calling the model.
        #[arg(long)]
        estimate: bool,
        #[arg(long)]
        json: bool,
    },
    /// Check the whole tree against the lock's rules.
    Audit {
        /// Only this file or directory, repo-relative.
        #[arg(long)]
        area: Option<String>,
        /// Only the files changed since this ref's merge base with HEAD, committed or not.
        #[arg(long, value_name = "REF", conflicts_with = "area")]
        changed_from: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Count the violations recent commits introduced that today's lock would have stopped.
    Backtest {
        /// How many first-parent commits of HEAD to check; at least 1.
        #[arg(
            long,
            value_name = "N",
            default_value_t = DEFAULT_DEPTH,
            value_parser = commit_count
        )]
        since: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show what the edit check decided in this checkout: blocks, escalations, unchecked edits
    /// and cleared blocks, oldest first.
    Events {
        /// Show at most this many, the most recent.
        #[arg(long, value_name = "N", default_value_t = events_cmd::DEFAULT_LIMIT)]
        limit: usize,
        /// Only events after this time, in milliseconds since the Unix epoch.
        #[arg(long, value_name = "MS")]
        since_ms: Option<u64>,
        #[arg(long)]
        json: bool,
    },
    /// List the spec docs pragma reads. Globs given here replace the saved ones (a preview).
    Docs {
        /// Also read docs matching this glob; repeatable.
        #[arg(long = "include", value_name = "GLOB")]
        include: Vec<String>,
        /// Leave out docs matching this glob; repeatable.
        #[arg(long = "exclude", value_name = "GLOB")]
        exclude: Vec<String>,
        /// Use only the globs given here, even none, in place of the saved ones.
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show or change the repo settings in .pragma/config.md.
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Commit .pragma/ and nothing else. Root checkout only.
    Commit {
        /// The commit message.
        #[arg(long, short = 'm')]
        message: String,
        #[arg(long)]
        json: bool,
    },
    /// Record findings that are not violations.
    Exception {
        #[command(subcommand)]
        action: ExceptionAction,
    },
    /// Debugging helpers.
    Debug {
        #[command(subcommand)]
        action: DebugAction,
    },
}

#[derive(Subcommand)]
enum HooksAction {
    /// Show whether pragma's entries are in Claude Code's settings.
    Status {
        #[arg(long)]
        settings: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Add pragma's entries to Claude Code's settings, beside the user's own hooks.
    Install {
        /// Claude Code settings file; defaults to $CLAUDE_CONFIG_DIR/settings.json or
        /// ~/.claude/settings.json.
        #[arg(long)]
        settings: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Remove pragma's entries from Claude Code's settings.
    Uninstall {
        #[arg(long)]
        settings: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the settings.
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Change settings; only the keys given are rewritten. Root checkout only.
    Set {
        /// Replace docs.include with these globs; repeatable.
        #[arg(
            long = "docs-include",
            value_name = "GLOB",
            conflicts_with = "clear_include"
        )]
        docs_include: Vec<String>,
        /// Replace docs.exclude with these globs; repeatable.
        #[arg(
            long = "docs-exclude",
            value_name = "GLOB",
            conflicts_with = "clear_exclude"
        )]
        docs_exclude: Vec<String>,
        /// Empty docs.include.
        #[arg(long)]
        clear_include: bool,
        /// Empty docs.exclude.
        #[arg(long)]
        clear_exclude: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DriftAction {
    /// Write the suggested fix of each finding into its doc, after checking again.
    Apply {
        /// Finding ids from `pragma drift`.
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum LockAction {
    /// Validate the lock: scopes compile and every pattern behaves on its examples.
    Check {
        #[arg(long)]
        json: bool,
    },
    /// List the clauses: what each asks, its fix and source, with the recorded exceptions.
    Show {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ExceptionAction {
    /// Mark a finding from the last `pragma audit` or `pragma drift` as not a violation.
    Add {
        id: String,
        #[arg(long, value_enum)]
        reason: ReasonArg,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DebugAction {
    /// Print the clause id and anchor for the paragraph at <doc>:<line>.
    Anchor {
        location: String,
        #[arg(long, default_value_t = 0)]
        ordinal: u32,
        /// Accepted for consistency; the output is always JSON.
        #[arg(long)]
        json: bool,
    },
}

/// A whole number of commits, at least 1: `--since 0` would check nothing and still overwrite the
/// saved report with an empty one.
fn commit_count(text: &str) -> Result<usize, String> {
    match text.parse::<usize>() {
        Ok(0) => Err("must be at least 1".to_string()),
        Ok(count) => Ok(count),
        Err(error) => Err(error.to_string()),
    }
}

fn main() -> ExitCode {
    // clap exits 2 on a bad argument, which Claude Code reads as a block. A hook never blocks
    // by accident, so a hook command line we do not understand is let through.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if error.use_stderr() && std::env::args().nth(1).as_deref() == Some("hook") {
                return ExitCode::SUCCESS;
            }
            // Outside hooks a command-line error exits 1 (clap's own 2 is reserved for blocks).
            let _ = error.print();
            return if error.use_stderr() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            };
        }
    };
    match cli.command {
        Command::Hook { event } => hook_cmd::run(event),
        command => match run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("pragma: {error:#}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run(command: Command) -> Result<()> {
    // Read lazily: `version` and `hooks` do not need a working directory.
    let cwd = || std::env::current_dir().context("reading the current directory");
    match command {
        Command::Hook { .. } => unreachable!("hooks are dispatched in main"),
        Command::Version { json } => output::version(json),
        Command::Hooks {
            action: HooksAction::Status { settings, json },
        } => hooks_cmd::status(settings, json, &ProcessEnv),
        Command::Hooks {
            action: HooksAction::Install { settings, json },
        } => hooks_cmd::install(settings, json, &ProcessEnv),
        Command::Hooks {
            action: HooksAction::Uninstall { settings, json },
        } => hooks_cmd::uninstall(settings, json, &ProcessEnv),
        Command::Status { json } => status_cmd::run(&cwd()?, json),
        Command::Init { json } => init_cmd::run(&cwd()?, json),
        Command::Pause { json } => {
            set_paused(&repo_here(&cwd()?)?, true)?;
            if json {
                return output::print_json(&PausedOutput { paused: true });
            }
            println!("pragma is paused for this repo on this machine.");
            Ok(())
        }
        Command::Resume { json } => {
            set_paused(&repo_here(&cwd()?)?, false)?;
            if json {
                return output::print_json(&PausedOutput { paused: false });
            }
            println!("pragma is on again for this repo.");
            Ok(())
        }
        Command::Lock {
            action: Some(LockAction::Check { json }),
            ..
        } => lock_cmd::check(&cwd()?, json),
        Command::Lock {
            action: Some(LockAction::Show { json }),
            ..
        } => lock_cmd::show(&cwd()?, json),
        Command::Lock { json, action: None } => lock_cmd::update(&cwd()?, json),
        Command::Drift {
            action: Some(DriftAction::Apply { ids, json }),
            ..
        } => drift_cmd::apply(&cwd()?, ids, json),
        Command::Drift { json, diff, .. } => drift_cmd::run(&cwd()?, json, diff),
        Command::Extract { estimate, json } => extract_cmd::run(&cwd()?, json, estimate),
        Command::Audit {
            area,
            changed_from,
            json,
        } => audit_cmd::run(&cwd()?, area.as_deref(), changed_from.as_deref(), json),
        Command::Backtest { since, json } => backtest_cmd::run(&cwd()?, since, json),
        Command::Events {
            limit,
            since_ms,
            json,
        } => events_cmd::run(&cwd()?, limit, since_ms, json),
        Command::Docs {
            include,
            exclude,
            preview,
            json,
        } => docs_cmd::run(&cwd()?, include, exclude, preview, json),
        Command::Commit { message, json } => commit_cmd::run(&cwd()?, &message, json),
        Command::Config {
            action: ConfigAction::Show { json },
        } => config_cmd::show(&cwd()?, json),
        Command::Config {
            action:
                ConfigAction::Set {
                    docs_include,
                    docs_exclude,
                    clear_include,
                    clear_exclude,
                    json,
                },
        } => config_cmd::set(
            &cwd()?,
            docs_include,
            docs_exclude,
            clear_include,
            clear_exclude,
            json,
        ),
        Command::Exception {
            action:
                ExceptionAction::Add {
                    id,
                    reason,
                    note,
                    json,
                },
        } => exception_cmd::add(&cwd()?, &id, reason, note, json),
        Command::Debug {
            action: DebugAction::Anchor {
                location, ordinal, ..
            },
        } => debug_cmd::anchor(&cwd()?, &location, ordinal),
    }
}

fn repo_here(cwd: &Path) -> Result<Repo> {
    Repo::discover(cwd).context("not inside a git repository")
}
