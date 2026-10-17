//! Runs content rules over one file's text: a word prefilter, then at most one parse and one
//! combined pass for every pattern and import call. Import statements are read from the top
//! level only, where they always are, so no rule walks the whole tree.

use ast_grep_core::Pattern;
use ast_grep_core::matcher::MatcherExt;
use ast_grep_core::ops::Any;
use ast_grep_core::tree_sitter::LanguageExt;

use crate::hashing::normalize_ws;
use crate::lang::CodeLang;
use crate::lock::Scope;
use crate::paths::join_relative;

/// One content rule, ready to run. `index` is the caller's handle for the rule.
pub enum ContentRule<'a> {
    Pattern {
        index: usize,
        pattern: &'a str,
        words: &'a [String],
    },
    Imports {
        index: usize,
        forbid: &'a Scope,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMatch {
    pub index: usize,
    /// 1-based.
    pub line: u32,
    /// The matched code, whitespace-normalized; for imports, the module specifier.
    pub matched: String,
    /// Byte range of the matched node.
    pub span: (usize, usize),
}

/// What one parse of a file produced: the matches, and the byte ranges of its top-level
/// statements (empty when the prefilter ruled out every rule and nothing was parsed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    pub matches: Vec<RawMatch>,
    pub top_level: Vec<(usize, usize)>,
}

/// Calls that load a module, checked by import boundaries.
const IMPORT_CALLS: [&str; 2] = ["require($$$ARGS)", "import($$$ARGS)"];

/// The literal identifiers in a pattern. Every one must appear in a text for the pattern to
/// match there, so a text missing any of them needs no parse. Metavariables (`$A`, `$$$A`) are
/// not literals.
pub fn pattern_words(pattern: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut after_dollar = false;
    for ch in pattern.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            current.push(ch);
            continue;
        }
        if !after_dollar && current.len() >= 3 && !current.starts_with(|c: char| c.is_ascii_digit())
        {
            words.push(std::mem::take(&mut current));
        }
        current.clear();
        after_dollar = ch == '$';
    }
    words.sort();
    words.dedup();
    words
}

/// Why `pattern` does not compile for `lang`, or `None` when it does.
pub fn compile_error(pattern: &str, lang: CodeLang) -> Option<String> {
    Pattern::try_new(pattern, lang.support())
        .err()
        .map(|error| error.to_string())
}

pub fn run(lang: CodeLang, rel_path: &str, text: &str, rules: &[ContentRule]) -> Vec<RawMatch> {
    scan(lang, rel_path, text, rules).matches
}

pub fn scan(lang: CodeLang, rel_path: &str, text: &str, rules: &[ContentRule]) -> Scan {
    let live: Vec<&ContentRule> = rules
        .iter()
        .filter(|rule| match rule {
            ContentRule::Pattern { words, .. } => {
                words.iter().all(|word| text.contains(word.as_str()))
            }
            ContentRule::Imports { .. } => true,
        })
        .collect();
    if live.is_empty() {
        return Scan::default();
    }
    let support = lang.support();
    let root = support.ast_grep(text);
    let program = root.root();
    let top_level: Vec<(usize, usize)> = program
        .children()
        .map(|node| (node.range().start, node.range().end))
        .collect();

    let mut patterns: Vec<(usize, Pattern)> = live
        .iter()
        .filter_map(|rule| match rule {
            ContentRule::Pattern { index, pattern, .. } => Pattern::try_new(pattern, support)
                .ok()
                .map(|compiled| (*index, compiled)),
            ContentRule::Imports { .. } => None,
        })
        .collect();
    let imports: Vec<(usize, &Scope)> = live
        .iter()
        .filter_map(|rule| match rule {
            ContentRule::Imports { index, forbid } => Some((*index, *forbid)),
            ContentRule::Pattern { .. } => None,
        })
        .collect();
    // Import calls ride along in the same pass, tagged with an index no rule uses.
    let call_tag = usize::MAX;
    if !imports.is_empty() {
        for call in IMPORT_CALLS {
            if let Ok(compiled) = Pattern::try_new(call, support) {
                patterns.push((call_tag, compiled));
            }
        }
    }

    let mut out = Vec::new();
    let mut specifiers = Vec::new();
    if !patterns.is_empty() {
        let any = Any::new(
            patterns
                .iter()
                .map(|(_, pattern)| pattern.clone())
                .collect::<Vec<_>>(),
        );
        for found in program.find_all(&any) {
            let node = found.get_node().clone();
            for (index, pattern) in &patterns {
                if pattern.match_node(node.clone()).is_none() {
                    continue;
                }
                if *index == call_tag {
                    let argument = node
                        .field("arguments")
                        .and_then(|args| args.children().find(|arg| arg.kind() == "string"));
                    if let Some(argument) = argument {
                        specifiers.push(argument);
                    }
                    continue;
                }
                out.push(RawMatch {
                    index: *index,
                    line: node.start_pos().line() as u32 + 1,
                    matched: normalize_ws(&node.text()),
                    span: (node.range().start, node.range().end),
                });
            }
        }
    }

    if !imports.is_empty() {
        for statement in program.children() {
            let kind = statement.kind();
            if kind == "import_statement" || kind == "export_statement" {
                specifiers.extend(statement.field("source"));
            }
        }
        for source in specifiers {
            let specifier = unquote(&source.text());
            let resolved = resolve_relative(rel_path, &specifier);
            for (index, forbid) in &imports {
                let hit = forbid.matches(&specifier)
                    || resolved.as_deref().is_some_and(|path| forbid.matches(path));
                if hit {
                    out.push(RawMatch {
                        index: *index,
                        line: source.start_pos().line() as u32 + 1,
                        matched: specifier.clone(),
                        span: (source.range().start, source.range().end),
                    });
                }
            }
        }
    }
    out.sort_by_key(|found| (found.span.0, found.index));
    Scan {
        matches: out,
        top_level,
    }
}

