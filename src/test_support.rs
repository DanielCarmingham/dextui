//! Shared test-only infrastructure for isolating process-wide environment
//! state.
//!
//! `src/app.rs` and `src/registry.rs` both need to point `XDG_CONFIG_HOME` at
//! a scratch directory while exercising `Registry::load`/`save` for real, so
//! neither ever touches the user's actual `~/.config/dextui/repos.toml`. Two
//! independent locks -- one defined in each module -- do not exclude each
//! other: both mutate the *same* process-wide variable, so their tests could
//! still run concurrently and clobber it out from under one another. Only one
//! shared lock, used by both modules, actually serialises them.
#![cfg(test)]

use std::sync::{Mutex, OnceLock};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Sets `vars` for the duration of `f`, restoring whatever was there
/// beforehand once `f` returns -- including when `f` panics, since the
/// restore runs via `Drop` rather than as plain code after the call.
///
/// `std::env::set_var`/`remove_var` are `unsafe` in edition 2024 precisely
/// because they mutate process-wide state, which is also exactly why every
/// caller has to go through the same lock here rather than one of its own.
///
/// The lock guard is bound as `_guard` and never dropped explicitly: locals
/// drop in reverse declaration order, so the restore (bound after the guard)
/// runs first and the lock is released only once the environment is back the
/// way it was. Dropping the guard early -- even by one statement -- would let
/// another thread take the lock and snapshot a value this call has not
/// finished restoring yet.
pub(crate) fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let saved: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(k, _)| ((*k).to_string(), std::env::var(k).ok()))
        .collect();

    struct Restore(Vec<(String, Option<String>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (k, v) in &self.0 {
                match v {
                    Some(v) => unsafe { std::env::set_var(k, v) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }
    let _restore = Restore(saved);

    for (k, v) in vars {
        match v {
            Some(v) => unsafe { std::env::set_var(k, v) },
            None => unsafe { std::env::remove_var(k) },
        }
    }

    f()
}

/// Points `XDG_CONFIG_HOME` at a fresh, empty scratch directory (keyed by
/// `tag` as well as pid, so tests never share -- and cannot leak state
/// through -- the same directory) for the duration of `f`, so anything that
/// calls `Registry::load`/`save` for real never touches the user's actual
/// `~/.config/dextui/repos.toml`. A suite that rewrote that file on every run
/// would be worse than no suite at all.
pub(crate) fn with_isolated_registry<T>(tag: &str, f: impl FnOnce() -> T) -> T {
    let dir = std::env::temp_dir().join(format!("dextui-test-registry-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    with_env(&[("XDG_CONFIG_HOME", Some(dir.to_str().unwrap()))], f)
}
