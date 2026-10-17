//! Extraction: spec doc paragraphs to clauses, through the user's own model CLI.

use serde::{Deserialize, Serialize};

pub mod cache;
pub mod derive;
pub mod lockfile;
pub mod model;
pub mod prompt;
pub mod run;
pub mod status;
pub mod validate;

/// The version of the prompt, schema and parsing. Bump it when any of them changes, so cached
/// answers from the old extractor are not reused.
/// 2: a paragraph whose clauses all fail to read is no longer cached as "no clauses".
pub const EXTRACTOR_VERSION: u32 = 2;

/// When a clause is enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rung {
    /// Checked by code during each edit; needs a library check and a lock entry.
    Blocking,
    /// Checked when the turn ends, by its code check or the model. Never blocks.
    Judged,
    /// Handed to the agent. Never checked.
    Context,
}
