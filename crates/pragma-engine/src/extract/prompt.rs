//! The extraction prompt (spike S0a, verbatim), the schema its answer must match, batching, and
//! reading the model's clauses back per paragraph.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Rung;

/// Paragraphs per model call. S0a ran AGENTS.md in batches of 10 at a median of 20 s each.
pub const BATCH_SIZE: usize = 10;

/// The S0a extraction prompt, verbatim. The paragraphs follow it, one JSON object per line.
pub const EXTRACTION_PROMPT: &str = r#"You turn a team's engineering docs into the specific things they ask for.

For each paragraph below, list its clauses: each single thing the paragraph asks a coding agent to do
or not do. For each clause give:
- "text": the instruction in one sentence, in the doc's words where possible.
- "rung": "blocking" if code can check it on a single file edit, "judged" if a reviewer could check
  it from a diff, "context" if it is background an agent should know.
- "scope": glob patterns for the files it applies to, from paths the paragraph names; [] when it
  applies everywhere.
- "check": for blocking clauses, one of
  {"rule": "forbidden-pattern", "pattern": "<ast-grep pattern>", "bad": "<code that breaks it>", "good": "<code that follows it>"},
  {"rule": "import-boundary", "forbid": ["<module specifier glob>"]},
  {"rule": "protected-paths"},
  {"rule": "file-naming", "case": "kebab" | "camel" | "pascal" | "snake"};
  null for other rungs.
- "fix": how to fix a violation, in one sentence.

A paragraph with no instruction gets an empty list. Answer with JSON only, no prose:
{"paragraphs": [{"id": "<id>", "clauses": [...]}]}

Paragraphs, one JSON object per line:
"#;

/// One paragraph as the model sees it.
pub struct PromptParagraph<'a> {
    pub id: String,
    pub heading_path: &'a [String],
    pub text: &'a str,
}

/// A clause as the model proposed it, before validation. This is what the cache keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawClause {
    pub text: String,
    pub rung: Rung,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub check: Option<RawCheck>,
    pub fix: String,
}

/// A proposed check: a library rule name and whichever params it takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawCheck {
    pub rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bad: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub good: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forbid: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
}

/// Splits paragraphs into model calls of at most `BATCH_SIZE`.
pub fn batches<T>(items: &[T]) -> std::slice::Chunks<'_, T> {
    items.chunks(BATCH_SIZE)
}

/// The prompt for one batch: the S0a text, then one JSON object per paragraph.
pub fn build(paragraphs: &[PromptParagraph]) -> String {
    let mut prompt = EXTRACTION_PROMPT.to_string();
    for para in paragraphs {
        let line = json!({
            "id": para.id,
            "heading": para.heading_path.join(" > "),
            "text": para.text,
        });
        prompt.push_str(&line.to_string());
        prompt.push('\n');
    }
    prompt
}

/// The JSON schema `claude -p --json-schema` holds the answer to. Check params stay loose here;
/// validation decides whether a check is usable.
pub fn schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let check = json!({
        "type": "object",
        "properties": {
            "rule": {
                "type": "string",
                "enum": ["forbidden-pattern", "import-boundary", "protected-paths", "file-naming"]
            },
            "pattern": { "type": "string" },
            "bad": { "type": "string" },
            "good": { "type": "string" },
            "forbid": strings,
            "case": { "type": "string", "enum": ["kebab", "camel", "pascal", "snake"] },
            "suffix": { "type": "string" }
        },
        "required": ["rule"]
    });
    let clause = json!({
        "type": "object",
        "properties": {
            "text": { "type": "string" },
            "rung": { "type": "string", "enum": ["blocking", "judged", "context"] },
            "scope": strings,
            "check": { "anyOf": [{ "type": "null" }, check] },
            "fix": { "type": "string" }
        },
        "required": ["text", "rung", "scope", "check", "fix"]
    });
    json!({
        "type": "object",
        "properties": {
            "paragraphs": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "clauses": { "type": "array", "items": clause }
                    },
                    "required": ["id", "clauses"]
                }
            }
        },
        "required": ["paragraphs"]
    })
}

