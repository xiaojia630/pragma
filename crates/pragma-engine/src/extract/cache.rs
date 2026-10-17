//! `$PRAGMA_HOME/cache/extract/`: the model's clauses per paragraph, keyed by (paragraph hash,
//! extractor version, model) and shared by every repo and worktree on the host. A corrupt or
//! missing entry is a miss, so the cache can always be deleted.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::EXTRACTOR_VERSION;
use super::prompt::RawClause;
use crate::hashing::short_hash;
use crate::writes::write_atomic;

/// Entries not read or written for this long are pruned.
pub const PRUNE_AFTER: Duration = Duration::from_secs(30 * 24 * 60 * 60);

pub struct Cache {
    dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    para_hash: String,
    extractor: u32,
    model: String,
    clauses: Vec<RawClause>,
}

impl Cache {
    /// The cache under `pragma_home`.
    pub fn new(pragma_home: &Path) -> Cache {
        Cache {
            dir: pragma_home.join("cache").join("extract"),
        }
    }

    fn path(&self, para_hash: &str, model: &str) -> PathBuf {
        let key = short_hash(format!("{para_hash}\n{EXTRACTOR_VERSION}\n{model}").as_bytes());
        self.dir.join(format!("{key}.json"))
    }

    fn read(&self, path: &Path, para_hash: &str, model: &str) -> Option<Vec<RawClause>> {
        let entry: Entry = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
        (entry.para_hash == para_hash
            && entry.extractor == EXTRACTOR_VERSION
            && entry.model == model)
            .then_some(entry.clauses)
    }

    /// The cached clauses, refreshing the entry's age so a live paragraph is never pruned.
    pub fn get(&self, para_hash: &str, model: &str) -> Option<Vec<RawClause>> {
        let path = self.path(para_hash, model);
        let clauses = self.read(&path, para_hash, model)?;
        if let Ok(file) = File::options().append(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(clauses)
    }

    /// True when `get` would hit, without touching the entry.
    pub fn contains(&self, para_hash: &str, model: &str) -> bool {
        self.read(&self.path(para_hash, model), para_hash, model)
            .is_some()
    }

    pub fn put(&self, para_hash: &str, model: &str, clauses: &[RawClause]) -> io::Result<()> {
        let entry = Entry {
            para_hash: para_hash.to_string(),
            extractor: EXTRACTOR_VERSION,
            model: model.to_string(),
            clauses: clauses.to_vec(),
        };
        let text = serde_json::to_string(&entry).map_err(io::Error::other)?;
        write_atomic(&self.path(para_hash, model), text.as_bytes())
    }

    /// Removes entries (and stray temp files) older than `max_age`; returns how many.
    pub fn prune(&self, max_age: Duration) -> io::Result<usize> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        let now = SystemTime::now();
        let mut removed = 0;
        for entry in entries.filter_map(Result::ok) {
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > max_age));
            if old && fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Rung;

    fn clauses(text: &str) -> Vec<RawClause> {
        vec![RawClause {
            text: text.into(),
            rung: Rung::Judged,
            scope: vec![],
            check: None,
            fix: "Fix it.".into(),
        }]
    }

    fn age(path: &Path, by: Duration) {
        File::options()
            .append(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - by)
            .unwrap();
    }

    fn only_file(cache: &Cache) -> PathBuf {
        let mut files: Vec<PathBuf> = fs::read_dir(&cache.dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        files.remove(0)
    }

    #[test]
    fn keys_entries_by_paragraph_and_model() {
        let temp = tempfile::tempdir().unwrap();
        let cache = Cache::new(temp.path());
        assert_eq!(cache.get("h1", "haiku"), None);
        cache.put("h1", "haiku", &clauses("one")).unwrap();
        assert_eq!(cache.get("h1", "haiku"), Some(clauses("one")));
        assert!(cache.contains("h1", "haiku"));
        assert_eq!(cache.get("h1", "sonnet"), None);
        assert_eq!(cache.get("h2", "haiku"), None);
        cache.put("h1", "haiku", &[]).unwrap();
        assert_eq!(cache.get("h1", "haiku"), Some(vec![]));
        assert!(temp.path().join("cache/extract").is_dir());
    }

    #[test]
    fn a_corrupt_entry_is_a_miss() {
        let temp = tempfile::tempdir().unwrap();
        let cache = Cache::new(temp.path());
        cache.put("h1", "haiku", &clauses("one")).unwrap();
        fs::write(only_file(&cache), "{ not json").unwrap();
        assert_eq!(cache.get("h1", "haiku"), None);
        assert!(!cache.contains("h1", "haiku"));
    }

    #[test]
    fn prunes_old_entries_and_reading_keeps_one_young() {
        let temp = tempfile::tempdir().unwrap();
        let cache = Cache::new(temp.path());
        assert_eq!(cache.prune(PRUNE_AFTER).unwrap(), 0);
        cache.put("h1", "haiku", &clauses("one")).unwrap();
        let path = only_file(&cache);
        age(&path, PRUNE_AFTER * 2);
        assert!(cache.get("h1", "haiku").is_some());
        assert_eq!(cache.prune(PRUNE_AFTER).unwrap(), 0);
        age(&path, PRUNE_AFTER * 2);
        assert!(cache.contains("h1", "haiku"));
        assert_eq!(cache.prune(PRUNE_AFTER).unwrap(), 1);
        assert_eq!(cache.get("h1", "haiku"), None);
    }
}
