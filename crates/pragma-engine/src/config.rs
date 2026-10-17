//! `.pragma/config.md`: repo settings in YAML frontmatter.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_norway::{Mapping, Value};

use crate::frontmatter;

pub const CONFIG_FILE: &str = "config.md";
/// The model extraction and judging use when `config.md` names none (spike S0a).
pub const DEFAULT_MODEL: &str = "haiku";

pub const DEFAULT_CONFIG: &str = "---\ndocs:\n  include: []\n  exclude: []\n---\n# pragma settings\n\n\
The block above is this repo's pragma settings. `docs.include` and `docs.exclude` add or remove\n\
spec docs by glob, on top of the docs pragma finds by itself.\n";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub docs: DocGlobs,
    /// The model alias for extraction and judging; `DEFAULT_MODEL` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocGlobs {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Config {
    pub fn model(&self) -> &str {
        self.model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .unwrap_or(DEFAULT_MODEL)
    }

    /// The config in `pragma_dir`, or the default when there is no `.pragma/` or no config.
    pub fn load(pragma_dir: Option<&Path>) -> Result<Config> {
        let Some(path) = pragma_dir
            .map(|dir| dir.join(CONFIG_FILE))
            .filter(|path| path.is_file())
        else {
            return Ok(Config::default());
        };
        let text = std::fs::read_to_string(&path)?;
        match frontmatter::split(&text).0 {
            Some(yaml) if !yaml.trim().is_empty() => {
                serde_norway::from_str(yaml).with_context(|| format!("reading {}", path.display()))
            }
            _ => Ok(Config::default()),
        }
    }
}

/// Rewrites `docs.include` and `docs.exclude` in the frontmatter of a `config.md`, leaving other
/// keys and the prose after the frontmatter as they are. `None` leaves a key alone. A file with
/// no frontmatter gets one in front of its text.
pub fn set_doc_globs(
    text: &str,
    include: Option<&[String]>,
    exclude: Option<&[String]>,
) -> Result<String> {
    let (yaml, body) = frontmatter::split(text);
    let mut root: Value = match yaml {
        Some(yaml) if !yaml.trim().is_empty() => {
            serde_norway::from_str(yaml).context("config.md frontmatter is not valid YAML")?
        }
        _ => Value::Mapping(Mapping::new()),
    };
    let Value::Mapping(root_map) = &mut root else {
        bail!("config.md frontmatter is not a mapping");
    };
    let docs = root_map
        .entry(Value::from("docs"))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if docs.is_null() {
        *docs = Value::Mapping(Mapping::new());
    }
    let Value::Mapping(docs) = docs else {
        bail!("config.md \"docs\" is not a mapping");
    };
    for (key, globs) in [("include", include), ("exclude", exclude)] {
        if let Some(globs) = globs {
            docs.insert(
                Value::from(key),
                Value::Sequence(
                    globs
                        .iter()
                        .map(|glob| Value::from(glob.as_str()))
                        .collect(),
                ),
            );
        }
    }
    let yaml = serde_norway::to_string(&root)?;
    Ok(format!("---\n{yaml}---\n{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_config_parses() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(CONFIG_FILE), DEFAULT_CONFIG).unwrap();
        assert_eq!(Config::load(Some(temp.path())).unwrap(), Config::default());
    }

    #[test]
    fn reads_doc_globs_and_tolerates_missing_files() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(Some(temp.path())).unwrap(), Config::default());
        assert_eq!(Config::load(None).unwrap(), Config::default());
        std::fs::write(
            temp.path().join(CONFIG_FILE),
            "---\ndocs:\n  exclude: [\"docs/old/**\"]\n---\n",
        )
        .unwrap();
        let config = Config::load(Some(temp.path())).unwrap();
        assert_eq!(config.docs.exclude, vec!["docs/old/**"]);
        assert_eq!(config.model(), DEFAULT_MODEL);
        std::fs::write(temp.path().join(CONFIG_FILE), "---\nmodel: sonnet\n---\n").unwrap();
        assert_eq!(Config::load(Some(temp.path())).unwrap().model(), "sonnet");
    }

    fn globs(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn set_doc_globs_changes_only_the_named_keys_and_keeps_the_body() {
        let text =
            "---\nowner: me\ndocs:\n  include: [a]\n  exclude: [b]\n---\n# Mine\n\nProse stays.\n";
        let out = set_doc_globs(text, Some(&globs(&["x/**", "y.md"])), None).unwrap();
        assert!(out.ends_with("---\n# Mine\n\nProse stays.\n"), "{out}");
        let config = frontmatter::split(&out).0.unwrap();
        let value: Value = serde_norway::from_str(config).unwrap();
        assert_eq!(value["owner"], Value::from("me"));
        assert_eq!(value["docs"]["exclude"][0], Value::from("b"));
        assert_eq!(value["docs"]["include"][1], Value::from("y.md"));
        assert_eq!(value["docs"]["include"].as_sequence().unwrap().len(), 2);
    }

    #[test]
    fn set_doc_globs_clears_with_an_empty_list_and_round_trips_through_load() {
        let temp = tempfile::tempdir().unwrap();
        let out =
            set_doc_globs(DEFAULT_CONFIG, Some(&globs(&["a"])), Some(&globs(&["b"]))).unwrap();
        let out = set_doc_globs(&out, Some(&[]), None).unwrap();
        std::fs::write(temp.path().join(CONFIG_FILE), &out).unwrap();
        let config = Config::load(Some(temp.path())).unwrap();
        assert!(config.docs.include.is_empty());
        assert_eq!(config.docs.exclude, vec!["b"]);
    }

    #[test]
    fn set_doc_globs_adds_frontmatter_to_a_file_without_any() {
        let out = set_doc_globs("# Notes\n", None, Some(&globs(&["z"]))).unwrap();
        assert!(
            out.starts_with("---\ndocs:\n  exclude:\n  - z\n---\n# Notes\n"),
            "{out}"
        );
    }

    #[test]
    fn set_doc_globs_refuses_frontmatter_it_cannot_keep() {
        assert!(set_doc_globs("---\n- a\n---\n", None, Some(&[])).is_err());
        assert!(set_doc_globs("---\ndocs: 3\n---\n", None, Some(&[])).is_err());
    }
}
