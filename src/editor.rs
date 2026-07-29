//! Hands a task description to `$EDITOR`.
//!
//! The in-app prompt is a single-line field, which cannot honestly edit a
//! multi-line description: it showed only the first line while the cursor sat at
//! the end of the last, so typing appended to text you could not see.
//!
//! Handing off to a real editor also means the TUI must stop reading the
//! terminal for the duration — see the main loop, which polls for input rather
//! than running a reader thread precisely so there is nothing else competing for
//! the tty while the child runs.

use std::io;
use std::path::PathBuf;
use std::process::Command;

/// `$VISUAL`, then `$EDITOR`, then `vi`.
///
/// The value may carry arguments (`code -w`, `emacsclient -nw`), so it is split
/// rather than executed whole; running it through a shell would break on paths
/// containing spaces and invite quoting bugs.
fn editor_command() -> (String, Vec<String>) {
    let spec = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| "vi".to_string());

    let mut parts = spec.split_whitespace().map(str::to_string);
    let program = parts.next().unwrap_or_else(|| "vi".to_string());
    (program, parts.collect())
}

fn scratch_path(task_id: &str) -> PathBuf {
    // `.md` so editors apply markdown highlighting; descriptions usually are.
    let name = format!("dextui-{}-{}.md", std::process::id(), task_id);
    std::env::temp_dir().join(name)
}

/// Opens `initial` in the user's editor and returns the result.
///
/// `Ok(None)` means the text came back unchanged, so the caller can skip the
/// write entirely rather than bumping `updated_at` for nothing.
///
/// The caller is responsible for leaving and restoring the alternate screen
/// around this; doing it here would tie the module to a particular backend.
pub fn edit(task_id: &str, initial: &str) -> io::Result<Option<String>> {
    let path = scratch_path(task_id);
    std::fs::write(&path, initial)?;

    let (program, args) = editor_command();
    let status = Command::new(&program).args(&args).arg(&path).status();

    let edited = match status {
        Ok(s) if s.success() => std::fs::read_to_string(&path)?,
        Ok(s) => {
            let _ = std::fs::remove_file(&path);
            return Err(io::Error::other(format!(
                "{program} exited with {s}; description left unchanged"
            )));
        }
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(io::Error::other(format!("could not run {program}: {e}")));
        }
    };

    let _ = std::fs::remove_file(&path);

    // Editors habitually add a trailing newline; that alone is not an edit.
    if edited.trim_end_matches('\n') == initial.trim_end_matches('\n') {
        Ok(None)
    } else {
        Ok(Some(edited.trim_end_matches('\n').to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `std::env::set_var` is unsafe in edition 2024 and these tests mutate
    /// process-wide state, so they run under one lock and restore what they set.
    fn with_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();

        let saved: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| ((*k).to_string(), std::env::var(k).ok()))
            .collect();

        for (k, v) in vars {
            match v {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }

        let out = f();

        for (k, v) in saved {
            match v {
                Some(v) => unsafe { std::env::set_var(&k, v) },
                None => unsafe { std::env::remove_var(&k) },
            }
        }
        out
    }

    #[test]
    fn visual_wins_over_editor() {
        with_env(&[("VISUAL", Some("hx")), ("EDITOR", Some("vim"))], || {
            assert_eq!(editor_command().0, "hx");
        });
    }

    #[test]
    fn editor_is_used_when_visual_is_unset() {
        with_env(&[("VISUAL", None), ("EDITOR", Some("vim"))], || {
            assert_eq!(editor_command().0, "vim");
        });
    }

    #[test]
    fn falls_back_to_vi_when_neither_is_set() {
        with_env(&[("VISUAL", None), ("EDITOR", None)], || {
            assert_eq!(editor_command().0, "vi");
        });
    }

    #[test]
    fn an_empty_or_blank_value_is_ignored() {
        // An exported-but-empty EDITOR is common and must not shadow the default.
        with_env(&[("VISUAL", Some("   ")), ("EDITOR", Some(""))], || {
            assert_eq!(editor_command().0, "vi");
        });
    }

    #[test]
    fn arguments_are_split_rather_than_shelled_out() {
        // `code -w` must run code with -w, not a shell string.
        with_env(&[("VISUAL", None), ("EDITOR", Some("code -w --new-window"))], || {
            let (program, args) = editor_command();
            assert_eq!(program, "code");
            assert_eq!(args, vec!["-w", "--new-window"]);
        });
    }

    #[test]
    fn the_scratch_file_is_markdown_and_names_the_task() {
        let p = scratch_path("abc123");
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.ends_with(".md"), "{name}");
        assert!(name.contains("abc123"), "{name}");
    }
}
