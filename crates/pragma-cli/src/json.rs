//! The shapes of the `--json` output the app reads. Each is a named, owned struct so the
//! app's types can be generated from them (`cargo test --features ts`).

use std::path::PathBuf;

use pragma_engine::audit::AuditReport;
use pragma_engine::backtest::BacktestReport;
use pragma_engine::drift::DriftReport;
use pragma_engine::events::Event;
use pragma_engine::extract::model::ModelError;
use pragma_engine::extract::run::DroppedCheck;
use pragma_engine::extract::status::ExtractionStatus;
use pragma_engine::index::RungCounts;
use pragma_engine::markdown::Anchor;
use pragma_engine::overrides::Exception;
use serde::Serialize;

/// `pragma version --json`.
#[derive(Debug, Serialize)]
pub struct VersionOutput {
    pub version: String,
    pub lock_format: u32,
}

/// Whether pragma's hooks are in Claude Code's settings.
#[derive(Debug, Serialize)]
pub struct HooksState {
    /// The settings file checked; None when no home directory is known.
    pub settings: Option<PathBuf>,
    pub installed: bool,
}

/// One hook pragma installs in Claude Code's settings.
#[derive(Debug, Serialize)]
pub struct HookEvent {
    /// Claude Code's hook event, like `PreToolUse`.
    pub event: String,
    /// The tools the hook runs for; None when it runs for every one.
    pub matcher: Option<String>,
    /// What the hook runs: `pragma hook <subcommand>`.
    pub subcommand: String,
}

/// `pragma hooks status --json`: the hooks' state and the hooks pragma installs.
#[derive(Debug, Serialize)]
pub struct HooksStatusOutput {
    #[serde(flatten)]
    pub state: HooksState,
    pub events: Vec<HookEvent>,
}

/// `pragma hooks install --json`.
#[derive(Debug, Serialize)]
pub struct HooksInstallOutput {
    pub settings: PathBuf,
    pub installed: bool,
}

/// `pragma hooks uninstall --json`.
#[derive(Debug, Serialize)]
pub struct HooksUninstallOutput {
    pub settings: PathBuf,
    /// False when there was no settings file to remove from.
    pub removed: bool,
}

/// `pragma status --json`.
#[derive(Debug, Serialize)]
pub struct Status {
    pub version: String,
    pub lock_format: u32,
    /// The repo has a `.pragma/` that applies here.
    pub enabled: bool,
    pub is_root_checkout: bool,
    pub pragma_dir: Option<PathBuf>,
    pub paused: bool,
    pub hooks: HooksState,
    /// The lock's clause count; None when there is no `.pragma/` or the lock does not load.
    pub clauses: Option<usize>,
    pub lock_error: Option<String>,
    /// Where reading the docs into clauses stands in this checkout.
    pub extraction: ExtractionStatus,
    /// `pragma lock` would add, remove or change blocking clauses. Only ever true in the root
    /// checkout, the one place `pragma lock` runs.
    pub lock_pending: bool,
    /// Doc paragraphs the lock has not settled yet, with no blocking clause change: they settle
    /// on the next `pragma lock`, so they alone are no reason to offer a lock update. 0 outside
    /// the root checkout.
    pub lock_unsettled: usize,
}

/// `pragma init --json`.
#[derive(Debug, Serialize)]
pub struct InitOutput {
    pub pragma_dir: PathBuf,
    /// Repo-relative paths created; empty when `.pragma/` was already set up.
    pub created: Vec<String>,
}

/// `pragma pause --json` and `pragma resume --json`.
#[derive(Debug, Serialize)]
pub struct PausedOutput {
    pub paused: bool,
}

/// `pragma lock check --json`.
#[derive(Debug, Serialize)]
pub struct LockCheckOutput {
    pub clauses: usize,
    pub problems: Vec<String>,
}

/// `pragma lock --json`: what updating the lock from the index changed. Hand-written clauses
/// are never in `removed` or `changed`.
#[derive(Debug, Serialize)]
pub struct LockUpdateOutput {
    /// Blocking clauses taken from the index.
    pub added: Vec<String>,
    /// Extracted clauses whose paragraph is gone from the docs.
    pub removed: Vec<String>,
    /// Entries carried over unchanged, byte for byte, hand-written ones included.
    pub kept: usize,
    /// Extracted clauses whose paragraph changed; its re-derived clauses are in `added`.
    pub changed: Vec<String>,
    /// Paragraphs newly recorded in the lock as settled: from now on a later extraction cannot
    /// add or drop blocking clauses for them until their text changes.
    pub settled: usize,
}

/// One clause in `pragma lock show --json`: what it asks, how to fix a violation, and the
/// paragraph it came from.
#[derive(Debug, Clone, Serialize)]
pub struct ClauseInfo {
    pub id: String,
    pub text: String,
    pub fix: String,
    /// The check's kind: a library rule name, or `pattern` for a generated check.
    pub rule: String,
    pub scope: Vec<String>,
    pub source: Anchor,
}

