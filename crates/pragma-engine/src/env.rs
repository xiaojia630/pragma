//! Environment access behind a trait, so tests can supply their own variables.

use std::collections::HashMap;
use std::path::PathBuf;

pub trait Env {
    /// The variable's value. Unset and empty are the same.
    fn var(&self, key: &str) -> Option<String>;
}

/// The real process environment.
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|value| !value.is_empty())
    }
}

impl Env for HashMap<String, String> {
    fn var(&self, key: &str) -> Option<String> {
        self.get(key).filter(|value| !value.is_empty()).cloned()
    }
}

/// The user's home directory: `HOME`, or `USERPROFILE` on Windows.
pub fn home_dir(env: &impl Env) -> Option<PathBuf> {
    env.var("HOME")
        .or_else(|| env.var("USERPROFILE"))
        .map(PathBuf::from)
}

/// pragma's per-user directory: `$PRAGMA_HOME`, else `~/.pragma`.
pub fn pragma_home(env: &impl Env) -> Option<PathBuf> {
    env.var("PRAGMA_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir(env).map(|home| home.join(".pragma")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn pragma_home_prefers_the_override() {
        let env = env(&[("PRAGMA_HOME", "/opt/pragma"), ("HOME", "/home/me")]);
        assert_eq!(pragma_home(&env), Some(PathBuf::from("/opt/pragma")));
    }

    #[test]
    fn pragma_home_falls_back_to_the_home_directory() {
        let env = env(&[("HOME", "/home/me"), ("PRAGMA_HOME", "")]);
        assert_eq!(pragma_home(&env), Some(PathBuf::from("/home/me/.pragma")));
    }

    #[test]
    fn pragma_home_is_none_without_a_home() {
        assert_eq!(pragma_home(&env(&[])), None);
    }
}
