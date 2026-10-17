//! References a doc makes to the repo: inline code spans, inline link targets, and Claude Code
//! `@path` imports. Fenced code blocks are skipped, because they hold examples, not claims about
//! the repo.

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

use crate::markdown::LineIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefSource {
    CodeSpan,
    Link,
    Import,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRef {
    pub source: RefSource,
    /// The reference as written, without backticks or link syntax.
    pub text: String,
    /// Byte range of `text` in the doc.
    pub start: usize,
    pub end: usize,
    /// 1-based.
    pub line: u32,
}

pub fn extract(doc: &str) -> Vec<RawRef> {
    let lines = LineIndex::new(doc);
    let mut out = Vec::new();
    let mut in_code_block = 0usize;
    let mut push = |source: RefSource, text: &str, start: usize| {
        out.push(RawRef {
            source,
            text: text.to_string(),
            start,
            end: start + text.len(),
            line: lines.line_of(start),
        });
    };
    for (event, range) in Parser::new_ext(doc, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block += 1,
            Event::End(TagEnd::CodeBlock) => in_code_block -= 1,
            _ if in_code_block > 0 => {}
            Event::Code(code) => {
                if let Some(at) = doc[range.clone()].find(code.as_ref()) {
                    push(RefSource::CodeSpan, &code, range.start + at);
                }
            }
            Event::Start(Tag::Link {
                link_type: LinkType::Inline,
                dest_url,
                ..
            }) => {
                let source = &doc[range.clone()];
                let after_text = source.rfind("](").map(|at| at + 2).unwrap_or(0);
                if let Some(at) = source[after_text..].find(dest_url.as_ref()) {
                    push(RefSource::Link, &dest_url, range.start + after_text + at);
                }
            }
            Event::Text(_) => {
                let slice = &doc[range.clone()];
                let starts_at_boundary = doc[..range.start]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace);
                for (at, token) in whitespace_tokens(slice) {
                    if at == 0 && !starts_at_boundary {
                        continue;
                    }
                    let path = token.trim_end_matches(['.', ',', ';', ':', ')']);
                    let Some(path) = path.strip_prefix('@').filter(|path| path.ends_with(".md"))
                    else {
                        continue;
                    };
                    push(RefSource::Import, path, range.start + at + 1);
                }
            }
            _ => {}
        }
    }
    out
}

/// Whitespace-separated tokens of `text`, each with its byte offset within `text`.
fn whitespace_tokens(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(at),
            (true, Some(s)) => {
                out.push((s, &text[s..at]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, &text[s..]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_imports_get_their_own_ranges_and_glued_ones_are_skipped() {
        let doc = "See @a.md and also @a.md, not x@b.md\n";
        let refs = extract(doc);
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert!(refs.iter().all(|r| r.source == RefSource::Import));
        assert_ne!(refs[0].start, refs[1].start);
        for r in &refs {
            assert_eq!(&doc[r.start..r.end], r.text);
            assert_eq!(r.text, "a.md");
        }
    }

    #[test]
    fn finds_code_spans_links_and_imports_with_positions() {
        let doc = "# T\n\nSee `src/a.ts` and [the guide](docs/guide.md#setup).\n\n@AGENTS.md\n\n```sh\n`not/this.ts`\n```\n";
        let refs = extract(doc);
        let summary: Vec<(RefSource, &str, u32)> = refs
            .iter()
            .map(|r| (r.source, r.text.as_str(), r.line))
            .collect();
        assert_eq!(
            summary,
            vec![
                (RefSource::CodeSpan, "src/a.ts", 3),
                (RefSource::Link, "docs/guide.md#setup", 3),
                (RefSource::Import, "AGENTS.md", 5),
            ]
        );
        for r in &refs {
            assert_eq!(&doc[r.start..r.end], r.text);
        }
    }

    #[test]
    fn skips_reference_style_links_and_autolinks() {
        let refs = extract("[a][x] and <https://example.com>\n\n[x]: docs/a.md\n");
        assert!(refs.is_empty(), "{refs:?}");
    }
}
