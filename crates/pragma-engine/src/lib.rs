//! The pragma engine: everything the `pragma` binary does, as a library.

pub mod audit;
pub mod backtest;
pub mod config;
pub mod docs;
pub mod drift;
pub mod edit;
pub mod env;
pub mod events;
pub mod extract;
pub mod frontmatter;
pub mod gate;
pub mod hashing;
pub mod index;
pub mod introduced;
pub mod lang;
pub mod lock;
pub mod lock_update;
pub mod markdown;
pub mod matcher;
pub mod message;
pub mod overrides;
pub mod paths;
pub mod refs;
pub mod repo;
pub mod repo_index;
pub mod rules;
pub mod settings;
pub mod writes;

#[cfg(test)]
pub(crate) mod testutil;
