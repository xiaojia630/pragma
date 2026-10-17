//! Splits a markdown doc into paragraphs: the unit pragma extracts clauses from and anchors to.
//! A paragraph is a prose paragraph, a top-level list item with its sub-items, or a table. A code
//! block attaches to the paragraph before it under the same heading.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};

use crate::hashing::{normalize_ws, short_hash};

/// Where a clause came from, precise enough to find the paragraph again after edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub path: String,
    pub heading_path: Vec<String>,
    pub para_hash: String,
    /// First and last line, 1-based.
    pub lines: [u32; 2],
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    pub heading_path: Vec<String>,
    /// Byte range in the doc.
    pub start: usize,
    pub end: usize,
    /// First and last line, 1-based.
    pub lines: [u32; 2],
    pub hash: String,
}

/// Maps byte offsets to 1-based line numbers.
pub struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(at, _)| at + 1));
        LineIndex { starts }
    }

    pub fn line_of(&self, offset: usize) -> u32 {
        self.starts.partition_point(|&start| start <= offset) as u32
    }
}

pub fn paragraphs(doc: &str) -> Vec<Paragraph> {
    let mut units: Vec<(Vec<String>, usize, usize)> = Vec::new();
    let mut headings: Vec<(u8, String)> = Vec::new();
    let mut heading: Option<(u8, String)> = None;
    let mut list_depth = 0usize;
    for (event, range) in Parser::new_ext(doc, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some((level as u8, String::new()))
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, text)) = heading.take() {
                    headings.retain(|(open, _)| *open < level);
                    headings.push((level, text.trim().to_string()));
                }
            }
            Event::Text(text) | Event::Code(text) if heading.is_some() => {
                if let Some((_, buffer)) = heading.as_mut() {
                    buffer.push_str(&text);
                }
            }
            Event::Start(Tag::List(_)) => list_depth += 1,
            Event::End(TagEnd::List(_)) => list_depth -= 1,
            Event::Start(Tag::Item) if list_depth == 1 => {
                units.push((heading_path(&headings), range.start, range.end));
            }
            Event::Start(Tag::Paragraph | Tag::Table(_)) if list_depth == 0 => {
                units.push((heading_path(&headings), range.start, range.end));
            }
            Event::Start(Tag::CodeBlock(_)) if list_depth == 0 => {
                let current = heading_path(&headings);
                if let Some(last) = units.last_mut().filter(|last| last.0 == current) {
                    last.2 = range.end;
                }
            }
            _ => {}
        }
    }
    let lines = LineIndex::new(doc);
    units
        .into_iter()
        .map(|(heading_path, start, end)| Paragraph {
            heading_path,
            start,
            end,
            lines: [
                lines.line_of(start),
                lines.line_of(end.saturating_sub(1).max(start)),
            ],
            hash: short_hash(normalize_ws(&doc[start..end]).as_bytes()),
        })
        .collect()
}

fn heading_path(headings: &[(u8, String)]) -> Vec<String> {
    headings.iter().map(|(_, text)| text.clone()).collect()
}

/// The paragraph that contains 1-based `line`.
pub fn paragraph_at_line(doc: &str, line: u32) -> Option<Paragraph> {
    paragraphs(doc)
        .into_iter()
        .find(|para| para.lines[0] <= line && line <= para.lines[1])
}

pub fn anchor_for(doc_path: &str, doc: &str, para: &Paragraph) -> Anchor {
    Anchor {
        path: doc_path.to_string(),
        heading_path: para.heading_path.clone(),
        para_hash: para.hash.clone(),
        lines: para.lines,
        quote: normalize_ws(&doc[para.start..para.end])
            .chars()
            .take(120)
            .collect(),
    }
}

/// A clause's id: stable while its paragraph is unchanged.
pub fn clause_id(doc_path: &str, para_hash: &str, ordinal: u32) -> String {
    short_hash(format!("{doc_path}\n{para_hash}\n{ordinal}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\ntitle: x\n---\n# Guide\n\nIntro paragraph.\n\n## Rules\n\n- Never do A.\n  - Not even a little.\n- Use `b()`.\n\n```ts\nb();\n```\n\nAfter the list.\n\n```ts\nexample();\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

    #[test]
    fn splits_paragraphs_items_and_tables_with_heading_paths() {
        let paras = paragraphs(DOC);
        let texts: Vec<&str> = paras.iter().map(|p| DOC[p.start..p.end].trim()).collect();
        assert_eq!(texts[0], "Intro paragraph.");
        assert_eq!(texts[1], "- Never do A.\n  - Not even a little.");
        assert!(texts[2].starts_with("- Use `b()`."));
        assert!(texts[3].starts_with("After the list.") && texts[3].ends_with("```"));
        assert!(texts[4].starts_with("| a | b |"));
        assert_eq!(paras.len(), 5);
        assert_eq!(paras[0].heading_path, vec!["Guide"]);
        assert_eq!(paras[1].heading_path, vec!["Guide", "Rules"]);
    }

    #[test]
    fn lines_are_one_based_and_inclusive() {
        let paras = paragraphs(DOC);
        assert_eq!(paras[0].lines, [6, 6]);
        assert_eq!(paras[1].lines, [10, 11]);
    }

    #[test]
    fn hash_ignores_reformatting_but_not_wording() {
        let a = paragraphs("Use   the\nstore.\n")[0].hash.clone();
        let b = paragraphs("Use the store.\n")[0].hash.clone();
        let c = paragraphs("Use the stores.\n")[0].hash.clone();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn finds_the_paragraph_at_a_line_and_anchors_it() {
        let para = paragraph_at_line(DOC, 11).unwrap();
        let anchor = anchor_for("AGENTS.md", DOC, &para);
        assert_eq!(anchor.quote, "- Never do A. - Not even a little.");
        assert_eq!(anchor.lines, [10, 11]);
        assert_eq!(anchor.heading_path, vec!["Guide", "Rules"]);
        assert_ne!(
            clause_id("AGENTS.md", &anchor.para_hash, 0),
            clause_id("AGENTS.md", &anchor.para_hash, 1)
        );
    }
}
