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
            // Missing is the normal first-run state, and silent -- there is
            // nothing to lose by treating it as an empty registry.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Registry::default(), None),
            // Anything else -- permissions, a directory sitting where the file
            // should be, any other I/O failure -- is NOT "no registry yet." It
            // means the real content could not be read, so reporting
            // `Registry::default()` here without a problem would let the very
            // next `save()` overwrite whatever is actually on disk with an
            // empty (or one-entry) file, silently discarding it. Callers that
            // persist must treat a `Some` here as a hard stop, not a status
            // line -- see `add_and_save`/`remove_and_save`.
            Err(e) => (Registry::default(), Some(format!("repos.toml: {e}"))),
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

    /// Adds `path` against whatever is actually on disk right now, not
    /// whatever `self` happens to hold, and only updates `self` once the
    /// write has actually landed.
    ///
    /// Two running dextui instances -- the ordinary case for a tool whose
    /// whole point is several repos -- registering different paths around the
    /// same moment would otherwise each trust its own in-memory snapshot and
    /// blindly overwrite the other's write. Re-reading first narrows that
    /// race to the gap between this read and this write, rather than the gap
    /// since each process started. It also refuses to write anything at all
    /// when the on-disk file cannot be read back honestly (see `load`),
    /// rather than overwriting content this process never actually saw.
    #[allow(dead_code)]
    pub fn add_and_save(&mut self, path: &str) -> Result<bool, String> {
        let (mut current, problem) = Registry::load();
        if let Some(p) = problem {
            return Err(p);
        }
        let changed = current.add(path);
        if changed {
            current.save()?;
            *self = current;
        }
        Ok(changed)
    }

    /// Mirrors `add_and_save`. Returns `Err` -- rather than silently keeping
    /// the in-memory removal -- when the change could not actually be
    /// persisted, so the caller can say so instead of reporting success on an
    /// entry that will simply reappear at the next launch.
    #[allow(dead_code)]
    pub fn remove_and_save(&mut self, path: &str) -> Result<bool, String> {
        let (mut current, problem) = Registry::load();
        if let Some(p) = problem {
            return Err(p);
        }
        let changed = current.remove(path);
        if changed {
            current.save()?;
            *self = current;
        }
        Ok(changed)
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

    /// `crate::test_support::with_isolated_registry`, not a copy of its own:
    /// this module and `app.rs`'s own tests both mutate the same
    /// process-wide `XDG_CONFIG_HOME`, and two independent locks -- one per
    /// module -- would not actually exclude each other from it. Only one
    /// shared lock, used by both, does.
    use crate::test_support::with_isolated_registry;

    /// The normal first-run state: nothing has ever been saved, so there is
    /// nothing to lose by treating it as an empty registry, silently.
    #[test]
    fn a_missing_file_loads_as_empty_and_silent() {
        with_isolated_registry("registry-missing", || {
            let (reg, problem) = Registry::load();
            assert_eq!(reg, Registry::default());
            assert!(problem.is_none(), "a first run must not be reported as a problem");
        });
    }

    /// Anything other than "the file does not exist yet" -- permissions, a
    /// directory sitting where the file should be -- must be reported, or the
    /// very next save would silently overwrite content this process never
    /// actually read. A directory at the registry's path is a reliable,
    /// portable way to provoke a non-`NotFound` read error without needing
    /// real permission bits.
    #[test]
    fn an_unreadable_file_is_reported_not_treated_as_empty() {
        with_isolated_registry("registry-unreadable", || {
            let p = path().unwrap();
            std::fs::create_dir_all(&p).unwrap(); // a dir where a file is expected

            let (reg, problem) = Registry::load();
            assert_eq!(reg, Registry::default());
            assert!(problem.is_some(), "an unreadable file must be reported");
        });
    }

    #[test]
    fn add_and_save_persists_and_updates_self() {
        with_isolated_registry("registry-add", || {
            let mut r = Registry::default();
            assert!(r.add_and_save("/x/one").unwrap());
            assert_eq!(r.repos, vec!["/x/one".to_string()]);

            let (on_disk, _) = Registry::load();
            assert_eq!(on_disk.repos, vec!["/x/one".to_string()]);
        });
    }

    /// The whole point: a second in-memory copy (standing in for a second
    /// dextui process) must not be able to blindly overwrite what the first
    /// one already wrote. `add_and_save` re-reads before writing, so both
    /// additions -- made against two independent `Registry` values that never
    /// saw each other's change -- must both survive.
    #[test]
    fn add_and_save_does_not_clobber_a_concurrent_write() {
        with_isolated_registry("registry-concurrent", || {
            let mut first = Registry::default();
            let mut second = Registry::default(); // stands in for another process

            assert!(first.add_and_save("/x/one").unwrap());
            assert!(second.add_and_save("/x/two").unwrap());

            let (on_disk, _) = Registry::load();
            assert_eq!(
                on_disk.repos,
                vec!["/x/one".to_string(), "/x/two".to_string()],
                "second's write must not have discarded first's"
            );
        });
    }

    #[test]
    fn remove_and_save_persists_and_updates_self() {
        with_isolated_registry("registry-remove", || {
            let mut r = Registry::default();
            r.add_and_save("/x/one").unwrap();
            r.add_and_save("/x/two").unwrap();

            assert!(r.remove_and_save("/x/one").unwrap());
            assert_eq!(r.repos, vec!["/x/two".to_string()]);

            let (on_disk, _) = Registry::load();
            assert_eq!(on_disk.repos, vec!["/x/two".to_string()]);
        });
    }

    /// A registry that failed to load must never be saved over -- the whole
    /// point of reporting the problem in the first place.
    #[test]
    fn add_and_save_refuses_when_the_file_cannot_be_read() {
        with_isolated_registry("registry-add-refuses", || {
            let p = path().unwrap();
            std::fs::create_dir_all(&p).unwrap(); // unreadable as a file

            let mut r = Registry::default();
            let err = r.add_and_save("/x/one").unwrap_err();
            assert!(!err.is_empty());
            // Nothing was written -- the directory is still a directory, not
            // a registry file, and `self` was not mutated either.
            assert!(r.repos.is_empty());
        });
    }

    #[test]
    fn remove_and_save_refuses_when_the_file_cannot_be_read() {
        with_isolated_registry("registry-remove-refuses", || {
            let p = path().unwrap();
            std::fs::create_dir_all(&p).unwrap();

            let mut r = Registry {
                repos: vec!["/x/one".into()],
            };
            let err = r.remove_and_save("/x/one").unwrap_err();
            assert!(!err.is_empty());
            // The in-memory copy is left exactly as it was -- rolled back
            // rather than half-applied -- so the caller's own state does not
            // silently disagree with what is (or isn't) actually on disk.
            assert_eq!(r.repos, vec!["/x/one".to_string()]);
        });
    }

    #[test]
    fn an_empty_file_is_normal_and_silent() {
        let (reg, problem) = Registry::parse("");
        assert_eq!(reg, Registry::default());
        assert!(problem.is_none(), "an empty registry is not a problem");
    }
}