fn unquote(text: &str) -> String {
    text.trim_matches(|c| c == '\'' || c == '"' || c == '`')
        .to_string()
}

/// A relative specifier (`./x`, `../x`) as a repo-relative path, so boundaries can name
/// directories.
fn resolve_relative(rel_path: &str, specifier: &str) -> Option<String> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }
    let dir = rel_path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    join_relative(dir, specifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern_rule<'a>(index: usize, pattern: &'a str, words: &'a [String]) -> ContentRule<'a> {
        ContentRule::Pattern {
            index,
            pattern,
            words,
        }
    }

    #[test]
    fn pattern_words_skips_metavariables_and_short_tokens() {
        assert_eq!(
            pattern_words("asProvisioned($$$ARGS)!"),
            vec!["asProvisioned"]
        );
        assert_eq!(pattern_words("$OBJ.taskManager"), vec!["taskManager"]);
        assert_eq!(
            pattern_words("export { $$$NAMES } from $SOURCE"),
            vec!["export", "from"]
        );
        assert!(pattern_words("$A!").is_empty());
    }

    #[test]
    fn finds_patterns_in_one_pass_and_attributes_them() {
        let text = "const a = asProvisioned(t)!.id;\nconst b = require('x');\n";
        let w1 = pattern_words("asProvisioned($$$A)!");
        let w2 = pattern_words("require($$$A)");
        let rules = [
            pattern_rule(0, "asProvisioned($$$A)!", &w1),
            pattern_rule(1, "require($$$A)", &w2),
        ];
        let found = run(CodeLang::TypeScript, "src/a.ts", text, &rules);
        let summary: Vec<(usize, u32, &str)> = found
            .iter()
            .map(|m| (m.index, m.line, m.matched.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![(0, 1, "asProvisioned(t)!"), (1, 2, "require('x')")]
        );
        assert_eq!(&text[found[0].span.0..found[0].span.1], "asProvisioned(t)!");
    }

    #[test]
    fn ignores_matches_inside_strings_and_comments() {
        let text = "// require() is not a probe\nconst s = \"node -p require('x')\";\n";
        let words = pattern_words("require($$$A)");
        let found = run(
            CodeLang::TypeScript,
            "a.ts",
            text,
            &[pattern_rule(0, "require($$$A)", &words)],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn checks_imports_by_specifier_and_by_resolved_path() {
        let text = "import { a } from '@renderer/x';\nexport { b } from '../renderer/y';\nconst c = await import('ok');\n";
        let forbid = Scope::new(&["@renderer/**".into(), "src/renderer/**".into()]).unwrap();
        let rules = [ContentRule::Imports {
            index: 7,
            forbid: &forbid,
        }];
        let found = run(CodeLang::TypeScript, "src/main/a.ts", text, &rules);
        let specifiers: Vec<&str> = found.iter().map(|m| m.matched.as_str()).collect();
        assert_eq!(specifiers, vec!["@renderer/x", "../renderer/y"]);
        assert_eq!(found[1].line, 2);
    }

    #[test]
    fn checks_require_and_dynamic_import_calls_anywhere() {
        let text = "function load() {\n  const a = require('@renderer/a');\n  return import('@renderer/b');\n}\n";
        let forbid = Scope::new(&["@renderer/**".into()]).unwrap();
        let rules = [ContentRule::Imports {
            index: 0,
            forbid: &forbid,
        }];
        let scan = scan(CodeLang::TypeScript, "src/main/a.ts", text, &rules);
        let specifiers: Vec<&str> = scan.matches.iter().map(|m| m.matched.as_str()).collect();
        assert_eq!(specifiers, vec!["@renderer/a", "@renderer/b"]);
        assert_eq!(scan.top_level, vec![(0, text.len() - 1)]);
    }

    #[test]
    fn matches_in_tsx_and_javascript() {
        let words = pattern_words("asProvisioned($$$A)!");
        let rule = [pattern_rule(0, "asProvisioned($$$A)!", &words)];
        let tsx = "const x = <A b={asProvisioned(t)!.id} />;\n";
        assert_eq!(run(CodeLang::Tsx, "a.tsx", tsx, &rule).len(), 1);
        let rw = pattern_words("require($$$A)");
        let js = [pattern_rule(0, "require($$$A)", &rw)];
        assert_eq!(
            run(
                CodeLang::JavaScript,
                "a.js",
                "const m = require('a');\n",
                &js
            )
            .len(),
            1
        );
    }

    #[test]
    fn survives_code_that_does_not_parse() {
        let words = pattern_words("asProvisioned($$$A)!");
        let rule = [pattern_rule(0, "asProvisioned($$$A)!", &words)];
        let broken = "function ( {{{ asProvisioned(t)!.id\n";
        let _ = run(CodeLang::TypeScript, "a.ts", broken, &rule);
    }
}
