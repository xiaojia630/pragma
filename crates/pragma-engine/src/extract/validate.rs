//! Validation of what the model proposed (§5.2 step 2): library params against each rule's
//! shape, scope globs by compiling them, and patterns by compiling them and running them on
//! their examples. A check that fails is dropped, and its clause becomes judged.

use crate::lock::{Check, Examples, LibraryRule, NamingCase, Scope};
use crate::rules::pattern_problems;

use super::prompt::RawCheck;

/// The globs that compile, and a problem for each one that does not.
pub fn scope(globs: &[String]) -> (Vec<String>, Vec<String>) {
    let mut valid = Vec::new();
    let mut problems = Vec::new();
    for glob in globs {
        match Scope::new(std::slice::from_ref(glob)) {
            Ok(_) => valid.push(glob.clone()),
            Err(error) => problems.push(format!("scope: {error:#}")),
        }
    }
    (valid, problems)
}

/// The check a proposal stands for, or why it cannot be used. `scope` is the clause's final
/// scope: protected paths and file naming over the whole repo are never taken from a guess.
///
/// A `forbidden-pattern` proposal is a pattern the model wrote, so it becomes a generated
/// `Check::Pattern`, which reaches blocking only through the backtest (S8). The other three
/// rules are library templates whose params the model filled in.
pub fn check(raw: &RawCheck, scope: &[String]) -> Result<Check, String> {
    let includes: Vec<&String> = scope.iter().filter(|glob| !glob.starts_with('!')).collect();
    let rule = raw.rule.as_str();
    let needs_scope = || {
        if includes.is_empty() {
            return Err(format!("{rule} needs a scope; the whole repo is too wide"));
        }
        match includes.iter().find(|glob| !narrows(glob)) {
            Some(glob) => Err(format!(
                "{rule} needs a scope narrower than the whole repo; {glob} matches every file"
            )),
            None => Ok(()),
        }
    };
    match rule {
        "protected-paths" => {
            needs_scope()?;
            Ok(Check::Library(LibraryRule::ProtectedPaths))
        }
        "import-boundary" => {
            let forbid = raw
                .forbid
                .clone()
                .filter(|forbid| !forbid.is_empty())
                .ok_or("import-boundary needs forbid globs")?;
            Scope::new(&forbid).map_err(|error| format!("forbid: {error:#}"))?;
            Ok(Check::Library(LibraryRule::ImportBoundary { forbid }))
        }
        "file-naming" => {
            needs_scope()?;
            let case = match raw.case.as_deref() {
                Some("kebab") => NamingCase::Kebab,
                Some("camel") => NamingCase::Camel,
                Some("pascal") => NamingCase::Pascal,
                Some("snake") => NamingCase::Snake,
                other => return Err(format!("file-naming has no known case: {other:?}")),
            };
            let suffix = raw.suffix.clone().filter(|suffix| !suffix.is_empty());
            Ok(Check::Library(LibraryRule::FileNaming { case, suffix }))
        }
        "forbidden-pattern" => {
            let (Some(pattern), Some(bad), Some(good)) = (&raw.pattern, &raw.bad, &raw.good) else {
                return Err("forbidden-pattern needs a pattern and bad and good examples".into());
            };
            let examples = Examples {
                bad: bad.clone(),
                good: good.clone(),
            };
            let problems = pattern_problems(pattern, &examples);
            if !problems.is_empty() {
                return Err(problems.join("; "));
            }
            Ok(Check::Pattern {
                pattern: pattern.clone(),
                examples,
            })
        }
        other => Err(format!("unknown rule {other:?}")),
    }
}

