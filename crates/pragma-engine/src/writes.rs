//! Safe file writes, and the record of what pragma itself wrote to `.pragma/`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use crate::hashing::short_hash;

/// Writes through a temp file and a rename, so a reader never sees half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let temp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}

/// Records the hash of a `.pragma/` file pragma wrote, so turn checks can tell pragma's own
/// writes from anyone else's.
pub fn record_write(shared_state_dir: &Path, rel: &str, bytes: &[u8]) -> io::Result<()> {
    let path = shared_state_dir.join("writes.json");
    let mut writes: BTreeMap<String, String> = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    writes.insert(rel.to_string(), short_hash(bytes));
    let text = serde_json::to_string_pretty(&writes).map_err(io::Error::other)?;
    write_atomic(&path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_creates_parents_and_replaces() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("a/b/c.txt");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(fs::read_dir(temp.path().join("a/b")).unwrap().count(), 1);
    }

    #[test]
    fn record_write_keeps_one_hash_per_file() {
        let temp = tempfile::tempdir().unwrap();
        record_write(temp.path(), ".pragma/overrides.md", b"one").unwrap();
        record_write(temp.path(), ".pragma/overrides.md", b"two").unwrap();
        let text = fs::read_to_string(temp.path().join("writes.json")).unwrap();
        let writes: BTreeMap<String, String> = serde_json::from_str(&text).unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[".pragma/overrides.md"], short_hash(b"two"));
    }
}
