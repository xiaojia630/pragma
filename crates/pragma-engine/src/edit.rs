//! Rebuilds a file as it would be after a Claude Code edit tool call.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::paths::normalize;
use crate::rules::FileOp;

/// The parts of Claude Code's hook input pragma reads. Unknown fields are ignored, and every
/// field is optional, so a future Claude Code version cannot make the parse fail.
#[derive(Debug, Default, Deserialize)]
pub struct HookInput {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedEdit {
    /// Absolute and normalized.
    pub file_path: PathBuf,
    pub before: String,
    pub after: String,
    pub op: FileOp,
}

/// The edit the tool call would make, or None when the call is not a file edit pragma
/// understands, or would fail anyway (its old string is missing or ambiguous).
pub fn proposed_edit(
    input: &HookInput,
    cwd: &Path,
    read: impl Fn(&Path) -> Option<String>,
) -> Option<ProposedEdit> {
    let tool = input.tool_name.as_deref()?;
    let args = &input.tool_input;
    let file_path = normalize(cwd, Path::new(args.get("file_path")?.as_str()?));
    let existing = read(&file_path);
    let op = if existing.is_some() {
        FileOp::Modify
    } else {
        FileOp::Create
    };
    let before = existing.unwrap_or_default();
    let after = match tool {
        "Write" => args.get("content")?.as_str()?.to_string(),
        "Edit" => apply(&before, args)?,
        "MultiEdit" => {
            let mut text = before.clone();
            for edit in args.get("edits")?.as_array()? {
                text = apply(&text, edit)?;
            }
            text
        }
        _ => return None,
    };
    Some(ProposedEdit {
        file_path,
        before,
        after,
        op,
    })
}

/// Applies one `{old_string, new_string, replace_all}` edit the way the Edit tool does.
fn apply(text: &str, edit: &Value) -> Option<String> {
    let old = edit.get("old_string")?.as_str()?;
    let new = edit.get("new_string")?.as_str()?;
    let replace_all = edit
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if old.is_empty() {
        return text.is_empty().then(|| new.to_string());
    }
    match text.matches(old).count() {
        0 => None,
        1 => Some(text.replacen(old, new, 1)),
        _ if replace_all => Some(text.replace(old, new)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(tool: &str, tool_input: Value) -> HookInput {
        HookInput {
            tool_name: Some(tool.into()),
            tool_input,
            ..HookInput::default()
        }
    }

    fn reader(content: Option<&'static str>) -> impl Fn(&Path) -> Option<String> {
        move |_| content.map(str::to_string)
    }

    #[test]
    fn write_creates_or_replaces() {
        let cwd = Path::new("/repo");
        let edit = proposed_edit(
            &input("Write", json!({"file_path": "src/a.ts", "content": "new"})),
            cwd,
            reader(None),
        )
        .unwrap();
        assert_eq!(edit.op, FileOp::Create);
        assert_eq!((edit.before.as_str(), edit.after.as_str()), ("", "new"));
        assert!(edit.file_path.ends_with("src/a.ts"));
        let edit = proposed_edit(
            &input(
                "Write",
                json!({"file_path": "/repo/a.ts", "content": "new"}),
            ),
            cwd,
            reader(Some("old")),
        )
        .unwrap();
        assert_eq!(edit.op, FileOp::Modify);
    }

    #[test]
    fn edit_replaces_one_unique_match_or_all_when_asked() {
        let cwd = Path::new("/repo");
        let one = json!({"file_path": "/repo/a.ts", "old_string": "b", "new_string": "c"});
        assert_eq!(
            proposed_edit(&input("Edit", one), cwd, reader(Some("a b")))
                .unwrap()
                .after,
            "a c"
        );
        let ambiguous = json!({"file_path": "/repo/a.ts", "old_string": "b", "new_string": "c"});
        assert_eq!(
            proposed_edit(&input("Edit", ambiguous), cwd, reader(Some("b b"))),
            None
        );
        let all = json!({"file_path": "/repo/a.ts", "old_string": "b", "new_string": "c", "replace_all": true});
        assert_eq!(
            proposed_edit(&input("Edit", all), cwd, reader(Some("b b")))
                .unwrap()
                .after,
            "c c"
        );
        let missing = json!({"file_path": "/repo/a.ts", "old_string": "zzz", "new_string": "c"});
        assert_eq!(
            proposed_edit(&input("Edit", missing), cwd, reader(Some("b"))),
            None
        );
    }

    #[test]
    fn multi_edit_applies_in_order_and_fails_as_a_whole() {
        let cwd = Path::new("/repo");
        let edits = json!({"file_path": "/repo/a.ts", "edits": [
            {"old_string": "a", "new_string": "b"},
            {"old_string": "b c", "new_string": "d"}
        ]});
        assert_eq!(
            proposed_edit(&input("MultiEdit", edits), cwd, reader(Some("a c")))
                .unwrap()
                .after,
            "d"
        );
        let broken =
            json!({"file_path": "/repo/a.ts", "edits": [{"old_string": "zzz", "new_string": "b"}]});
        assert_eq!(
            proposed_edit(&input("MultiEdit", broken), cwd, reader(Some("a"))),
            None
        );
    }

    #[test]
    fn ignores_other_tools_and_malformed_input() {
        let cwd = Path::new("/repo");
        assert_eq!(
            proposed_edit(&input("Bash", json!({"command": "ls"})), cwd, reader(None)),
            None
        );
        assert_eq!(
            proposed_edit(&input("Write", json!({"content": "x"})), cwd, reader(None)),
            None
        );
        assert_eq!(
            proposed_edit(&HookInput::default(), cwd, reader(None)),
            None
        );
    }
}