/// Whether an include glob names something: a literal path segment (`src`, or `drizzle` in
/// `**/drizzle/**`) or a file name a path must have (`*.ts`). `**`, `**/*`, `*` and `*.*` match
/// every file, so they are no scope for a rule that guards paths.
fn narrows(glob: &str) -> bool {
    glob.split('/').any(|segment| {
        let mut in_class = false;
        segment.chars().any(|char| match char {
            '[' => {
                in_class = true;
                false
            }
            ']' => {
                in_class = false;
                false
            }
            _ if in_class => false,
            '*' | '?' | '.' | '{' | '}' | ',' => false,
            _ => true,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(rule: &str) -> RawCheck {
        RawCheck {
            rule: rule.into(),
            pattern: None,
            bad: None,
            good: None,
            forbid: None,
            case: None,
            suffix: None,
        }
    }

    fn globs(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn scope_keeps_globs_that_compile() {
        let (valid, problems) = scope(&globs(&["src/**/*.ts", "[bad", "!**/legacy/**"]));
        assert_eq!(valid, globs(&["src/**/*.ts", "!**/legacy/**"]));
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn protected_paths_needs_an_include_glob() {
        assert_eq!(
            check(&raw("protected-paths"), &globs(&["drizzle/**"])),
            Ok(Check::Library(LibraryRule::ProtectedPaths))
        );
        assert!(check(&raw("protected-paths"), &[]).is_err());
        assert!(check(&raw("protected-paths"), &globs(&["!src/**"])).is_err());
    }

    #[test]
    fn a_glob_that_matches_every_file_is_no_scope() {
        let naming = RawCheck {
            case: Some("kebab".into()),
            ..raw("file-naming")
        };
        for rule in [raw("protected-paths"), naming.clone()] {
            for scope in [
                &["**"][..],
                &["**/*"],
                &["*"],
                &["**/**"],
                &["*/**"],
                &["**/*.*"],
                &["src/**", "**"],
                &["**", "!docs/**"],
            ] {
                let error = check(&rule, &globs(scope)).unwrap_err();
                assert!(error.contains("whole repo"), "{scope:?}: {error}");
            }
            for scope in [
                &["src/**"][..],
                &["**/drizzle/**"],
                &["**/*.ts"],
                &["*.{ts,tsx}"],
                &["src/**", "!src/legacy/**"],
            ] {
                assert!(check(&rule, &globs(scope)).is_ok(), "{scope:?}");
            }
        }
    }

    #[test]
    fn import_boundary_needs_forbid_globs_that_compile() {
        let mut proposal = raw("import-boundary");
        assert!(check(&proposal, &[]).is_err());
        proposal.forbid = Some(vec![]);
        assert!(check(&proposal, &[]).is_err());
        proposal.forbid = Some(globs(&["[bad"]));
        assert!(check(&proposal, &[]).unwrap_err().starts_with("forbid:"));
        proposal.forbid = Some(globs(&["electron", "node:*"]));
        assert_eq!(
            check(&proposal, &globs(&["src/renderer/**"])),
            Ok(Check::Library(LibraryRule::ImportBoundary {
                forbid: globs(&["electron", "node:*"])
            }))
        );
    }

    #[test]
    fn file_naming_needs_a_known_case_and_a_scope() {
        let mut proposal = raw("file-naming");
        assert!(check(&proposal, &globs(&["src/**"])).is_err());
        proposal.case = Some("title".into());
        assert!(check(&proposal, &globs(&["src/**"])).is_err());
        proposal.case = Some("kebab".into());
        assert!(check(&proposal, &[]).is_err());
        proposal.suffix = Some(".test.ts".into());
        assert_eq!(
            check(&proposal, &globs(&["src/**"])),
            Ok(Check::Library(LibraryRule::FileNaming {
                case: NamingCase::Kebab,
                suffix: Some(".test.ts".into())
            }))
        );
    }

    #[test]
    fn a_forbidden_pattern_becomes_a_generated_pattern_when_it_behaves() {
        let mut proposal = raw("forbidden-pattern");
        assert!(check(&proposal, &[]).is_err());
        proposal.pattern = Some("require($$$A)".into());
        proposal.bad = Some("const a = require('a');".into());
        proposal.good = Some("import a from 'a';".into());
        assert_eq!(
            check(&proposal, &[]),
            Ok(Check::Pattern {
                pattern: "require($$$A)".into(),
                examples: Examples {
                    bad: "const a = require('a');".into(),
                    good: "import a from 'a';".into(),
                },
            })
        );
        proposal.bad = Some("import a from 'a';".into());
        assert!(
            check(&proposal, &[])
                .unwrap_err()
                .contains("does not match its bad example")
        );
        proposal.pattern = Some("$$$A".into());
        assert!(
            check(&proposal, &[])
                .unwrap_err()
                .contains("does not compile")
        );
    }

    #[test]
    fn an_unknown_rule_is_refused() {
        assert!(check(&raw("max-lines"), &globs(&["src/**"])).is_err());
    }
}
