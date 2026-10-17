//! Which findings an edit introduced. Newness is decided by counts per (clause, matched text),
//! never by line numbers, so moving code around is never a new violation.

use std::collections::{HashMap, HashSet};

use crate::rules::{CompiledLock, Finding};

/// The byte range in `after` that differs from `before`: everything between their common prefix
/// and common suffix, on character boundaries.
pub fn changed_range(before: &str, after: &str) -> (usize, usize) {
    let (old, new) = (before.as_bytes(), after.as_bytes());
    let mut prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    while !after.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let room = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    while !after.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    (prefix, new.len() - suffix)
}

/// The content findings an edit introduces in one file. The new version is parsed once. When a
/// finding touches what changed, the old and new versions of just the top-level statements the
/// edit touched are compared, so a large file is never parsed twice.
pub fn introduced_by_edit(
    lock: &CompiledLock,
    rel_path: &str,
    before: &str,
    after: &str,
) -> Vec<Finding> {
    let scan = lock.content_scan(rel_path, after);
    let (start, end) = changed_range(before, after);
    let touches = |(from, to): (usize, usize)| from <= end && start <= to;
    if !scan.findings.iter().any(|(_, span)| touches(*span)) {
        return Vec::new();
    }
    let touched: Vec<&(usize, usize)> = scan
        .top_level
        .iter()
        .filter(|span| touches(**span))
        .collect();
    let window_start = touched.first().map_or(0, |span| span.0.min(start));
    let window_end = touched.last().map_or(after.len(), |span| span.1.max(end));
    let before_end = (window_end + before.len()).checked_sub(after.len());
    let old_window = before_end.and_then(|before_end| before.get(window_start..before_end));
    let Some(old_window) = old_window else {
        let all_after: Vec<Finding> = scan
            .findings
            .into_iter()
            .map(|(finding, _)| finding)
            .collect();
        return introduced(
            &lock.content_findings(rel_path, before),
            &all_after,
            before,
            after,
        );
    };
    let new_in_window: Vec<Finding> = scan
        .findings
        .into_iter()
        .filter(|(_, (from, to))| *from >= window_start && *to <= window_end)
        .map(|(finding, _)| finding)
        .collect();
    let old_in_window = lock.content_findings(rel_path, old_window);
    introduced(&old_in_window, &new_in_window, old_window, after)
}

