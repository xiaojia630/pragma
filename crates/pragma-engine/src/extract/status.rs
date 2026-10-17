//! Where extraction stands in a checkout, for `pragma status` and the app: never run, running,
//! done, or paused because the model was unavailable on the last run.

use serde::Serialize;

use super::lockfile;
use super::model::ModelError;
use super::run::ExtractProgress;
use crate::index::{Index, RungCounts};
use crate::repo::Repo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtractionState {
    Never,
    /// A run holds the repo's extraction lock (it may be another worktree's).
    Running,
    Done,
    /// The last run stopped because the model was unavailable; the index, if any, is older.
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtractionStatus {
    pub state: ExtractionState,
    /// Why extraction is paused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<ModelError>,
    /// The index's clauses per rung, when there is an index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clauses: Option<RungCounts>,
    /// Milliseconds since the Unix epoch: when the run started (running), stopped (paused) or
    /// wrote the index (done).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// This checkout's run as it goes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<ExtractProgress>,
}

pub fn status(repo: &Repo) -> ExtractionStatus {
    let state_dir = repo.state_dir();
    let index = Index::load(&state_dir);
    let clauses = index.as_ref().map(Index::counts);
    let progress = ExtractProgress::load(&state_dir);
    let mut status = ExtractionStatus {
        state: ExtractionState::Never,
        reason: None,
        clauses,
        at: None,
        progress: None,
    };
    if let Some(holder) = lockfile::holder(&repo.shared_state_dir()) {
        status.state = ExtractionState::Running;
        status.at = Some(holder.started_at);
        status.progress = progress.filter(|progress| progress.finished_at.is_none());
        return status;
    }
    let generated_at = index.as_ref().map(|index| index.generated_at);
    if let Some(progress) = progress
        && let (Some(error), Some(finished_at)) = (&progress.error, progress.finished_at)
        && generated_at.is_none_or(|generated_at| finished_at > generated_at)
    {
        status.state = ExtractionState::Paused;
        status.reason = Some(error.clone());
        status.at = Some(finished_at);
        return status;
    }
    if let Some(generated_at) = generated_at {
        status.state = ExtractionState::Done;
        status.at = Some(generated_at);
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::model::ModelErrorKind;
    use crate::index::INDEX_FORMAT;
    use crate::testutil::repo;

    fn progress(finished_at: Option<u64>, error: Option<ModelError>) -> ExtractProgress {
        ExtractProgress {
            total: 10,
            done: 4,
            cached: 2,
            clauses_so_far: 3,
            started_at: 100,
            finished_at,
            error,
        }
    }

    fn index(generated_at: u64) -> Index {
        Index {
            format: INDEX_FORMAT,
            extractor: 1,
            model: "haiku".into(),
            generated_at,
            paragraphs: vec![],
            clauses: vec![],
            failed: vec![],
        }
    }

    #[test]
    fn never_then_done_then_paused_then_done_again() {
        let (_temp, root) = repo();
        let repo = Repo::discover(&root).unwrap();
        let state = repo.state_dir();
        assert_eq!(status(&repo).state, ExtractionState::Never);

        index(200).save(&state).unwrap();
        progress(Some(200), None).save(&state).unwrap();
        let done = status(&repo);
        assert_eq!(done.state, ExtractionState::Done);
        assert_eq!(done.at, Some(200));
        assert_eq!(done.clauses, Some(RungCounts::default()));

        let logged_out = ModelError {
            kind: ModelErrorKind::LoggedOut,
            message: "not logged in".into(),
        };
        progress(Some(300), Some(logged_out.clone()))
            .save(&state)
            .unwrap();
        let paused = status(&repo);
        assert_eq!(paused.state, ExtractionState::Paused);
        assert_eq!(paused.reason, Some(logged_out.clone()));
        assert_eq!(paused.at, Some(300));
        assert!(paused.clauses.is_some(), "the older index still counts");

        index(400).save(&state).unwrap();
        assert_eq!(status(&repo).state, ExtractionState::Done);
    }

    #[test]
    fn running_while_a_live_run_holds_the_lock() {
        let (_temp, root) = repo();
        let repo = Repo::discover(&root).unwrap();
        progress(None, None).save(&repo.state_dir()).unwrap();
        let _lock = lockfile::acquire(&repo.shared_state_dir())
            .unwrap()
            .unwrap();
        let running = status(&repo);
        assert_eq!(running.state, ExtractionState::Running);
        assert_eq!(running.progress.unwrap().done, 4);
        assert!(running.at.is_some());
    }
}