/// `pragma lock show --json`: the effective lock's clauses and the exceptions recorded
/// against them in `overrides.md`.
#[derive(Debug, Serialize)]
pub struct LockShowOutput {
    pub clauses: Vec<ClauseInfo>,
    pub exceptions: Vec<Exception>,
}

/// One doc in `pragma docs --json`.
#[derive(Debug, Serialize)]
pub struct DocEntry {
    pub path: String,
    pub paragraphs: usize,
    pub historical: bool,
}

/// `pragma docs --json`.
#[derive(Debug, Serialize)]
pub struct DocsOutput {
    pub docs: Vec<DocEntry>,
    pub total_paragraphs: usize,
    /// Docs the exclude globs removed.
    pub excluded: Vec<String>,
}

/// `pragma drift --json`, and the saved `drift.json`.
#[derive(Debug, Serialize)]
pub struct DriftOutput {
    #[serde(flatten)]
    pub report: DriftReport,
    pub stale_exceptions: Vec<Exception>,
}

/// `pragma drift apply --json`.
#[derive(Debug, Serialize)]
pub struct DriftApplyOutput {
    /// Findings whose suggested fix was written to its doc.
    pub applied: Vec<String>,
    /// Ids that were not found, had no fix, or whose doc text had changed.
    pub skipped: Vec<String>,
}

/// `pragma audit --json`, and the saved `audit.json`.
#[derive(Debug, Serialize)]
pub struct AuditOutput {
    #[serde(flatten)]
    pub report: AuditReport,
    pub stale_exceptions: Vec<Exception>,
    /// Directories holding a package manifest ("" is the repo root), so findings can be
    /// grouped by package.
    pub package_roots: Vec<String>,
    /// With `--changed-from`: the files checked, those the branch changed. Absent for an audit
    /// of the whole tree or an area.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<Vec<String>>,
}

/// `pragma backtest --json`, and the saved `backtest.json`. In a partial clone,
/// `incomplete_commits` counts the commits whose files are not local: their hits are left out.
#[derive(Debug, Serialize)]
pub struct BacktestOutput {
    #[serde(flatten)]
    pub report: BacktestReport,
    /// The lock's clauses.
    pub rules: usize,
    /// The lock's clauses history actually runs: those that apply to a commit's files, not to
    /// an edit as it is made (protected paths). 0 means nothing could be checked.
    pub rules_checked: usize,
}

/// One event in `pragma events --json`, with the clause it names.
#[derive(Debug, Serialize)]
pub struct EventInfo {
    #[serde(flatten)]
    pub event: Event,
    /// The clause from the effective lock; None when the event names no clause or the lock no
    /// longer has it.
    pub clause: Option<ClauseInfo>,
}

/// `pragma events --json`: the edit check's events in this checkout, oldest first.
#[derive(Debug, Serialize)]
pub struct EventsOutput {
    pub events: Vec<EventInfo>,
}

/// `pragma exception add --json`.
#[derive(Debug, Serialize)]
pub struct ExceptionOutput {
    /// False when the finding was already an exception.
    pub recorded: bool,
    pub exception: Exception,
}

/// `pragma commit --json`.
#[derive(Debug, Serialize)]
pub struct CommitOutput {
    pub committed: bool,
    pub sha: Option<String>,
    /// Repo-relative paths in the commit.
    pub files: Vec<String>,
}

/// `pragma debug anchor`.
#[derive(Debug, Serialize)]
pub struct DebugAnchorOutput {
    pub id: String,
    pub anchor: Anchor,
}

/// `pragma extract --json`.
#[derive(Debug, Serialize)]
pub struct ExtractOutput {
    /// Docs read; historical docs are not counted.
    pub docs: usize,
    /// Distinct paragraphs; identical ones count once.
    pub paragraphs: usize,
    /// Paragraphs answered from the cache.
    pub cached: usize,
    pub model_calls: usize,
    /// The index's clauses per rung. Blocking ones block only once `pragma lock` takes them.
    pub clauses: RungCounts,
    /// Proposed checks that failed validation (their clauses are judged without a check), and
    /// blocking clauses whose scope covers no file in the repo (judged, check kept). The
    /// index records each reason as the clause's `dropped_reason`.
    pub dropped_checks: Vec<DroppedCheck>,
    /// Calls that failed; their paragraphs are retried on the next run.
    pub errors: Vec<String>,
}

/// `pragma extract --estimate --json`: what a run would cost, without calling the model.
#[derive(Debug, Serialize)]
pub struct ExtractEstimateOutput {
    pub docs: usize,
    pub paragraphs: usize,
    pub uncached_paragraphs: usize,
    pub model_calls: usize,
}

/// `pragma extract --json` when the model is unavailable; the command exits 1 and the index is
/// left as it was.
#[derive(Debug, Serialize)]
pub struct ExtractFailedOutput {
    pub error: ModelError,
}