pub fn introduced(
    before: &[Finding],
    after: &[Finding],
    before_text: &str,
    after_text: &str,
) -> Vec<Finding> {
    let key = |finding: &Finding| (finding.clause_id.clone(), finding.matched.clone());
    let mut before_counts: HashMap<(String, String), usize> = HashMap::new();
    for finding in before {
        *before_counts.entry(key(finding)).or_default() += 1;
    }
    let mut groups: Vec<((String, String), Vec<&Finding>)> = Vec::new();
    for finding in after {
        let finding_key = key(finding);
        match groups
            .iter_mut()
            .find(|(group_key, _)| *group_key == finding_key)
        {
            Some((_, members)) => members.push(finding),
            None => groups.push((finding_key, vec![finding])),
        }
    }
    let old_lines: HashSet<&str> = before_text.lines().map(str::trim).collect();
    let new_lines: Vec<&str> = after_text.lines().collect();
    let line_is_new = |finding: &Finding| {
        let text = new_lines
            .get(finding.line.saturating_sub(1) as usize)
            .map(|line| line.trim());
        !old_lines.contains(text.unwrap_or(""))
    };
    let mut out = Vec::new();
    for (group_key, members) in groups {
        let surplus = members
            .len()
            .saturating_sub(before_counts.get(&group_key).copied().unwrap_or(0));
        if surplus == 0 {
            continue;
        }
        // Point at lines the edit added when possible, so the message shows the edit itself.
        let mut chosen: Vec<&Finding> = members
            .iter()
            .copied()
            .filter(|f| line_is_new(f))
            .take(surplus)
            .collect();
        for finding in members.iter().rev() {
            if chosen.len() >= surplus {
                break;
            }
            if !chosen.contains(finding) {
                chosen.push(finding);
            }
        }
        out.extend(chosen.into_iter().cloned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(clause: &str, line: u32, matched: &str) -> Finding {
        Finding {
            clause_id: clause.into(),
            file: "a.ts".into(),
            line,
            matched: matched.into(),
        }
    }

    #[test]
    fn existing_violations_that_moved_are_not_new() {
        let before = vec![finding("c", 1, "x!")];
        let after = vec![finding("c", 40, "x!")];
        assert!(introduced(&before, &after, "x!\n", "\n".repeat(39).as_str()).is_empty());
    }

    #[test]
    fn a_second_copy_of_an_existing_violation_is_new() {
        let before_text = "a = x!;\n";
        let after_text = "a = x!;\nb = x!;\n";
        let before = vec![finding("c", 1, "x!")];
        let after = vec![finding("c", 1, "x!"), finding("c", 2, "x!")];
        assert_eq!(
            introduced(&before, &after, before_text, after_text),
            vec![finding("c", 2, "x!")]
        );
    }

    #[test]
    fn prefers_the_added_line_even_when_it_comes_first() {
        let before_text = "keep = x!;\n";
        let after_text = "added = x!;\nkeep = x!;\n";
        let before = vec![finding("c", 1, "x!")];
        let after = vec![finding("c", 1, "x!"), finding("c", 2, "x!")];
        assert_eq!(
            introduced(&before, &after, before_text, after_text),
            vec![finding("c", 1, "x!")]
        );
    }

    #[test]
    fn different_clauses_and_texts_are_counted_apart() {
        let after = vec![
            finding("c", 1, "x!"),
            finding("d", 1, "x!"),
            finding("c", 2, "y!"),
        ];
        assert_eq!(introduced(&[], &after, "", "x!\ny!\n").len(), 3);
    }

    mod by_edit {
        use super::super::*;
        use crate::lock::{Check, Examples, LibraryRule, Lock, LockedClause};
        use crate::markdown::Anchor;

        fn lock() -> CompiledLock {
            let clause = LockedClause {
                id: "bang".into(),
                anchor: Anchor {
                    path: "AGENTS.md".into(),
                    heading_path: vec![],
                    para_hash: "p".into(),
                    lines: [1, 1],
                    quote: String::new(),
                },
                text: "No bang.".into(),
                scope: vec![],
                check: Check::Library(LibraryRule::ForbiddenPattern {
                    pattern: "asProvisioned($$$A)!".into(),
                    examples: Examples {
                        bad: "asProvisioned(t)!".into(),
                        good: "asProvisioned(t)".into(),
                    },
                }),
                fix: String::new(),
                origin: None,
                frontmatter: None,
            };
            CompiledLock::compile(&Lock {
                format: 1,
                paragraphs: vec![],
                clauses: vec![clause],
            })
            .unwrap()
        }

        fn file(functions: &[&str]) -> String {
            functions
                .iter()
                .enumerate()
                .map(|(i, body)| format!("export function f{i}(t: T) {{\n  return {body};\n}}\n"))
                .collect()
        }

        #[test]
        fn changed_range_is_the_middle_on_char_boundaries() {
            assert_eq!(changed_range("abcXdef", "abcYYdef"), (3, 5));
            assert_eq!(changed_range("same", "same"), (4, 4));
            assert_eq!(changed_range("café", "cafè"), (3, 5));
            assert_eq!(changed_range("aa", "aaa"), (2, 3));
        }

        #[test]
        fn an_edit_far_from_existing_violations_introduces_nothing() {
            let before = file(&["asProvisioned(t)!.id", "1", "2"]);
            let after = before.replace("return 2;", "return 3;");
            assert!(introduced_by_edit(&lock(), "a.ts", &before, &after).is_empty());
        }

        #[test]
        fn a_new_violation_next_to_an_old_one_is_found() {
            let before = file(&["asProvisioned(t)!.id", "1"]);
            let after = before.replace("return 1;", "return asProvisioned(t)!.id;");
            let found = introduced_by_edit(&lock(), "a.ts", &before, &after);
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].line, 5);
        }

        #[test]
        fn moving_a_violation_within_the_edit_is_not_new() {
            let before =
                "export function f(t: T) {\n  const a = asProvisioned(t)!.id;\n  const b = 1;\n}\n";
            let after =
                "export function f(t: T) {\n  const b = 1;\n  const a = asProvisioned(t)!.id;\n}\n";
            assert!(introduced_by_edit(&lock(), "a.ts", before, after).is_empty());
        }

        #[test]
        fn an_edit_inside_a_multi_line_match_is_caught() {
            let before = "const id = asProvisioned(\n  task,\n).id;\n";
            let after = "const id = asProvisioned(\n  task,\n)!.id;\n";
            assert_eq!(introduced_by_edit(&lock(), "a.ts", before, after).len(), 1);
        }

        #[test]
        fn edits_in_two_places_are_compared_together() {
            let before = file(&["asProvisioned(t)!.id", "1", "2"]);
            let after = before
                .replace("return asProvisioned(t)!.id;", "return 0;")
                .replace("return 2;", "return asProvisioned(t)!.id;");
            assert!(introduced_by_edit(&lock(), "a.ts", &before, &after).is_empty());
        }
    }
}
