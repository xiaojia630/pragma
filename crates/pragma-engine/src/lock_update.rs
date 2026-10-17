//! The lock update rule (§5.2 "The lock", R5): how `pragma lock` takes blocking clauses from a
//! checkout's index into `.pragma/pragma.lock`.
//!
//! A paragraph's identity is its doc path, its text hash and, when the doc's frontmatter sets
//! scope or rung, that frontmatter's fingerprint (`settled_key`): frontmatter wins over the
//! text, so changing it changes the paragraph.
//!
//! - An extracted entry whose paragraph is still in the index keeps its exact bytes and stays
//!   authoritative: the index cannot add new blocking clauses for that paragraph, so model
//!   randomness causes no lock churn.
//! - An extracted entry whose paragraph is no longer in the index is dropped. When the doc still
//!   has a paragraph under the same heading, the paragraph is taken to have changed (`changed`);
//!   otherwise it is gone (`removed`). Either way the index's blocking clauses for the doc's
//!   current paragraphs come in (`added`), which is how a changed paragraph is re-derived.
//! - A doc the index could not fully read (a failed paragraph, or the doc itself) keeps its
//!   extracted entries and settled paragraphs as they are: a partial index never loosens the
//!   lock. Its answered paragraphs still settle and bring their blocking clauses.
//! - A hand-written entry (any `origin` but `extracted`) is never touched. While its paragraph
//!   is in the index it is authoritative for that paragraph too, so extraction does not add a
//!   duplicate beside it.
//! - The lock records every paragraph it has decided on (`Lock::paragraphs`), blocking clauses
//!   or not. A settled paragraph still in the index is held like a kept entry's: no blocking
//!   clause is added or dropped for it until its identity changes or it is gone. So a new
//!   machine or a pruned cache causes no lock churn. Paragraphs the index failed on are not
//!   settled until they get an answer, and neither is a copy that collapsed into another doc's
//!   identical paragraph: that doc answered for it, and the copy settles once it answers for
//!   itself (when the other copy goes, its clauses come in). A copy whose answering copy is not
//!   in the index at all (an untracked draft left out) keeps the settled state it had.
//! - A paragraph the lock has not settled takes the index's blocking clauses.
//!
//! The lock's bytes stay stable because `Lock::to_json` sorts by id, and the caller writes
//! nothing when the update is empty.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::extract::Rung;
use crate::index::{Index, IndexedParagraph};
use crate::lock::{Lock, LockOrigin, LockedClause, settled_doc, settled_key};

#[derive(Debug, Clone, PartialEq)]
pub struct LockUpdate {
    pub lock: Lock,
    /// Ids taken from the index.
    pub added: Vec<String>,
    /// Extracted ids dropped because their paragraph is gone.
    pub removed: Vec<String>,
    /// Extracted ids dropped because their paragraph changed; the re-derived clauses are in
    /// `added`.
    pub changed: Vec<String>,
    /// Entries carried over unchanged, hand-written ones included.
    pub kept: usize,
    /// Paragraphs newly recorded as settled.
    pub settled: usize,
    /// The settled paragraphs changed: some were settled, or some are gone.
    pub paragraphs_changed: bool,
}

impl LockUpdate {
    /// True when blocking clauses would be added, removed or changed: what `lock_pending`
    /// reports. Settling paragraphs alone is not a rule change.
    pub fn changes_rules(&self) -> bool {
        !(self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty())
    }

    /// True when the lock would not change at all, settled paragraphs included.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && !self.paragraphs_changed
    }
}

