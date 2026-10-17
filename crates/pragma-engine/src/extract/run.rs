//! One extraction run: plan the paragraphs, answer what the cache can, ask the model for the
//! rest (4 calls at a time), and build the index.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

use serde::{Deserialize, Serialize};

use super::EXTRACTOR_VERSION;
use super::cache::Cache;
use super::derive::{DocContext, closer_first, derive, require_files};
use super::model::{Model, ModelError, ModelErrorKind};
use super::prompt::{self, BATCH_SIZE, PromptParagraph, RawClause};
use crate::docs::DocFile;
use crate::events::now_ms;
use crate::hashing::normalize_ws;
use crate::index::{FailedParagraph, INDEX_FORMAT, Index, IndexedClause, IndexedParagraph, Origin};
use crate::markdown::{Anchor, Paragraph, anchor_for, clause_id, paragraphs};

/// Model calls in flight at once (S0a ran 4 in parallel).
pub const PARALLEL_CALLS: usize = 4;

/// Written to `<git dir>/pragma/extract-progress.json` as a run goes, for the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractProgress {
    /// Distinct paragraphs to extract.
    pub total: usize,
    /// Paragraphs answered so far, from the cache or the model, or given up on this run.
    pub done: usize,
    pub cached: usize,
    pub clauses_so_far: usize,
    /// Milliseconds since the Unix epoch.
    pub started_at: u64,
    /// Set when the run ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    /// Why the run stopped, when the model was unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ModelError>,
}

pub const PROGRESS_FILE: &str = "extract-progress.json";

impl ExtractProgress {
    pub fn load(state_dir: &Path) -> Option<ExtractProgress> {
        serde_json::from_str(&std::fs::read_to_string(state_dir.join(PROGRESS_FILE)).ok()?).ok()
    }

    pub fn save(&self, state_dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        crate::writes::write_atomic(&state_dir.join(PROGRESS_FILE), text.as_bytes())
    }
}

/// A proposed check that failed validation, so its clause became judged without it, or a
/// blocking clause whose scope covers no file in the repo, so it became judged (R8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DroppedCheck {
    pub id: String,
    pub reason: String,
}

/// The docs and paragraphs a run covers. Historical docs yield nothing.
pub struct Plan {
    /// The checkout's root: a blocking clause's scope may name a directory the walk skips.
    root: PathBuf,
    docs: Vec<PlannedDoc>,
    /// Docs that could not be read, with why: recorded as failed, so the lock keeps theirs.
    unreadable: Vec<(String, String)>,
    unique: Vec<UniqueParagraph>,
    /// Every file in the checkout (`walk_files`): a blocking clause must cover at least one.
    files: Vec<String>,
}

struct PlannedDoc {
    context: DocContext,
    paragraphs: Vec<(Paragraph, Anchor)>,
}

struct UniqueParagraph {
    hash: String,
    heading_path: Vec<String>,
    text: String,
}

pub struct Outcome {
    pub index: Index,
    pub cached: usize,
    pub model_calls: usize,
    pub dropped_checks: Vec<DroppedCheck>,
    pub errors: Vec<String>,
}

impl Plan {
    pub fn new(root: &Path, docs: &[DocFile], files: Vec<String>) -> Plan {
        let mut planned = Vec::new();
        let mut unreadable = Vec::new();
        let mut unique: Vec<UniqueParagraph> = Vec::new();
        let mut seen = HashSet::new();
        for doc in docs.iter().filter(|doc| !doc.historical) {
            let text = match std::fs::read_to_string(root.join(&doc.rel)) {
                Ok(text) => text,
                Err(error) => {
                    unreadable.push((doc.rel.clone(), error.to_string()));
                    continue;
                }
            };
            let paras: Vec<(Paragraph, Anchor)> = paragraphs(&text)
                .into_iter()
                .map(|para| {
                    let anchor = anchor_for(&doc.rel, &text, &para);
                    (para, anchor)
                })
                .collect();
            for (para, _) in &paras {
                if seen.insert(para.hash.clone()) {
                    unique.push(UniqueParagraph {
                        hash: para.hash.clone(),
                        heading_path: para.heading_path.clone(),
                        text: text[para.start..para.end].trim().to_string(),
                    });
                }
            }
            planned.push(PlannedDoc {
                context: DocContext::new(&doc.rel, &text),
                paragraphs: paras,
            });
        }
        Plan {
            root: root.to_path_buf(),
            docs: planned,
            unreadable,
            unique,
            files,
        }
    }

    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }

    /// Distinct paragraphs: identical ones are extracted once.
    pub fn paragraph_count(&self) -> usize {
        self.unique.len()
    }

    pub fn uncached(&self, cache: &Cache, model: &str) -> usize {
        self.unique
            .iter()
            .filter(|para| !cache.contains(&para.hash, model))
            .count()
    }
}

