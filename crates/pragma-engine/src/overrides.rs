//! `.pragma/overrides.md`: user corrections and exceptions, as data in YAML frontmatter above
//! prose the user owns. pragma rewrites only the keys it manages and keeps every other key.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use crate::audit::{AuditFinding, AuditReport};
use crate::drift::{DriftFinding, DriftReport};
use crate::frontmatter;
use crate::rules::Finding;

pub const OVERRIDES_FILE: &str = "overrides.md";

pub const DEFAULT_BODY: &str = "# pragma overrides\n\nCorrections and exceptions for this repo's rules. pragma keeps the list above; the text here\nis yours.\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// The rule is right, and this code is sanctioned anyway.
    Deliberate,
    /// The check fired on code that does not break the rule.
    CheckIsWrong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Exception {
    Violation {
        clause: String,
        para_hash: String,
        file: String,
        matched: String,
        reason: Reason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Drift {
        doc: String,
        para_hash: String,
        reference: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
}

impl Exception {
    /// Covers a finding of a clause whose paragraph hash is `para_hash`. Matches by clause id, or
    /// by paragraph when the clause was re-derived and got a new id.
    pub fn covers_violation(&self, finding: &Finding, para_hash: &str) -> bool {
        match self {
            Exception::Violation {
                clause,
                para_hash: exception_para,
                file,
                matched,
                ..
            } => {
                (clause == &finding.clause_id || exception_para == para_hash)
                    && file == &finding.file
                    && matched == &finding.matched
            }
            Exception::Drift { .. } => false,
        }
    }

    pub fn covers_drift(&self, finding: &DriftFinding) -> bool {
        match self {
            Exception::Drift {
                doc,
                para_hash,
                reference,
                ..
            } => {
                doc == &finding.doc
                    && para_hash == &finding.para_hash
                    && reference == &finding.reference
            }
            Exception::Violation { .. } => false,
        }
    }
}

pub struct Overrides {
    data: Mapping,
    body: String,
}

impl Overrides {
    /// The overrides at `path`; empty when the file does not exist.
    pub fn load(path: &Path) -> Result<Overrides> {
        if !path.exists() {
            return Ok(Overrides {
                data: Mapping::new(),
                body: DEFAULT_BODY.to_string(),
            });
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let (yaml, body) = frontmatter::split(&text);
        let data = match yaml.filter(|yaml| !yaml.trim().is_empty()) {
            Some(yaml) => match serde_norway::from_str::<Value>(yaml)? {
                Value::Mapping(mapping) => mapping,
                _ => {
                    return Err(anyhow!(
                        "{}: the frontmatter is not a mapping",
                        path.display()
                    ));
                }
            },
            None => Mapping::new(),
        };
        Ok(Overrides {
            data,
            body: body.to_string(),
        })
    }

    pub fn exceptions(&self) -> Result<Vec<Exception>> {
        match self.data.get("exceptions") {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(value) => {
                Ok(serde_norway::from_value(value.clone()).context("reading exceptions")?)
            }
        }
    }

    /// Adds `exception`. Returns false when an identical one is already there.
    pub fn add_exception(&mut self, exception: Exception) -> Result<bool> {
        let mut exceptions = self.exceptions()?;
        if exceptions.contains(&exception) {
            return Ok(false);
        }
        exceptions.push(exception);
        self.data.insert(
            Value::from("exceptions"),
            serde_norway::to_value(&exceptions)?,
        );
        Ok(true)
    }

    pub fn to_text(&self) -> Result<String> {
        Ok(format!(
            "---\n{}---\n{}",
            serde_norway::to_string(&self.data)?,
            self.body
        ))
    }
}

/// True when one of `exceptions` covers `finding`; `para_hash` is the paragraph hash of the
/// finding's clause.
pub fn excepted(exceptions: &[Exception], finding: &Finding, para_hash: &str) -> bool {
    exceptions
        .iter()
        .any(|exception| exception.covers_violation(finding, para_hash))
}

/// Drops audit findings an exception covers, and returns the violation exceptions that covered
/// nothing: their code changed or went away. Only exceptions for files the audit `looked_at`
/// (all of them, an area, or what a branch changed) can be stale, because it saw nothing else.
pub fn apply_to_audit(
    report: &mut AuditReport,
    exceptions: &[Exception],
    looked_at: impl Fn(&str) -> bool,
    para_of: impl Fn(&str) -> String,
) -> Vec<Exception> {
    let covers = |exception: &Exception, found: &AuditFinding| {
        exception.covers_violation(&found.finding, &para_of(&found.finding.clause_id))
    };
    let stale = exceptions
        .iter()
        .filter(
            |exception| matches!(exception, Exception::Violation { file, .. } if looked_at(file)),
        )
        .filter(|exception| !report.findings.iter().any(|found| covers(exception, found)))
        .cloned()
        .collect();
    report.findings.retain(|found| {
        !excepted(
            exceptions,
            &found.finding,
            &para_of(&found.finding.clause_id),
        )
    });
    report.counts = BTreeMap::new();
    for found in &report.findings {
        *report
            .counts
            .entry(found.finding.clause_id.clone())
            .or_default() += 1;
    }
    stale
}

/// Drops drift findings marked intended, and returns the drift exceptions that covered nothing.
pub fn apply_to_drift(report: &mut DriftReport, exceptions: &[Exception]) -> Vec<Exception> {
    let stale = exceptions
        .iter()
        .filter(|exception| matches!(exception, Exception::Drift { .. }))
        .filter(|exception| {
            !report
                .findings
                .iter()
                .any(|found| exception.covers_drift(found))
        })
        .cloned()
        .collect();
    report.findings.retain(|found| {
        !exceptions
            .iter()
            .any(|exception| exception.covers_drift(found))
    });
    stale
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drift::{Confidence, DriftKind};

    fn violation() -> Exception {
        Exception::Violation {
            clause: "c1".into(),
            para_hash: "p1".into(),
            file: "src/a.ts".into(),
            matched: "require('x')".into(),
            reason: Reason::Deliberate,
            note: Some("CommonJS-only module".into()),
        }
    }

    #[test]
    fn round_trips_and_keeps_unknown_keys_and_the_body() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(OVERRIDES_FILE);
        std::fs::write(
            &path,
            "---\nclauses:\n  c9: {rung: off}\n---\n# Mine\n\nMy notes.\n",
        )
        .unwrap();
        let mut overrides = Overrides::load(&path).unwrap();
        assert!(overrides.add_exception(violation()).unwrap());
        assert!(!overrides.add_exception(violation()).unwrap());
        let text = overrides.to_text().unwrap();
        assert!(
            text.contains("c9:") && text.ends_with("# Mine\n\nMy notes.\n"),
            "{text}"
        );
        std::fs::write(&path, &text).unwrap();
        assert_eq!(
            Overrides::load(&path).unwrap().exceptions().unwrap(),
            vec![violation()]
        );
    }

    #[test]
    fn a_missing_file_starts_empty_with_the_default_body() {
        let temp = tempfile::tempdir().unwrap();
        let overrides = Overrides::load(&temp.path().join(OVERRIDES_FILE)).unwrap();
        assert!(overrides.exceptions().unwrap().is_empty());
        assert!(overrides.to_text().unwrap().contains("# pragma overrides"));
    }

    #[test]
    fn exceptions_cover_by_clause_or_paragraph_and_by_code() {
        let finding = Finding {
            clause_id: "c1".into(),
            file: "src/a.ts".into(),
            line: 3,
            matched: "require('x')".into(),
        };
        assert!(violation().covers_violation(&finding, "other"));
        let rederived = Finding {
            clause_id: "c2".into(),
            ..finding.clone()
        };
        assert!(violation().covers_violation(&rederived, "p1"));
        assert!(!violation().covers_violation(&rederived, "other"));
        let elsewhere = Finding {
            file: "src/b.ts".into(),
            ..finding
        };
        assert!(!violation().covers_violation(&elsewhere, "p1"));
        let drift = DriftFinding {
            id: "d".into(),
            doc: "AGENTS.md".into(),
            line: 1,
            para_hash: "p".into(),
            kind: DriftKind::MissingPath,
            reference: "src/example.ts".into(),
            suggestion: None,
            confidence: Confidence::High,
            fix_span: None,
            fix_original: None,
        };
        let intended = Exception::Drift {
            doc: "AGENTS.md".into(),
            para_hash: "p".into(),
            reference: "src/example.ts".into(),
            note: None,
        };
        assert!(intended.covers_drift(&drift));
        assert!(!violation().covers_drift(&drift));
    }

    #[test]
    fn apply_to_audit_filters_recounts_and_reports_stale_exceptions() {
        let found = |file: &str| AuditFinding {
            id: file.into(),
            finding: Finding {
                clause_id: "c1".into(),
                file: file.into(),
                line: 1,
                matched: "require('x')".into(),
            },
        };
        let mut report = AuditReport {
            files_checked: 2,
            findings: vec![found("src/a.ts"), found("src/b.ts")],
            counts: BTreeMap::from([("c1".to_string(), 2)]),
        };
        let gone = Exception::Violation {
            clause: "c1".into(),
            para_hash: "p1".into(),
            file: "src/gone.ts".into(),
            matched: "require('x')".into(),
            reason: Reason::Deliberate,
            note: None,
        };
        let stale = apply_to_audit(
            &mut report,
            &[violation(), gone.clone()],
            |_| true,
            |_| "p1".into(),
        );
        assert_eq!(report.findings, vec![found("src/b.ts")]);
        assert_eq!(report.counts, BTreeMap::from([("c1".to_string(), 1)]));
        assert_eq!(stale, vec![gone]);
        let mut report = AuditReport::default();
        assert!(
            apply_to_audit(
                &mut report,
                &[violation()],
                |file| file.starts_with("lib/"),
                |_| "p1".into()
            )
            .is_empty()
        );
    }
}
