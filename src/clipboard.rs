//! Copy text to the clipboard from inside the TUI.
//!
//! Two transports, both always attempted, because neither one covers every
//! terminal this runs in:
//!
//! - **OSC 52**, written to stdout. The terminal you are *looking at* puts
//!   the text on its own clipboard, so it works over SSH -- the phone-over-SSH
//!   case this app is deliberately usable in -- and through tmux, whose
//!   default `set-clipboard external` forwards it to the outer terminal.
//!   Terminal.app ignores it.
//! - **A native tool** (`pbcopy`, `wl-copy`, `xclip`, `xsel`), when one is on
//!   `PATH`. Local runs get the OS clipboard even in a terminal that drops
//!   OSC 52; over SSH it lands on the remote machine, which is harmless.
//!
//! Neither can report whether the terminal accepted the text, so `copy`
//! returns nothing and the caller says what was *sent* rather than claiming
//! success. A native spawn that fails is logged and otherwise silent: the
//! OSC 52 write has already happened, and a status-bar complaint about a tool
//! the user may not even have would be noise.

use std::io::{self, Write};
use std::process::{Command, Stdio};

/// The escape that asks the terminal to set its clipboard: `ESC ] 52 ; c ;
/// <base64> BEL`. `c` is the system clipboard selection; `BEL` terminates as
/// widely as `ESC \` and survives more multiplexers.
pub fn osc52(text: &str) -> Vec<u8> {
    let mut out = b"\x1b]52;c;".to_vec();
    out.extend_from_slice(base64(text.as_bytes()).as_bytes());
    out.push(0x07);
    out
}

/// Standard base64 with padding (RFC 4648 section 4). Hand-rolled because it
/// is twenty lines and the alternative is a crate for one call site.
pub fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | (b as u32) << (16 - 8 * i));
        let sextets = [n >> 18, n >> 12, n >> 6, n];
        for (i, s) in sextets.iter().enumerate() {
            if i <= chunk.len() {
                out.push(TABLE[(s & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The native clipboard writer to fall back on, as a ready-to-spawn command,
/// or `None` when nothing suitable is on `PATH`. Candidates are tried in
/// order, so on Linux a Wayland session with `wl-copy` wins over an `xclip`
/// left over from X.
pub fn native_tool() -> Option<Command> {
    native_tool_from(&candidates(), on_path)
}

fn candidates() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        vec![("pbcopy", &[])]
    } else {
        vec![
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    }
}

fn native_tool_from(
    candidates: &[(&str, &[&str])],
    exists: impl Fn(&str) -> bool,
) -> Option<Command> {
    candidates
        .iter()
        .find(|(name, _)| exists(name))
        .map(|(name, args)| {
            let mut cmd = Command::new(name);
            cmd.args(*args);
            cmd
        })
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Send `text` to the clipboard by every route available. Must be called
/// between frames, never during a draw: it writes to the same stdout ratatui
/// draws on.
pub fn copy(text: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(&osc52(text));
    let _ = stdout.flush();
    drop(stdout);

    let Some(mut cmd) = native_tool() else {
        crate::log::line("clip", &format!("osc52 only, {} bytes", text.len()));
        return;
    };
    let tool = cmd.get_program().to_string_lossy().into_owned();
    // stdout and stderr are closed, not inherited: xclip forks and holds the
    // selection, and an inherited stdout would hand it the TUI's terminal.
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let outcome = cmd.spawn().and_then(|mut child| {
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()
    });
    match outcome {
        Ok(status) if status.success() => {
            crate::log::line("clip", &format!("osc52 + {tool}, {} bytes", text.len()))
        }
        Ok(status) => crate::log::line("clip", &format!("{tool} exited {status}")),
        Err(e) => crate::log::line("clip", &format!("{tool} failed to run: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4648 section 10 test vectors, which cover every padding case.
    #[test]
    fn base64_matches_the_rfc_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (input, want) in cases {
            assert_eq!(base64(input.as_bytes()), want, "input {input:?}");
        }
    }

    #[test]
    fn base64_handles_high_bytes_and_multiline_text() {
        // "é\n" -- non-ASCII and a newline, both routine in a description.
        assert_eq!(base64("é\n".as_bytes()), "w6kK");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    /// The exact bytes, because a terminal that receives a malformed OSC 52
    /// does nothing at all rather than complaining.
    #[test]
    fn osc52_is_esc_bracket_52_c_base64_bel() {
        assert_eq!(osc52("hi"), b"\x1b]52;c;aGk=\x07");
        assert_eq!(osc52(""), b"\x1b]52;c;\x07");
    }

    #[test]
    fn the_first_native_tool_on_path_wins_and_none_means_none() {
        let candidates: &[(&str, &[&str])] =
            &[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])];
        let cmd = native_tool_from(candidates, |name| name == "xclip").unwrap();
        assert_eq!(cmd.get_program(), "xclip");
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-selection", "clipboard"]);

        let cmd = native_tool_from(candidates, |_| true).unwrap();
        assert_eq!(cmd.get_program(), "wl-copy");

        assert!(native_tool_from(candidates, |_| false).is_none());
    }
}