/// The clauses per paragraph id in a model answer. A paragraph missing from the answer is
/// missing here too (it failed and is retried next run), and so is one that had clauses none of
/// which reads: caching it as "no clauses" would never ask again. Otherwise a clause that does
/// not read is dropped.
pub fn parse_answer(answer: &Value) -> BTreeMap<String, Vec<RawClause>> {
    let mut found = BTreeMap::new();
    let Some(paragraphs) = answer["paragraphs"].as_array() else {
        return found;
    };
    for para in paragraphs {
        let (Some(id), Some(clauses)) = (para["id"].as_str(), para["clauses"].as_array()) else {
            continue;
        };
        let read: Vec<RawClause> = clauses
            .iter()
            .filter_map(|clause| serde_json::from_value::<RawClause>(clause.clone()).ok())
            .filter(|clause| !clause.text.trim().is_empty())
            .collect();
        if read.is_empty() && !clauses.is_empty() {
            continue;
        }
        found.insert(id.to_string(), read);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_s0a_verbatim_followed_by_one_paragraph_per_line() {
        let heading = vec!["Code Style".to_string(), "Imports".to_string()];
        let prompt = build(&[
            PromptParagraph {
                id: "p0".into(),
                heading_path: &heading,
                text: "Use top-level `import` statements; do not use `require()`.",
            },
            PromptParagraph {
                id: "p1".into(),
                heading_path: &[],
                text: "Line one\nline \"two\"",
            },
        ]);
        assert!(prompt.starts_with(
            "You turn a team's engineering docs into the specific things they ask for.\n"
        ));
        let lines: Vec<&str> = prompt.lines().collect();
        let tail = &lines[lines.len() - 3..];
        assert_eq!(tail[0], "Paragraphs, one JSON object per line:");
        let first: Value = serde_json::from_str(tail[1]).unwrap();
        assert_eq!(first["id"], "p0");
        assert_eq!(first["heading"], "Code Style > Imports");
        let second: Value = serde_json::from_str(tail[2]).unwrap();
        assert_eq!(second["text"], "Line one\nline \"two\"");
        assert!(tail[1].starts_with(r#"{"id":"p0""#));
    }

    #[test]
    fn a_paragraph_whose_clauses_all_fail_to_read_is_not_an_answer() {
        let good = json!({ "text": "Do it.", "rung": "judged", "scope": [], "check": null,
                           "fix": "Do it." });
        let answer = json!({ "paragraphs": [
            { "id": "p0", "clauses": [{ "text": "Missing everything else." }] },
            { "id": "p1", "clauses": [] },
            { "id": "p2", "clauses": [good, { "rung": 7 }] },
            { "id": "p3", "clauses": [{ "text": " ", "rung": "judged", "scope": [],
                                        "check": null, "fix": "x" }] },
        ]});
        let found = parse_answer(&answer);
        // Left out, like a paragraph missing from the answer: it fails and is retried.
        assert!(!found.contains_key("p0"));
        assert!(!found.contains_key("p3"));
        assert_eq!(found["p1"].len(), 0);
        assert_eq!(found["p2"].len(), 1);
    }

    #[test]
    fn batches_hold_at_most_ten_paragraphs() {
        let items: Vec<u32> = (0..23).collect();
        let sizes: Vec<usize> = batches(&items).map(<[u32]>::len).collect();
        assert_eq!(sizes, vec![10, 10, 3]);
    }

    #[test]
    fn the_schema_requires_every_clause_field() {
        let schema = schema();
        let clause = &schema["properties"]["paragraphs"]["items"]["properties"]["clauses"]["items"];
        assert_eq!(
            clause["required"],
            json!(["text", "rung", "scope", "check", "fix"])
        );
        assert_eq!(
            clause["properties"]["rung"]["enum"],
            json!(["blocking", "judged", "context"])
        );
    }

    #[test]
    fn reads_clauses_per_paragraph_in_the_shapes_s0a_saw() {
        // Shapes from S0a: a forbidden pattern, a protected path, a judged clause with a null
        // check, a link-only item with no clauses, and a paragraph left out of the answer.
        let answer = json!({ "paragraphs": [
            { "id": "p0", "clauses": [
                { "text": "Do not use require().", "rung": "blocking", "scope": ["**/*.ts"],
                  "check": { "rule": "forbidden-pattern", "pattern": "require($$$A)",
                             "bad": "const a = require('a');", "good": "import a from 'a';" },
                  "fix": "Use a top-level import." },
                { "text": "Use top-level import statements.", "rung": "judged", "scope": [],
                  "check": null, "fix": "Move the import to the top of the file." }
            ]},
            { "id": "p1", "clauses": [
                { "text": "Do not hand-edit numbered Drizzle migrations.", "rung": "blocking",
                  "scope": ["drizzle/**"], "check": { "rule": "protected-paths" },
                  "fix": "Run pnpm run db:generate." }
            ]},
            { "id": "p2", "clauses": [] },
            { "id": "p3", "clauses": [
                { "text": "", "rung": "context", "scope": [], "check": null, "fix": "" },
                { "text": "Unknown rung.", "rung": "sometimes", "scope": [], "check": null, "fix": "x" }
            ]}
        ]});
        let found = parse_answer(&answer);
        // p3's clauses all fail to read: it is left out, so it fails and is asked again.
        assert_eq!(found.keys().collect::<Vec<_>>(), vec!["p0", "p1", "p2"]);
        assert_eq!(found["p0"].len(), 2);
        let check = found["p0"][0].check.as_ref().unwrap();
        assert_eq!(check.rule, "forbidden-pattern");
        assert_eq!(check.pattern.as_deref(), Some("require($$$A)"));
        assert_eq!(found["p0"][1].rung, Rung::Judged);
        assert_eq!(found["p0"][1].check, None);
        assert_eq!(found["p1"][0].scope, vec!["drizzle/**"]);
        assert!(found["p2"].is_empty());
        assert!(!found.contains_key("p4"));
        assert!(parse_answer(&json!({ "nope": [] })).is_empty());
    }

    #[test]
    fn raw_clauses_round_trip_for_the_cache() {
        let clause = RawClause {
            text: "Name files in kebab-case.".into(),
            rung: Rung::Blocking,
            scope: vec!["src/**".into()],
            check: Some(RawCheck {
                rule: "file-naming".into(),
                pattern: None,
                bad: None,
                good: None,
                forbid: None,
                case: Some("kebab".into()),
                suffix: None,
            }),
            fix: "Rename the file.".into(),
        };
        let json = serde_json::to_value(&clause).unwrap();
        assert_eq!(
            json["check"],
            json!({ "rule": "file-naming", "case": "kebab" })
        );
        assert_eq!(serde_json::from_value::<RawClause>(json).unwrap(), clause);
    }
}