/// Model calls needed for `uncached` paragraphs.
pub fn model_calls_for(uncached: usize) -> usize {
    uncached.div_ceil(BATCH_SIZE)
}

fn is_fatal(error: &ModelError) -> bool {
    matches!(
        error.kind,
        ModelErrorKind::Missing | ModelErrorKind::LoggedOut | ModelErrorKind::Cancelled
    )
}

/// Runs extraction. A missing or logged-out model stops the run with its error, as does a run
/// where every call failed; otherwise paragraphs whose call failed are listed in the index's
/// `failed` and retried next time. Every answer is cached as it arrives.
pub fn run(
    plan: &Plan,
    cache: &Cache,
    model: &dyn Model,
    started_at: u64,
    report: &mut dyn FnMut(&ExtractProgress),
) -> Result<Outcome, ModelError> {
    let name = model.name().to_string();
    let mut answers: HashMap<&str, Vec<RawClause>> = HashMap::new();
    let mut pending: Vec<&UniqueParagraph> = Vec::new();
    for para in &plan.unique {
        match cache.get(&para.hash, &name) {
            Some(clauses) => {
                answers.insert(&para.hash, clauses);
            }
            None => pending.push(para),
        }
    }
    let cached = answers.len();
    let mut progress = ExtractProgress {
        total: plan.unique.len(),
        done: cached,
        cached,
        clauses_so_far: answers.values().map(Vec::len).sum(),
        started_at,
        finished_at: None,
        error: None,
    };
    report(&progress);

    let batches: Vec<&[&UniqueParagraph]> = pending.chunks(BATCH_SIZE).collect();
    let schema = prompt::schema();
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let mut failures: HashMap<&str, String> = HashMap::new();
    let mut errors = Vec::new();
    let mut fatal: Option<ModelError> = None;
    let mut first_error: Option<ModelError> = None;
    let mut model_calls = 0;
    let mut answered_calls = 0;
    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        for _ in 0..PARALLEL_CALLS.min(batches.len()) {
            let sender = sender.clone();
            let (batches, next, stop, schema) = (&batches, &next, &stop, &schema);
            scope.spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let at = next.fetch_add(1, Ordering::SeqCst);
                    let Some(batch) = batches.get(at) else {
                        break;
                    };
                    let paras: Vec<PromptParagraph> = batch
                        .iter()
                        .enumerate()
                        .map(|(n, para)| PromptParagraph {
                            id: format!("p{n}"),
                            heading_path: &para.heading_path,
                            text: &para.text,
                        })
                        .collect();
                    let answer = model.complete(&prompt::build(&paras), schema);
                    if sender.send((at, answer)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        for (at, answer) in receiver {
            model_calls += 1;
            let batch = batches[at];
            match answer {
                Ok(answer) => {
                    answered_calls += 1;
                    let mut found = prompt::parse_answer(&answer);
                    for (n, para) in batch.iter().enumerate() {
                        match found.remove(&format!("p{n}")) {
                            Some(clauses) => {
                                if let Err(error) = cache.put(&para.hash, &name, &clauses) {
                                    errors.push(format!("writing the cache: {error}"));
                                }
                                progress.clauses_so_far += clauses.len();
                                answers.insert(&para.hash, clauses);
                            }
                            None => {
                                failures.insert(
                                    &para.hash,
                                    "the model's answer left this paragraph out, or none of its \
                                     clauses could be read"
                                        .into(),
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    errors.push(format!(
                        "a call for {} paragraphs failed: {error}",
                        batch.len()
                    ));
                    for para in batch {
                        failures.insert(&para.hash, error.message.clone());
                    }
                    if is_fatal(&error) {
                        stop.store(true, Ordering::SeqCst);
                        fatal.get_or_insert(error.clone());
                    }
                    first_error.get_or_insert(error);
                }
            }
            progress.done += batch.len();
            report(&progress);
        }
    });
    if let Some(error) = fatal {
        return Err(error);
    }
    if let (0, Some(error)) = (answered_calls, first_error) {
        return Err(error);
    }

    let mut docs: Vec<&PlannedDoc> = plan.docs.iter().collect();
    docs.sort_by(|a, b| closer_first(&a.context, &b.context));
    let mut paragraphs = Vec::new();
    let mut clauses = Vec::new();
    let mut failed: Vec<FailedParagraph> = plan
        .unreadable
        .iter()
        .map(|(rel, error)| FailedParagraph {
            path: rel.clone(),
            para_hash: String::new(),
            lines: [0, 0],
            reason: format!("could not read the doc: {error}"),
        })
        .collect();
    errors.extend(
        plan.unreadable
            .iter()
            .map(|(rel, error)| format!("could not read {rel}: {error}")),
    );
    let mut dropped_checks = Vec::new();
    // The first doc (closer first) with each paragraph answers for every identical copy.
    let mut representatives = HashMap::new();
    for doc in docs {
        let context = &doc.context;
        let fingerprint = context.fingerprint();
        for (para, anchor) in &doc.paragraphs {
            // The same paragraph twice with the same scope basis is one paragraph.
            let key = (
                para.hash.as_str(),
                &context.dir,
                &context.frontmatter_scope,
                context.frontmatter_rung,
            );
            let first = !representatives.contains_key(&key);
            let representative: &str = representatives.entry(key).or_insert(&context.rel);
            paragraphs.push(IndexedParagraph {
                path: context.rel.clone(),
                para_hash: para.hash.clone(),
                heading_path: para.heading_path.clone(),
                frontmatter: fingerprint.clone(),
                same_as: (representative != context.rel).then(|| representative.to_string()),
            });
            if !first {
                continue;
            }
            let Some(raws) = answers.get(para.hash.as_str()) else {
                failed.push(FailedParagraph {
                    path: context.rel.clone(),
                    para_hash: para.hash.clone(),
                    lines: para.lines,
                    reason: failures
                        .get(para.hash.as_str())
                        .cloned()
                        .unwrap_or_else(|| "not extracted".into()),
                });
                continue;
            };
            for (ordinal, raw) in raws.iter().enumerate() {
                let mut derived = derive(raw, context);
                require_files(&mut derived, &plan.files, &plan.root);
                let id = clause_id(&context.rel, &para.hash, ordinal as u32);
                if let Some(reason) = &derived.dropped {
                    dropped_checks.push(DroppedCheck {
                        id: id.clone(),
                        reason: reason.clone(),
                    });
                }
                clauses.push(IndexedClause {
                    id,
                    anchor: anchor.clone(),
                    text: normalize_ws(&raw.text),
                    rung: derived.rung,
                    scope: derived.scope,
                    check: derived.check,
                    fix: normalize_ws(&raw.fix),
                    origin: Origin::Repo,
                    dropped_reason: derived.dropped,
                });
            }
        }
    }
    Ok(Outcome {
        index: Index {
            format: INDEX_FORMAT,
            extractor: EXTRACTOR_VERSION,
            model: name,
            generated_at: now_ms(),
            paragraphs,
            clauses,
            failed,
        },
        cached,
        model_calls,
        dropped_checks,
        errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docs::walk_files;
    use crate::extract::Rung;
    use crate::lock::{Check, LibraryRule};
    use serde_json::{Value, json};
    use std::fs;
    use std::sync::Mutex;

    /// A model that answers from the paragraph text: "migrations" is a protected path,
    /// "require" a pattern, "background" context, "skip" left out of the answer, anything else
    /// no clauses.
    struct Fake {
        calls: Mutex<Vec<usize>>,
        fail: Option<ModelErrorKind>,
    }

    impl Fake {
        fn new(fail: Option<ModelErrorKind>) -> Fake {
            Fake {
                calls: Mutex::new(Vec::new()),
                fail,
            }
        }

        fn calls(&self) -> Vec<usize> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Model for Fake {
        fn name(&self) -> &str {
            "fake"
        }

        fn complete(&self, prompt: &str, _schema: &Value) -> Result<Value, ModelError> {
            let lines: Vec<Value> = prompt
                .lines()
                .filter(|line| line.starts_with(r#"{"id":"#))
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            self.calls.lock().unwrap().push(lines.len());
            if let Some(kind) = self.fail {
                std::thread::sleep(std::time::Duration::from_millis(30));
                return Err(ModelError {
                    kind,
                    message: format!("{kind:?}"),
                });
            }
            let paragraphs: Vec<Value> = lines
                .iter()
                .filter(|line| !line["text"].as_str().unwrap().contains("skip"))
                .map(|line| {
                    let text = line["text"].as_str().unwrap();
                    let clauses = if text.contains("migrations") {
                        json!([{ "text": "Do not hand-edit migrations.", "rung": "blocking",
                                 "scope": ["drizzle/**"], "check": { "rule": "protected-paths" },
                                 "fix": "Run the generator." }])
                    } else if text.contains("require") {
                        json!([{ "text": "Do not use require().", "rung": "blocking",
                                 "scope": ["**/*.ts"],
                                 "check": { "rule": "forbidden-pattern", "pattern": "require($$$A)",
                                            "bad": "const a = require('a');",
                                            "good": "import a from 'a';" },
                                 "fix": "Use import." }])
                    } else if text.contains("garbled") {
                        json!([{ "text": "A clause with nothing else." }])
                    } else if text.contains("vendored") {
                        json!([{ "text": "Do not hand-edit vendored code.", "rung": "blocking",
                                 "scope": ["vendor/**"], "check": { "rule": "protected-paths" },
                                 "fix": "Update the dependency." }])
                    } else if text.contains("everywhere") {
                        json!([{ "text": "Protect everything.", "rung": "blocking", "scope": [],
                                 "check": { "rule": "protected-paths" }, "fix": "Do not." }])
                    } else if text.contains("background") {
                        json!([{ "text": "This is background.", "rung": "context", "scope": [],
                                 "check": null, "fix": "None." }])
                    } else {
                        json!([])
                    };
                    json!({ "id": line["id"], "clauses": clauses })
                })
                .collect();
            Ok(json!({ "paragraphs": paragraphs }))
        }
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn doc(rel: &str) -> DocFile {
        DocFile {
            rel: rel.into(),
            historical: false,
        }
    }

    fn extract(root: &Path, home: &Path, docs: &[DocFile], model: &Fake) -> Outcome {
        let plan = Plan::new(root, docs, walk_files(root));
        run(&plan, &Cache::new(home), model, 1, &mut |_| {}).unwrap()
    }

    #[test]
    fn extracts_derives_and_caches() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        write(
            &root,
            "AGENTS.md",
            "# Rules\n\nDo not hand-edit migrations.\n\nNever require() modules.\n\nSome background.\n\nNothing to see.\n\nProtect everywhere.\n",
        );
        write(&root, "drizzle/0001_init.sql", "create table a;\n");
        let model = Fake::new(None);
        let outcome = extract(&root, &home, &[doc("AGENTS.md")], &model);
        assert_eq!(model.calls(), vec![5]);
        assert_eq!((outcome.cached, outcome.model_calls), (0, 1));
        let index = &outcome.index;
        let rungs: Vec<(Rung, &str)> = index
            .clauses
            .iter()
            .map(|clause| (clause.rung, clause.text.as_str()))
            .collect();
        assert_eq!(
            rungs,
            vec![
                (Rung::Blocking, "Do not hand-edit migrations."),
                (Rung::Judged, "Do not use require()."),
                (Rung::Context, "This is background."),
                (Rung::Judged, "Protect everything."),
            ]
        );
        assert_eq!(
            index.clauses[0].check,
            Some(Check::Library(LibraryRule::ProtectedPaths))
        );
        assert_eq!(index.clauses[0].anchor.lines, [3, 3]);
        assert_eq!(index.clauses[0].anchor.heading_path, vec!["Rules"]);
        assert_eq!(outcome.dropped_checks.len(), 1);
        assert_eq!(outcome.dropped_checks[0].id, index.clauses[3].id);
        // Every paragraph is listed, the ones with no clauses too.
        assert_eq!(index.paragraphs.len(), 5);
        assert!(
            index
                .paragraphs
                .iter()
                .all(|para| para.path == "AGENTS.md" && para.heading_path == vec!["Rules"])
        );
        assert!(index.failed.is_empty());

        let again = extract(&root, &home, &[doc("AGENTS.md")], &model);
        assert_eq!(
            model.calls(),
            vec![5],
            "every paragraph came from the cache"
        );
        assert_eq!((again.cached, again.model_calls), (5, 0));
        assert_eq!(again.index.clauses, outcome.index.clauses);
    }

    #[test]
    fn batches_of_ten_and_identical_paragraphs_once() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        let body: String = (0..23).map(|n| format!("Paragraph {n}.\n\n")).collect();
        write(
            &root,
            "AGENTS.md",
            &format!("# A\n\n{body}Do not hand-edit migrations.\n"),
        );
        write(&root, "CLAUDE.md", "Do not hand-edit migrations.\n");
        write(
            &root,
            "apps/web/AGENTS.md",
            "Do not hand-edit migrations.\n",
        );
        let model = Fake::new(None);
        let docs = [
            doc("AGENTS.md"),
            doc("CLAUDE.md"),
            doc("apps/web/AGENTS.md"),
        ];
        let outcome = extract(&root, &home, &docs, &model);
        let mut calls = model.calls();
        calls.sort();
        assert_eq!(calls, vec![4, 10, 10]);
        let migrations: Vec<(&str, &Vec<String>)> = outcome
            .index
            .clauses
            .iter()
            .map(|clause| (clause.anchor.path.as_str(), &clause.scope))
            .collect();
        // The root copies collapse to one; the nested doc keeps its own, scoped, listed first.
        assert_eq!(
            migrations,
            vec![
                (
                    "apps/web/AGENTS.md",
                    &vec!["apps/web/drizzle/**".to_string()]
                ),
                ("AGENTS.md", &vec!["drizzle/**".to_string()]),
            ]
        );
    }

    #[test]
    fn copies_name_the_doc_that_answered_and_unreadable_docs_fail() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        write(&root, "AGENTS.md", "Do not hand-edit migrations.\n");
        write(&root, "CLAUDE.md", "Do not hand-edit migrations.\n");
        write(
            &root,
            ".cursor/rules/db.mdc",
            "---\nglobs: drizzle/**\n---\nDo not hand-edit migrations.\n",
        );
        let model = Fake::new(None);
        let docs = [
            doc("AGENTS.md"),
            doc("CLAUDE.md"),
            doc(".cursor/rules/db.mdc"),
            doc("GONE.md"),
        ];
        let outcome = extract(&root, &home, &docs, &model);
        let para = |path: &str| {
            outcome
                .index
                .paragraphs
                .iter()
                .find(|para| para.path == path)
                .unwrap()
        };
        assert_eq!(para("AGENTS.md").same_as, None);
        assert_eq!(para("CLAUDE.md").same_as.as_deref(), Some("AGENTS.md"));
        // Frontmatter scope is another scope basis: not a copy, and fingerprinted.
        assert_eq!(para(".cursor/rules/db.mdc").same_as, None);
        assert!(para(".cursor/rules/db.mdc").frontmatter.is_some());
        assert_eq!(para("AGENTS.md").frontmatter, None);
        let gone = &outcome.index.failed[0];
        assert_eq!(
            (gone.path.as_str(), gone.para_hash.as_str()),
            ("GONE.md", "")
        );
        assert!(gone.reason.contains("could not read"), "{}", gone.reason);
        assert!(outcome.errors.iter().any(|error| error.contains("GONE.md")));
    }

    #[test]
    fn a_paragraph_whose_clauses_do_not_read_fails_and_is_retried() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        write(
            &root,
            "AGENTS.md",
            "Some background.\n\nA garbled answer.\n",
        );
        let model = Fake::new(None);
        let outcome = extract(&root, &home, &[doc("AGENTS.md")], &model);
        assert_eq!(outcome.index.failed.len(), 1, "{:?}", outcome.index.failed);
        assert_eq!(outcome.index.failed[0].lines, [3, 3]);
        assert!(outcome.index.failed[0].reason.contains("could be read"));
        // Not cached as "no clauses": the next run asks again.
        let again = extract(&root, &home, &[doc("AGENTS.md")], &model);
        assert_eq!(model.calls(), vec![2, 1]);
        assert_eq!(again.cached, 1);
    }

    #[test]
    fn historical_docs_yield_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        write(&root, "docs/adr/0001.md", "Do not hand-edit migrations.\n");
        let model = Fake::new(None);
        let docs = [DocFile {
            rel: "docs/adr/0001.md".into(),
            historical: true,
        }];
        let outcome = extract(&root, &home, &docs, &model);
        assert!(model.calls().is_empty());
        assert!(outcome.index.clauses.is_empty());
        assert_eq!(Plan::new(&root, &docs, vec![]).doc_count(), 0);
    }

    #[test]
    fn a_paragraph_left_out_of_the_answer_fails_and_is_retried() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        write(&root, "AGENTS.md", "Some background.\n\nPlease skip me.\n");
        let model = Fake::new(None);
        let outcome = extract(&root, &home, &[doc("AGENTS.md")], &model);
        assert_eq!(outcome.index.failed.len(), 1);
        assert_eq!(outcome.index.failed[0].lines, [3, 3]);
        assert!(
            outcome.index.failed[0]
                .reason
                .contains("left this paragraph out")
        );
        let again = extract(&root, &home, &[doc("AGENTS.md")], &model);
        assert_eq!(model.calls(), vec![2, 1]);
        assert_eq!(again.cached, 1);
    }

    #[test]
    fn a_missing_or_logged_out_model_stops_the_run() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        let body: String = (0..120).map(|n| format!("Paragraph {n}.\n\n")).collect();
        write(&root, "AGENTS.md", &body);
        let plan = Plan::new(&root, &[doc("AGENTS.md")], vec![]);
        for kind in [ModelErrorKind::LoggedOut, ModelErrorKind::Missing] {
            let model = Fake::new(Some(kind));
            let error = run(&plan, &Cache::new(&home), &model, 1, &mut |_| {})
                .err()
                .unwrap();
            assert_eq!(error.kind, kind);
            // Workers stop taking batches once the model is known to be unavailable.
            assert!(model.calls().len() < 12, "{:?}", model.calls());
        }
    }

    #[test]
    fn a_run_where_every_call_failed_is_an_error_and_reports_progress() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        write(&root, "AGENTS.md", "One.\n\nTwo.\n");
        let plan = Plan::new(&root, &[doc("AGENTS.md")], vec![]);
        assert_eq!(plan.uncached(&Cache::new(&home), "fake"), 2);
        let mut seen = Vec::new();
        let error = run(
            &plan,
            &Cache::new(&home),
            &Fake::new(Some(ModelErrorKind::Timeout)),
            7,
            &mut |progress| seen.push((progress.done, progress.total)),
        )
        .err()
        .unwrap();
        assert_eq!(error.kind, ModelErrorKind::Timeout);
        assert_eq!(seen, vec![(0, 2), (2, 2)]);
    }

    #[test]
    fn a_blocking_clause_whose_scope_covers_no_file_is_judged() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        // The root doc names `drizzle/`, but the migrations live under an app.
        write(&root, "AGENTS.md", "Do not hand-edit migrations.\n");
        write(&root, "apps/x/drizzle/0001_init.sql", "create table a;\n");
        write(&root, "apps/x/AGENTS.md", "Do not hand-edit migrations.\n");
        let model = Fake::new(None);
        let docs = [doc("AGENTS.md"), doc("apps/x/AGENTS.md")];
        let outcome = extract(&root, &home, &docs, &model);
        let by_path = |path: &str| {
            outcome
                .index
                .clauses
                .iter()
                .find(|clause| clause.anchor.path == path)
                .unwrap()
        };
        let nested = by_path("apps/x/AGENTS.md");
        assert_eq!(nested.scope, vec!["apps/x/drizzle/**"]);
        assert_eq!(nested.rung, Rung::Blocking);
        assert_eq!(nested.dropped_reason, None);
        let root_clause = by_path("AGENTS.md");
        assert_eq!(root_clause.rung, Rung::Judged);
        assert_eq!(
            root_clause.check,
            Some(Check::Library(LibraryRule::ProtectedPaths)),
            "the check stays for when the scope is corrected"
        );
        let reason = root_clause.dropped_reason.as_deref().unwrap();
        assert!(reason.contains("matches no file"), "{reason}");
        assert!(
            outcome
                .dropped_checks
                .iter()
                .any(|dropped| dropped.id == root_clause.id && dropped.reason == reason)
        );
    }

    #[test]
    fn a_blocking_clause_on_a_directory_the_walk_skips_stays_blocking() {
        let temp = tempfile::tempdir().unwrap();
        let (root, home) = (temp.path().join("repo"), temp.path().join("home"));
        write(&root, "AGENTS.md", "Never touch vendored code.\n");
        write(&root, "vendor/x.go", "package x\n");
        let model = Fake::new(None);
        let outcome = extract(&root, &home, &[doc("AGENTS.md")], &model);
        assert!(!walk_files(&root).contains(&"vendor/x.go".to_string()));
        let clause = &outcome.index.clauses[0];
        assert_eq!(clause.scope, vec!["vendor/**"]);
        assert_eq!(clause.rung, Rung::Blocking);
        assert_eq!(clause.dropped_reason, None);
        assert!(outcome.dropped_checks.is_empty());
    }

    #[test]
    fn model_calls_round_up() {
        assert_eq!(model_calls_for(0), 0);
        assert_eq!(model_calls_for(10), 1);
        assert_eq!(model_calls_for(11), 2);
    }
}
