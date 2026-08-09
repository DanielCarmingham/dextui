//! An always-on, file-only diagnostic log for the watch/refresh path.
//!
//! The app's only other feedback channel is `app.status` -- one line,
//! overwritten, with no history -- and once the alternate screen is up,
//! stdout and stderr belong to the TUI, so nothing can be printed there
//! either. Worse, the stat-gated safety net in `watch.rs` has a "decided
//! **not** to refresh" branch that is invisible by design: on a quiet tick it
//! does nothing at all, which is correct and indistinguishable from a broken
//! watcher without this.
//!
//! **Always on.** An opt-in log is off precisely when the bug you did not
//! expect happens, and a sync fault is the kind that will not reproduce on
//! demand.
//!
//! **File only**, at `$XDG_STATE_HOME/dextui/log`, falling back to
//! `~/.local/state/dextui/log`. State, not config: it is machine-local and
//! disposable, and must never sit beside `config.toml`, which is the user's
//! hand-edited text.
//!
//! **Size-capped by truncation at startup, not rotation.** A log you `tail -f`
//! while reproducing a fault does not need history, and rotation is machinery
//! for a problem this does not have.
//!
//! **Failure is silent and total.** If the file cannot be opened or written,
//! the app behaves exactly as if logging were off -- no panic, no status-bar
//! complaint, no error for a caller to handle. A logger that can break the
//! program it exists to diagnose is worse than no logger.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Above this many bytes the log is truncated at startup. Generous enough to
/// hold a long `tail -f` session's worth of watch/dex/store/registry lines
/// without ever mattering in ordinary use.
const CAP: usize = 1_000_000;

/// The resolved path, computed once by `init()`. `None` means logging is off
/// for the rest of the run -- either `HOME` could not be resolved, or nothing
/// has called `init()` yet.
static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Resolves the log path and truncates it if oversized. Call once, early in
/// `main`, before the TUI takes the terminal.
///
/// Never fails outwardly: an unresolvable path just leaves `line` a no-op for
/// the rest of the run.
pub fn init() {
    let resolved = resolve();
    if let Some(p) = &resolved {
        truncate_if_oversized(p);
    }
    let _ = LOG_PATH.set(resolved);
}

/// The resolved log path, if `init()` found one usable. Exposed mainly so
/// `dextui config`-style diagnostics could point at it; nothing here reads it
/// back.
#[allow(dead_code)]
pub fn path() -> Option<PathBuf> {
    LOG_PATH.get().cloned().flatten()
}

/// Appends one line, or does nothing at all if `init()` found no usable path
/// -- which is also what happens if `init()` was never called, so a missing
/// call site is silent rather than a panic.
pub fn line(area: &str, msg: &str) {
    if let Some(Some(p)) = LOG_PATH.get() {
        write_line(p, area, msg);
    }
}

/// Where the log lives, honouring `XDG_STATE_HOME` like `config::path` honours
/// `XDG_CONFIG_HOME` -- same shape, different variable, because a log is
/// disposable machine-local state and must never sit beside the user's
/// hand-edited `config.toml`.
fn resolve() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local").join("state"),
    };
    Some(base.join("dextui").join("log"))
}

/// `HH:MM:SS  area  message`, area padded to a fixed column so the file reads
/// straight rather than ragged. `registry` (8 characters) is the longest of
/// the four areas in use, so that is the padding width.
fn format_line(area: &str, msg: &str) -> String {
    let ts = chrono::Local::now().format("%H:%M:%S");
    format!("{ts}  {area:<8}  {msg}\n")
}

/// Appends one formatted line, creating the containing directory if needed.
/// Every write opens fresh with `append(true).create(true)` -- no buffering
/// held across calls, so an append per event is cheap at human tempo and a
/// crash never loses a line that was already written.
///
/// Any failure -- an uncreatable directory, an unopenable or unwritable file
/// -- is discarded rather than propagated. See the module docs: logging must
/// never be a way for the app itself to break.
fn write_line(path: &Path, area: &str, msg: &str) {
    if let Some(dir) = path.parent()
        && std::fs::create_dir_all(dir).is_err()
    {
        return;
    }
    let Ok(mut file) = OpenOptions::new().append(true).create(true).open(path) else {
        return;
    };
    let _ = file.write_all(format_line(area, msg).as_bytes());
}

/// Truncates the file to empty if it has grown past `CAP`. A missing file, or
/// one that cannot be inspected, is left exactly alone -- there is nothing to
/// truncate, and it is not this function's job to report why.
fn truncate_if_oversized(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() > CAP as u64 {
        let _ = OpenOptions::new().write(true).truncate(true).open(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole value is that it is already on when something goes wrong, so a
    /// missing directory must be created rather than silently dropping the log.
    #[test]
    fn a_line_is_appended_and_the_directory_is_created() {
        let dir = std::env::temp_dir().join(format!("dextui-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("nested").join("log");

        write_line(&file, "watch", "registered /x/.dex");
        write_line(&file, "dex", "list /x/.dex - 14 tasks 173ms");

        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("registered /x/.dex"), "{text}");
        assert!(text.contains("14 tasks"), "{text}");
        assert_eq!(text.lines().count(), 2, "appended, not overwritten: {text}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A logger that can break the program it exists to diagnose is worse than no
    /// logger. An unwritable path must be a no-op, not a panic and not an error
    /// the caller has to handle.
    #[test]
    fn an_unwritable_path_is_silently_ignored() {
        // /dev/null/x cannot be created: /dev/null is not a directory.
        write_line(std::path::Path::new("/dev/null/x/log"), "watch", "ignored");
    }

    /// Truncation, not rotation. Reproducing a fault does not need history, and a
    /// log that grows without bound is its own problem.
    #[test]
    fn an_oversized_log_is_truncated_at_startup() {
        let dir = std::env::temp_dir().join(format!("dextui-log-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("log");
        std::fs::write(&file, "x".repeat(CAP + 1)).unwrap();

        truncate_if_oversized(&file);

        assert_eq!(std::fs::metadata(&file).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_within_the_cap_survives_startup() {
        let dir = std::env::temp_dir().join(format!("dextui-log-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("log");
        std::fs::write(&file, "kept").unwrap();

        truncate_if_oversized(&file);

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The area column is what makes the file skimmable; a ragged one is not.
    #[test]
    fn the_area_column_is_aligned() {
        let a = format_line("watch", "one");
        let b = format_line("registry", "two");
        let col = |s: &str| s.find("one").or_else(|| s.find("two")).unwrap();
        assert_eq!(col(&a), col(&b), "ragged columns:\n{a}\n{b}");
    }
}
