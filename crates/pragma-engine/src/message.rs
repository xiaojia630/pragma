//! The text Claude Code hands the agent when pragma blocks an edit. It is review feedback that
//! quotes the doc, never a command to run.

use crate::lock::LockedClause;
use crate::rules::Finding;

pub struct Violation<'a> {
    pub clause: &'a LockedClause,
    pub finding: &'a Finding,
    /// The source line the finding is on.
    pub line_text: &'a str,
}

pub fn block_message(violations: &[Violation]) -> String {
    let mut out = String::from("pragma blocked this edit.\n");
    for violation in violations {
        let anchor = &violation.clause.anchor;
        let heading = anchor
            .heading_path
            .last()
            .map(|heading| format!(", \"{heading}\""))
            .unwrap_or_default();
        out.push_str(&format!(
            "Rule ({}:{}{heading}): {}\n",
            anchor.path, anchor.lines[0], violation.clause.text
        ));
        let finding = violation.finding;
        if finding.line == 0 {
            out.push_str(&format!("Found: {}\n", finding.file));
        } else {
            let text = violation.line_text.trim();
            out.push_str(&format!(
                "Found: {}:{}  {text}\n",
                finding.file, finding.line
            ));
        }
        out.push_str(&format!("Fix: {}\n", violation.clause.fix));
    }
    out.push_str(
        "If the user explicitly asked for this change, stop and tell them pragma blocked it; they can\n\
         pause pragma for this repo with `pragma pause`.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{Check, LibraryRule};
    use crate::markdown::Anchor;

    #[test]
    fn matches_the_spec_format() {
        let clause = LockedClause {
            id: "c".into(),
            anchor: Anchor {
                path: "AGENTS.md".into(),
                heading_path: vec!["Agent Guardrails".into()],
                para_hash: "h".into(),
                lines: [142, 142],
                quote: String::new(),
            },
            text: "never use asProvisioned(...)!; use explicit null checks.".into(),
            scope: vec![],
            check: Check::Library(LibraryRule::ProtectedPaths),
            fix: "const p = asProvisioned(task); if (!p) return; then use p.id.".into(),
            origin: None,
            frontmatter: None,
        };
        let finding = Finding {
            clause_id: "c".into(),
            file: "src/core/features/tasks/browser/task-view.tsx".into(),
            line: 88,
            matched: "asProvisioned(task)!".into(),
        };
        let line_text = "    const id = asProvisioned(task)!.id;";
        let message = block_message(&[Violation {
            clause: &clause,
            finding: &finding,
            line_text,
        }]);
        assert_eq!(
            message,
            "pragma blocked this edit.\n\
             Rule (AGENTS.md:142, \"Agent Guardrails\"): never use asProvisioned(...)!; use explicit null checks.\n\
             Found: src/core/features/tasks/browser/task-view.tsx:88  const id = asProvisioned(task)!.id;\n\
             Fix: const p = asProvisioned(task); if (!p) return; then use p.id.\n\
             If the user explicitly asked for this change, stop and tell them pragma blocked it; they can\n\
             pause pragma for this repo with `pragma pause`.\n"
        );
    }

    #[test]
    fn findings_about_the_file_show_no_line() {
        let clause = LockedClause {
            id: "p".into(),
            anchor: Anchor {
                path: "AGENTS.md".into(),
                heading_path: vec![],
                para_hash: "h".into(),
                lines: [267, 267],
                quote: String::new(),
            },
            text: "Do not hand-edit numbered Drizzle migrations.".into(),
            scope: vec![],
            check: Check::Library(LibraryRule::ProtectedPaths),
            fix: "Run pnpm run db:generate.".into(),
            origin: None,
            frontmatter: None,
        };
        let finding = Finding {
            clause_id: "p".into(),
            file: "drizzle/0001_x.sql".into(),
            line: 0,
            matched: "drizzle/0001_x.sql".into(),
        };
        let violation = Violation {
            clause: &clause,
            finding: &finding,
            line_text: "",
        };
        let message = block_message(&[violation]);
        assert!(
            message.contains("Rule (AGENTS.md:267): Do not hand-edit"),
            "{message}"
        );
        assert!(message.contains("Found: drizzle/0001_x.sql\n"), "{message}");
    }
}
