//! The list of repositories the sidebar shows.
//!
//! **A separate file from `config.toml` on purpose.** That file is read-only to
//! the app -- persisting toggles would clobber a file someone hand-edited, and
//! silently change their defaults. This one is app-owned state that the app
//! writes. Two files, and neither rule needs qualifying.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[allow(dead_code)]
pub struct Registry {
    #[serde(default)]
    pub repos: Vec<String>,
}

impl Registry {
    /// Returns whether the list changed, so a duplicate `a` can be reported
    /// rather than looking like nothing happened.
    #[allow(dead_code)]
    pub fn add(&mut self, path: &str) -> bool {
        if self.repos.iter().any(|p| p == path) {
            return false;
        }
        self.repos.push(path.to_string());
        self.repos.sort();
        true
    }

    #[allow(dead_code)]
    pub fn remove(&mut self, path: &str) -> bool {
        let before = self.repos.len();
        self.repos.retain(|p| p != path);
        self.repos.len() != before
    }

    /// Parses registry text, reporting a problem rather than failing.
    #[allow(dead_code)]
    pub fn parse(text: &str) -> (Registry, Option<String>) {
        if text.trim().is_empty() {
            return (Registry::default(), None);
        }
        match toml::from_str::<Registry>(text) {
            Ok(r) => (r, None),
            Err(e) => (Registry::default(), Some(format!("repos.toml: {e}"))),
        }
    }

    #[allow(dead_code)]
    pub fn load() -> (Registry, Option<String>) {
        let Some(p) = path() else {
            return (Registry::default(), None);
        };
        match std::fs::read_to_string(&p) {
            Ok(text) => Registry::parse(&text),
            // Missing is the normal first-run state, and silent.
            Err(_) => (Registry::default(), None),
        }
    }

    #[allow(dead_code)]
    pub fn save(&self) -> Result<(), String> {
        let p = path().ok_or_else(|| "could not resolve a config directory".to_string())?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = toml::to_string(self).map_err(|e| format!("could not serialise: {e}"))?;
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// Beside `config.toml`, resolved the same way so both follow XDG.
#[allow(dead_code)]
pub fn path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("dextui").join("repos.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_a_repo_reports_that_it_changed() {
        let mut r = Registry::default();
        assert!(r.add("/x/one"));
        assert_eq!(r.repos, vec!["/x/one".to_string()]);
    }

    /// Pressing `a` twice is not a mistake worth an error, but the caller needs
    /// to know so it can say "already registered" instead of nothing at all.
    #[test]
    fn adding_a_known_repo_changes_nothing_and_says_so() {
        let mut r = Registry::default();
        r.add("/x/one");
        assert!(!r.add("/x/one"));
        assert_eq!(r.repos.len(), 1, "duplicated: {:?}", r.repos);
    }

    #[test]
    fn removing_reports_whether_it_was_there() {
        let mut r = Registry::default();
        r.add("/x/one");
        assert!(r.remove("/x/one"));
        assert!(!r.remove("/x/one"));
        assert!(r.repos.is_empty());
    }

    #[test]
    fn a_registry_round_trips_through_toml() {
        let mut r = Registry::default();
        r.add("/x/one");
        r.add("/x/two");

        let text = toml::to_string(&r).unwrap();
        let back: Registry = toml::from_str(&text).unwrap();
        assert_eq!(back, r);
    }

    /// A bad file must not stop the app starting -- the same rule config already
    /// follows, for the same reason.
    #[test]
    fn a_malformed_file_is_reported_and_not_fatal() {
        let (reg, problem) = Registry::parse("repos = \"not a list\"");
        assert_eq!(reg, Registry::default());
        assert!(problem.is_some(), "a bad file must be reported");
    }

    #[test]
    fn an_empty_file_is_normal_and_silent() {
        let (reg, problem) = Registry::parse("");
        assert_eq!(reg, Registry::default());
        assert!(problem.is_none(), "an empty registry is not a problem");
    }
}