pub fn update(lock: &Lock, index: &Index) -> LockUpdate {
    let present: HashSet<String> = index.paragraphs.iter().map(IndexedParagraph::key).collect();
    let listed: HashSet<(&str, &str)> = index
        .paragraphs
        .iter()
        .map(|para| (para.path.as_str(), para.para_hash.as_str()))
        .collect();
    // Copies whose answering copy is in this index: they unsettle. A copy whose answering copy
    // was left out (an untracked draft, see `retain_docs`) keeps whatever it had: it neither
    // unsettles, which would churn the lock, nor settles, since that copy's clauses are not here.
    let collapsed: HashSet<String> = index
        .paragraphs
        .iter()
        .filter(|para| {
            para.same_as
                .as_deref()
                .is_some_and(|doc| listed.contains(&(doc, para.para_hash.as_str())))
        })
        .map(IndexedParagraph::key)
        .collect();
    let fingerprints: HashMap<(&str, &str), Option<&str>> = index
        .paragraphs
        .iter()
        .map(|para| {
            (
                (para.path.as_str(), para.para_hash.as_str()),
                para.frontmatter.as_deref(),
            )
        })
        .collect();
    let sections: HashSet<(&str, &[String])> = index
        .paragraphs
        .iter()
        .map(|para| (para.path.as_str(), para.heading_path.as_slice()))
        .collect();
    let incomplete: HashSet<&str> = index.failed.iter().map(|para| para.path.as_str()).collect();
    let failed: HashSet<(&str, &str)> = index
        .failed
        .iter()
        .map(|para| (para.path.as_str(), para.para_hash.as_str()))
        .collect();
    let mut kept_clauses = Vec::new();
    let (mut removed, mut changed) = (Vec::new(), Vec::new());
    for clause in &lock.clauses {
        let anchor = &clause.anchor;
        let here = present.contains(&clause.paragraph_key());
        if here || !clause.is_extracted() || incomplete.contains(anchor.path.as_str()) {
            kept_clauses.push(clause.clone());
        } else if sections.contains(&(anchor.path.as_str(), anchor.heading_path.as_slice())) {
            changed.push(clause.id.clone());
        } else {
            removed.push(clause.id.clone());
        }
    }
    // The paragraphs the lock still speaks for: their blocking clauses are the lock's. That is
    // every settled paragraph still in the docs and answering for itself, every settled
    // paragraph of a doc the index could not fully read, and every paragraph a kept entry
    // anchors to.
    let before: BTreeSet<String> = lock.paragraphs.iter().cloned().collect();
    let still: BTreeSet<String> = before
        .iter()
        .filter(|key| {
            (present.contains(*key) && !collapsed.contains(*key))
                || incomplete.contains(settled_doc(key))
        })
        .cloned()
        .collect();
    // A hand-written clause carries no frontmatter fingerprint: it speaks for its paragraph
    // under whatever frontmatter the doc has now.
    let fingerprint_of =
        |path: &str, hash: &str| fingerprints.get(&(path, hash)).copied().flatten();
    let mut held: HashSet<String> = kept_clauses
        .iter()
        .map(|clause| {
            if clause.is_extracted() {
                clause.paragraph_key()
            } else {
                let anchor = &clause.anchor;
                settled_key(
                    &anchor.path,
                    &anchor.para_hash,
                    fingerprint_of(&anchor.path, &anchor.para_hash),
                )
            }
        })
        .filter(|key| present.contains(key))
        .collect();
    held.extend(still.iter().cloned());
    // After this update every paragraph the index answered for itself is decided on; a failed
    // one waits for an answer, a collapsed copy for its own.
    let mut after = still;
    after.extend(
        index
            .paragraphs
            .iter()
            .filter(|para| {
                para.same_as.is_none()
                    && !failed.contains(&(para.path.as_str(), para.para_hash.as_str()))
            })
            .map(IndexedParagraph::key),
    );
    let settled = after.difference(&before).count();
    let paragraphs_changed = after != before;
    let mut ids: HashSet<String> = kept_clauses.iter().map(|c| c.id.clone()).collect();
    let kept = kept_clauses.len();
    let mut clauses = kept_clauses;
    let mut added = Vec::new();
    for clause in &index.clauses {
        let (Rung::Blocking, Some(check)) = (clause.rung, &clause.check) else {
            continue;
        };
        let anchor = &clause.anchor;
        let frontmatter = fingerprint_of(&anchor.path, &anchor.para_hash);
        let key = settled_key(&anchor.path, &anchor.para_hash, frontmatter);
        if held.contains(&key) || !ids.insert(clause.id.clone()) {
            continue;
        }
        added.push(clause.id.clone());
        clauses.push(LockedClause {
            id: clause.id.clone(),
            anchor: anchor.clone(),
            text: clause.text.clone(),
            scope: clause.scope.clone(),
            check: check.clone(),
            fix: clause.fix.clone(),
            origin: Some(LockOrigin::Extracted),
            frontmatter: frontmatter.map(str::to_string),
        });
    }
    added.sort();
    removed.sort();
    changed.sort();
    LockUpdate {
        lock: Lock {
            format: lock.format,
            clauses,
            paragraphs: after.into_iter().collect(),
        },
        added,
        removed,
        changed,
        kept,
        settled,
        paragraphs_changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{INDEX_FORMAT, IndexedClause, IndexedParagraph, Origin};
    use crate::lock::{Check, LibraryRule};
    use crate::markdown::Anchor;

    fn anchor(path: &str, heading: &str, hash: &str) -> Anchor {
        Anchor {
            path: path.into(),
            heading_path: vec![heading.into()],
            para_hash: hash.into(),
            lines: [3, 3],
            quote: format!("paragraph {hash}"),
        }
    }

    fn locked(id: &str, path: &str, hash: &str, origin: Option<LockOrigin>) -> LockedClause {
        LockedClause {
            id: id.into(),
            anchor: anchor(path, "Rules", hash),
            text: format!("Locked {id}."),
            scope: vec!["drizzle/**".into()],
            check: Check::Library(LibraryRule::ProtectedPaths),
            fix: "Run the generator.".into(),
            origin,
            frontmatter: None,
        }
    }

    fn indexed(id: &str, path: &str, hash: &str, rung: Rung) -> IndexedClause {
        IndexedClause {
            id: id.into(),
            anchor: anchor(path, "Rules", hash),
            text: format!("Indexed {id}."),
            rung,
            scope: vec!["drizzle/**".into()],
            check: (rung == Rung::Blocking).then_some(Check::Library(LibraryRule::ProtectedPaths)),
            fix: "Run the generator.".into(),
            origin: Origin::Repo,
            dropped_reason: None,
        }
    }

    /// An index whose paragraphs are `paras` (path, hash), all under "Rules".
    fn index(paras: &[(&str, &str)], clauses: Vec<IndexedClause>) -> Index {
        Index {
            format: INDEX_FORMAT,
            extractor: 1,
            model: "haiku".into(),
            generated_at: 1,
            paragraphs: paras
                .iter()
                .map(|(path, hash)| IndexedParagraph {
                    path: path.to_string(),
                    para_hash: hash.to_string(),
                    heading_path: vec!["Rules".into()],
                    frontmatter: None,
                    same_as: None,
                })
                .collect(),
            clauses,
            failed: vec![],
        }
    }

    fn lock(clauses: Vec<LockedClause>) -> Lock {
        Lock {
            format: 1,
            clauses,
            paragraphs: vec![],
        }
    }

    fn ids(lock: &Lock) -> Vec<&str> {
        let mut ids: Vec<&str> = lock.clauses.iter().map(|c| c.id.as_str()).collect();
        ids.sort();
        ids
    }

    const EXTRACTED: Option<LockOrigin> = Some(LockOrigin::Extracted);

    fn failure(path: &str, hash: &str) -> crate::index::FailedParagraph {
        crate::index::FailedParagraph {
            path: path.into(),
            para_hash: hash.into(),
            lines: [3, 3],
            reason: "timeout".into(),
        }
    }

    /// Marks `path`'s copy of `hash` as collapsed into `into`'s.
    fn collapse(index: &mut Index, path: &str, hash: &str, into: &str) {
        let para = index
            .paragraphs
            .iter_mut()
            .find(|para| para.path == path && para.para_hash == hash)
            .unwrap();
        para.same_as = Some(into.into());
    }

    #[test]
    fn a_doc_the_index_could_not_fully_read_keeps_its_entries() {
        let mut current = lock(vec![
            locked("old", "AGENTS.md", "h1", EXTRACTED),
            locked("other", "CLAUDE.md", "h5", EXTRACTED),
        ]);
        current.paragraphs = settled(&[("AGENTS.md", "h1"), ("CLAUDE.md", "h5")]);
        // AGENTS.md was edited and one of its paragraphs failed; CLAUDE.md could not be read.
        let mut partial = index(
            &[("AGENTS.md", "h1b"), ("AGENTS.md", "h2")],
            vec![indexed("new", "AGENTS.md", "h1b", Rung::Blocking)],
        );
        partial.failed.push(failure("AGENTS.md", "h2"));
        partial.failed.push(crate::index::FailedParagraph {
            lines: [0, 0],
            ..failure("CLAUDE.md", "")
        });
        let update = update(&current, &partial);
        assert!(
            update.removed.is_empty() && update.changed.is_empty(),
            "{update:?}"
        );
        assert_eq!(update.added, vec!["new"]);
        assert_eq!(ids(&update.lock), vec!["new", "old", "other"]);
        // The unreadable doc's settled paragraph stays settled; the answered one settles.
        assert_eq!(
            update.lock.paragraphs,
            settled(&[
                ("AGENTS.md", "h1"),
                ("AGENTS.md", "h1b"),
                ("CLAUDE.md", "h5")
            ])
        );
    }

    #[test]
    fn frontmatter_is_part_of_a_paragraphs_identity() {
        let with = |fingerprint: &str| {
            let mut index = index(
                &[("db.mdc", "h1")],
                vec![indexed(fingerprint, "db.mdc", "h1", Rung::Blocking)],
            );
            index.paragraphs[0].frontmatter = Some(fingerprint.into());
            index
        };
        let first = update(&Lock::empty(), &with("f1"));
        assert_eq!(first.added, vec!["f1"]);
        assert_eq!(first.lock.clauses[0].frontmatter.as_deref(), Some("f1"));
        assert_eq!(first.lock.paragraphs, vec!["db.mdc#h1@f1"]);
        assert!(super::update(&first.lock, &with("f1")).is_empty());
        // Same text, new frontmatter: the paragraph changed.
        let second = super::update(&first.lock, &with("f2"));
        assert_eq!(second.changed, vec!["f1"]);
        assert_eq!(second.added, vec!["f2"]);
        assert_eq!(second.lock.paragraphs, vec!["db.mdc#h1@f2"]);
    }

    #[test]
    fn a_hand_written_clause_holds_its_paragraph_in_a_doc_with_frontmatter() {
        let hand = locked("hand", "db.mdc", "h1", None);
        let current = lock(vec![hand.clone()]);
        let mut index = index(
            &[("db.mdc", "h1")],
            vec![indexed("fresh", "db.mdc", "h1", Rung::Blocking)],
        );
        index.paragraphs[0].frontmatter = Some("f1".into());
        let update = update(&current, &index);
        assert!(
            update.added.is_empty(),
            "added beside the hand-written clause: {update:?}"
        );
        assert_eq!(update.kept, 1);
        assert_eq!(update.lock.clauses, vec![hand]);
    }

    #[test]
    fn a_copy_whose_answering_copy_is_not_in_the_index_keeps_its_settled_state() {
        // The copy that answered is an untracked draft, filtered out before the update.
        let orphaned = |clauses| {
            let mut index = index(&[("AGENTS.md", "h1")], clauses);
            collapse(&mut index, "AGENTS.md", "h1", "DRAFT.md");
            index
        };
        let mut current = lock(vec![locked("a", "AGENTS.md", "h1", EXTRACTED)]);
        current.paragraphs = settled(&[("AGENTS.md", "h1")]);
        let update = update(&current, &orphaned(vec![]));
        assert!(update.is_empty(), "{update:?}");
        // Not settled yet: it stays unsettled, so its own clauses come in once it answers.
        let first = super::update(&Lock::empty(), &orphaned(vec![]));
        assert!(first.is_empty(), "{first:?}");
        let answered = index(
            &[("AGENTS.md", "h1")],
            vec![indexed("a", "AGENTS.md", "h1", Rung::Blocking)],
        );
        assert_eq!(super::update(&first.lock, &answered).added, vec!["a"]);
    }

    #[test]
    fn a_copy_takes_over_when_the_copy_that_answered_for_it_goes() {
        let mut both = index(
            &[("AGENTS.md", "h1"), ("CLAUDE.md", "h1")],
            vec![indexed("a", "AGENTS.md", "h1", Rung::Blocking)],
        );
        collapse(&mut both, "CLAUDE.md", "h1", "AGENTS.md");
        let first = update(&Lock::empty(), &both);
        assert_eq!(first.added, vec!["a"]);
        assert_eq!(first.lock.paragraphs, settled(&[("AGENTS.md", "h1")]));
        // Only CLAUDE.md has it now: its clause comes in.
        let alone = index(
            &[("AGENTS.md", "h2"), ("CLAUDE.md", "h1")],
            vec![indexed("c", "CLAUDE.md", "h1", Rung::Blocking)],
        );
        let second = super::update(&first.lock, &alone);
        assert_eq!(second.changed, vec!["a"]);
        assert_eq!(second.added, vec!["c"]);
    }

    #[test]
    fn a_copy_of_a_paragraph_that_failed_is_not_settled() {
        let mut both = index(&[("AGENTS.md", "h1"), ("CLAUDE.md", "h1")], vec![]);
        both.failed.push(failure("AGENTS.md", "h1"));
        collapse(&mut both, "CLAUDE.md", "h1", "AGENTS.md");
        let first = update(&Lock::empty(), &both);
        assert!(first.is_empty(), "{first:?}");
        // The failed copy goes before it is retried: the other copy answers and is locked.
        let alone = index(
            &[("CLAUDE.md", "h1")],
            vec![indexed("c", "CLAUDE.md", "h1", Rung::Blocking)],
        );
        let second = super::update(&first.lock, &alone);
        assert_eq!(second.added, vec!["c"]);
        // A lock that settled a copy before copies were marked unsettles it.
        let mut stale = Lock::empty();
        stale.paragraphs = settled(&[("AGENTS.md", "h1"), ("CLAUDE.md", "h1")]);
        let mut again = index(
            &[("AGENTS.md", "h1"), ("CLAUDE.md", "h1")],
            vec![indexed("a", "AGENTS.md", "h1", Rung::Judged)],
        );
        collapse(&mut again, "CLAUDE.md", "h1", "AGENTS.md");
        let migrated = super::update(&stale, &again);
        assert_eq!(migrated.lock.paragraphs, settled(&[("AGENTS.md", "h1")]));
    }

    #[test]
    fn an_unchanged_paragraph_keeps_its_entry_and_takes_no_new_blocking_clause() {
        let current = lock(vec![locked("old", "AGENTS.md", "h1", EXTRACTED)]);
        // A fresh extraction proposes a different blocking clause for the same paragraph.
        let index = index(
            &[("AGENTS.md", "h1")],
            vec![
                indexed("fresh", "AGENTS.md", "h1", Rung::Blocking),
                indexed("judged", "AGENTS.md", "h1", Rung::Judged),
            ],
        );
        let update = update(&current, &index);
        assert!(update.added.is_empty() && update.removed.is_empty() && update.changed.is_empty());
        assert_eq!(update.kept, 1);
        assert_eq!(update.lock.clauses, current.clauses);
        // A lock from before settled paragraphs settles this one now; then nothing changes.
        assert_eq!(update.settled, 1);
        assert!(super::update(&update.lock, &index).is_empty());
    }

    #[test]
    fn a_changed_paragraph_replaces_its_clauses() {
        let current = lock(vec![
            locked("old", "AGENTS.md", "h1", EXTRACTED),
            locked("other", "AGENTS.md", "h2", EXTRACTED),
        ]);
        let index = index(
            &[("AGENTS.md", "h1b"), ("AGENTS.md", "h2")],
            vec![
                indexed("new", "AGENTS.md", "h1b", Rung::Blocking),
                indexed("context", "AGENTS.md", "h1b", Rung::Context),
            ],
        );
        let update = update(&current, &index);
        assert_eq!(update.changed, vec!["old"]);
        assert_eq!(update.added, vec!["new"]);
        assert!(update.removed.is_empty());
        assert_eq!(update.kept, 1);
        assert_eq!(ids(&update.lock), vec!["new", "other"]);
        let new = update.lock.clauses.iter().find(|c| c.id == "new").unwrap();
        assert_eq!(new.origin, EXTRACTED);
        assert_eq!(new.text, "Indexed new.");
        assert_eq!(new.anchor.para_hash, "h1b");
        assert_eq!(new.scope, vec!["drizzle/**"]);
        assert_eq!(new.check, Check::Library(LibraryRule::ProtectedPaths));
    }

    #[test]
    fn a_paragraph_that_is_gone_drops_its_clauses() {
        let current = lock(vec![
            locked("gone", "OLD.md", "h9", EXTRACTED),
            locked("stays", "AGENTS.md", "h1", EXTRACTED),
        ]);
        let index = index(&[("AGENTS.md", "h1")], vec![]);
        let update = update(&current, &index);
        assert_eq!(update.removed, vec!["gone"]);
        assert!(update.changed.is_empty() && update.added.is_empty());
        assert_eq!(ids(&update.lock), vec!["stays"]);
    }

    #[test]
    fn hand_written_clauses_are_never_touched_and_hold_their_paragraph() {
        let hand_gone = locked("hand-gone", "OLD.md", "h9", None);
        let hand_here = locked("hand-here", "AGENTS.md", "h1", None);
        let current = lock(vec![hand_gone.clone(), hand_here.clone()]);
        let index = index(
            &[("AGENTS.md", "h1")],
            vec![indexed("dup", "AGENTS.md", "h1", Rung::Blocking)],
        );
        let update = update(&current, &index);
        assert!(update.added.is_empty() && update.removed.is_empty() && update.changed.is_empty());
        assert_eq!(update.kept, 2);
        assert_eq!(update.lock.clauses, vec![hand_gone, hand_here]);
    }

    #[test]
    fn a_new_paragraph_takes_only_its_blocking_clauses() {
        let index = index(
            &[("AGENTS.md", "h1")],
            vec![
                indexed("b", "AGENTS.md", "h1", Rung::Blocking),
                indexed("j", "AGENTS.md", "h1", Rung::Judged),
                indexed("c", "AGENTS.md", "h1", Rung::Context),
            ],
        );
        let update = update(&Lock::empty(), &index);
        assert_eq!(update.added, vec!["b"]);
        assert_eq!(update.kept, 0);
        assert_eq!(ids(&update.lock), vec!["b"]);
        // Applying the same index again changes nothing, byte for byte.
        let again = super::update(&update.lock, &index);
        assert!(again.is_empty());
        assert_eq!(again.lock.to_json(), update.lock.to_json());
    }

    fn settled(paras: &[(&str, &str)]) -> Vec<String> {
        paras
            .iter()
            .map(|(doc, hash)| settled_key(doc, hash, None))
            .collect()
    }

    #[test]
    fn a_settled_paragraph_with_no_blocking_clause_takes_none_later() {
        let first = index(
            &[("AGENTS.md", "h1")],
            vec![indexed("judged", "AGENTS.md", "h1", Rung::Judged)],
        );
        let update = update(&Lock::empty(), &first);
        assert!(!update.is_empty(), "settling a paragraph changes the lock");
        assert!(!update.changes_rules(), "but it is not a rule change");
        assert_eq!(update.settled, 1);
        assert!(update.added.is_empty());
        assert_eq!(update.lock.paragraphs, settled(&[("AGENTS.md", "h1")]));

        // A later extraction (a new machine, a pruned cache) proposes a blocking clause.
        let later = index(
            &[("AGENTS.md", "h1")],
            vec![indexed("fresh", "AGENTS.md", "h1", Rung::Blocking)],
        );
        let again = super::update(&update.lock, &later);
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(again.lock.to_json(), update.lock.to_json());
    }

    #[test]
    fn a_settled_paragraph_keeps_its_clause_when_extraction_stops_proposing_it() {
        let mut current = lock(vec![locked("old", "AGENTS.md", "h1", EXTRACTED)]);
        current.paragraphs = settled(&[("AGENTS.md", "h1")]);
        let update = update(&current, &index(&[("AGENTS.md", "h1")], vec![]));
        assert!(update.is_empty(), "{update:?}");
        assert_eq!(update.lock, current);
    }

    #[test]
    fn an_edited_settled_paragraph_is_re_derived_and_a_gone_one_unsettled() {
        let mut current = Lock::empty();
        current.paragraphs = settled(&[("AGENTS.md", "h1"), ("OLD.md", "h9")]);
        let update = update(
            &current,
            &index(
                &[("AGENTS.md", "h1b")],
                vec![indexed("new", "AGENTS.md", "h1b", Rung::Blocking)],
            ),
        );
        assert_eq!(update.added, vec!["new"]);
        assert_eq!(update.settled, 1);
        assert!(update.changes_rules());
        assert_eq!(update.lock.paragraphs, settled(&[("AGENTS.md", "h1b")]));
    }

    #[test]
    fn a_failed_paragraph_is_not_settled() {
        let mut failed = index(&[("AGENTS.md", "h1")], vec![]);
        failed.failed.push(crate::index::FailedParagraph {
            path: "AGENTS.md".into(),
            para_hash: "h1".into(),
            lines: [3, 3],
            reason: "timeout".into(),
        });
        let update = update(&Lock::empty(), &failed);
        assert!(update.is_empty(), "{update:?}");
        assert!(update.lock.paragraphs.is_empty());
    }

    #[test]
    fn a_lock_without_settled_paragraphs_still_parses_checks_and_updates() {
        // A copy of this repo's own lock as it was written by hand, before extraction existed.
        let text = include_str!("../testdata/lock-before-extraction.lock");
        let dogfood = Lock::parse(text).unwrap();
        assert!(dogfood.paragraphs.is_empty());
        assert!(crate::rules::validate(&dogfood).is_empty());
        assert!(!dogfood.to_json().contains("\"paragraphs\""));
        let update = update(&dogfood, &index(&[], vec![]));
        assert!(update.is_empty());
        assert_eq!(update.kept, dogfood.clauses.len());
        assert_eq!(update.lock.to_json(), dogfood.to_json());
        // Fields a reader does not know are ignored, as an older binary ignores `paragraphs`.
        let newer = r#"{"format": 1, "clauses": [], "paragraphs": ["A.md#h"],
                        "later": true}"#;
        assert_eq!(Lock::parse(newer).unwrap().paragraphs.len(), 1);
    }
}
