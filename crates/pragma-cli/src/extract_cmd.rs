//! `pragma extract`: reads this checkout's spec docs into clauses and writes the index. Works
//! in any git repo, enabled or not, so the app can estimate the cost before enabling.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use pragma_engine::config::Config;
use pragma_engine::docs::{discover_in, walk_files};
use pragma_engine::env::{ProcessEnv, pragma_home};
use pragma_engine::events::now_ms;
use pragma_engine::extract::cache::{Cache, PRUNE_AFTER};
use pragma_engine::extract::lockfile;
use pragma_engine::extract::model::{Cancel, ClaudeCli, DEFAULT_TIMEOUT, Model, ModelError};
use pragma_engine::extract::run::{self, Plan, model_calls_for};
use serde_json::Value;

use crate::cancel;
use crate::json::{ExtractEstimateOutput, ExtractFailedOutput, ExtractOutput};
use crate::output::print_json;
use crate::repo_here;

/// Stands in for a model CLI that could not be found: fine while every paragraph is cached.
struct Unavailable {
    name: String,
    error: ModelError,
}

impl Model for Unavailable {
    fn name(&self) -> &str {
        &self.name
    }

    fn complete(&self, _prompt: &str, _schema: &Value) -> Result<Value, ModelError> {
        Err(self.error.clone())
    }
}

pub fn run(cwd: &Path, json: bool, estimate: bool) -> Result<()> {
    // Before anything slow: a parent gone by the time the watch starts is still noticed.
    let parent = cancel::parent_id();
    let repo = repo_here(cwd)?;
    let config = Config::load(repo.effective_pragma_dir().as_deref())?;
    let model_name = config.model().to_string();
    let files = walk_files(&repo.toplevel);
    let docs = discover_in(&repo.toplevel, &config.docs, &files)?;
    let plan = Plan::new(&repo.toplevel, &docs, files);
    let home = pragma_home(&ProcessEnv).context("no home directory; set PRAGMA_HOME")?;
    let cache = Cache::new(&home);
    if estimate {
        let uncached = plan.uncached(&cache, &model_name);
        let output = ExtractEstimateOutput {
            docs: plan.doc_count(),
            paragraphs: plan.paragraph_count(),
            uncached_paragraphs: uncached,
            model_calls: model_calls_for(uncached),
        };
        if json {
            return print_json(&output);
        }
        println!(
            "{} docs, {} paragraphs, {} not yet read: about {} model calls on your Claude account.",
            output.docs, output.paragraphs, output.uncached_paragraphs, output.model_calls
        );
        return Ok(());
    }

    let shared = repo.shared_state_dir();
    let _lock = match lockfile::acquire(&shared)? {
        Ok(lock) => lock,
        Err(holder) => bail!(
            "extraction is already running in this repo (pid {})",
            holder.pid
        ),
    };
    let _ = cache.prune(PRUNE_AFTER);
    // A stop signal or a lost parent stops the model calls in flight; the run then ends below
    // with a `cancelled` error, and dropping `_lock` releases the run.
    let cancel = Arc::new(Cancel::default());
    let _watch = cancel::watch(cancel.clone(), parent);
    let model: Box<dyn Model> = match ClaudeCli::from_env(&ProcessEnv, &model_name, DEFAULT_TIMEOUT)
    {
        Ok(cli) => Box::new(cli.with_cancel(cancel)),
        Err(error) => Box::new(Unavailable {
            name: model_name.clone(),
            error,
        }),
    };
    let state = repo.state_dir();
    let mut last = None;
    let result = run::run(&plan, &cache, model.as_ref(), now_ms(), &mut |progress| {
        let _ = progress.save(&state);
        last = Some(progress.clone());
    });
    let finish = |error: Option<ModelError>| {
        if let Some(mut progress) = last.clone() {
            progress.finished_at = Some(now_ms());
            progress.error = error;
            let _ = progress.save(&state);
        }
    };
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            finish(Some(error.clone()));
            if json {
                print_json(&ExtractFailedOutput {
                    error: error.clone(),
                })?;
            }
            bail!("extraction stopped: {error}");
        }
    };
    outcome.index.save(&state)?;
    finish(None);
    let output = ExtractOutput {
        docs: plan.doc_count(),
        paragraphs: plan.paragraph_count(),
        cached: outcome.cached,
        model_calls: outcome.model_calls,
        clauses: outcome.index.counts(),
        dropped_checks: outcome.dropped_checks,
        errors: outcome.errors,
    };
    if json {
        return print_json(&output);
    }
    print_human(&output);
    Ok(())
}

fn print_human(output: &ExtractOutput) {
    println!(
        "Read {} docs, {} paragraphs ({} from the cache, {} model calls).",
        output.docs, output.paragraphs, output.cached, output.model_calls
    );
    println!(
        "Clauses: {} blocking, {} judged, {} context.",
        output.clauses.blocking, output.clauses.judged, output.clauses.context
    );
    if output.clauses.blocking > 0 {
        println!("Blocking clauses block edits once they are in .pragma/pragma.lock.");
    }
    if !output.dropped_checks.is_empty() {
        println!(
            "{} proposed checks did not validate or cover no file; those clauses are judged.",
            output.dropped_checks.len()
        );
    }
    for error in &output.errors {
        println!("  {error}");
    }
}
