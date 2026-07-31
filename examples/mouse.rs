//! A mouse probe: what the *terminal* actually reports, with nothing else in
//! the way.
//!
//! ```bash
//! cargo run --example mouse
//! ```
//!
//! Deliberately raw crossterm rather than ratatui. When a mouse bug is being
//! chased in a ratatui app, the first question is whether the coordinates
//! arriving are the ones you think -- and a probe built on the same layer
//! being investigated cannot answer that.
//!
//! Two things it shows that are easy to get wrong:
//!
//! - **Both coordinate bases.** crossterm reports 0-based column/row, which is
//!   what `App::body_top`, `divider_x` and `select_at_row` all speak. The SGR
//!   escapes on the wire, and anything you send with `tmux send-keys`, are
//!   1-based. Mixing them is a one-cell error that looks exactly like a
//!   layout bug.
//! - **Motion with no button held**, which needs `?1003h`. crossterm's
//!   `EnableMouseCapture` only enables `?1002h` (motion *while dragging*), so
//!   without this the pointer would appear to teleport between clicks.
//!
//! `q` or `Esc` quits. Every terminal state it turns on is turned off again on
//! the way out, including on the error paths -- a probe that leaves your shell
//! in raw mode with the mouse captured is worse than no probe.

use std::io::{self, Write};
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    poll, read, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, queue};

/// How many events to keep on screen, newest first.
const HISTORY: usize = 14;

fn main() -> io::Result<()> {
    if std::env::args().any(|a| a == "--raw") {
        return raw();
    }
    let mut out = io::stdout();

    enable_raw_mode()?;
    execute!(out, EnterAlternateScreen, EnableMouseCapture, Hide)?;
    // Any-motion tracking, which `EnableMouseCapture` does not turn on. Sent
    // raw because crossterm has no wrapper for it.
    write!(out, "\x1b[?1003h")?;
    out.flush()?;

    let result = run(&mut out);

    // Restored in the reverse order, and unconditionally: `?1003l` first,
    // since it is the one crossterm does not know about and therefore will not
    // clean up for us.
    let _ = write!(out, "\x1b[?1003l");
    let _ = execute!(out, Show, DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();

    result
}

/// `--raw`: the literal bytes the terminal sends, with no parser in between.
///
/// The last thing that can be wrong when a coordinate is "wildly wrong" is the
/// parse, and everything above reads events through crossterm -- so it cannot
/// answer whether crossterm was handed something odd or produced something odd
/// from something ordinary. This reads stdin itself and escapes what arrives.
///
/// What to look for. A left press at column 60, row 6 (1-based, as the wire
/// counts) should appear as exactly:
///
/// ```text
/// ESC [ < 0 ; 60 ; 6 M
/// ```
///
/// If that is what arrives and the crosshair sits elsewhere, the fault is
/// above this line. If the numbers here are already wrong -- wildly large, say,
/// which is what pixel-resolution reporting (`?1016h`) looks like -- then it is
/// the terminal, and no amount of app-side arithmetic will fix it.
fn raw() -> io::Result<()> {
    use std::io::Read;

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnableMouseCapture)?;
    write!(out, "\x1b[?1003h")?;
    writeln!(out, "raw mouse bytes — move and click; ctrl-c or q to quit\r")?;
    out.flush()?;

    let mut buf = [0u8; 1024];
    let mut stdin = io::stdin();
    loop {
        let n = stdin.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if buf[..n].contains(&b'q') {
            break;
        }
        // One line per read, so a sequence split across reads is visible as a
        // split rather than silently rejoined -- that is itself a finding.
        let shown: String = buf[..n]
            .iter()
            .map(|b| match b {
                0x1b => "ESC ".to_string(),
                0x20..=0x7e => format!("{} ", *b as char),
                _ => format!("\\x{b:02x} "),
            })
            .collect();
        write!(out, "{shown}\r\n")?;
        out.flush()?;
    }

    let _ = write!(out, "\x1b[?1003l");
    let _ = execute!(out, DisableMouseCapture);
    let _ = disable_raw_mode();
    Ok(())
}

fn run(out: &mut io::Stdout) -> io::Result<()> {
    let mut log: Vec<String> = Vec::new();
    let mut pointer: Option<(u16, u16)> = None;
    let mut counts = (0u32, 0u32, 0u32); // clicks, scrolls, moves

    draw(out, pointer, &log, counts)?;

    loop {
        // Everything already queued is taken before redrawing.
        //
        // This matters far more here than in the app: `?1003h` reports motion
        // for every cell the pointer crosses, and `draw` repaints the entire
        // screen. One repaint per event caps throughput low enough that a fast
        // trackpad can outrun it, and the unread bytes then fill the 4 KB tty
        // input buffer until the kernel drops some -- a byte lost inside
        // `\e[<35;60;10M` leaves a truncated sequence, which crossterm reads
        // as a position nobody pointed at, or as the bare `\e` that quits.
        //
        // A probe that can misreport under load is worse than none, since it
        // is trusted precisely when something already looks impossible.
        loop {
            match read()? {
                Event::Key(k)
                    if k.kind == KeyEventKind::Press
                        && matches!(k.code, KeyCode::Char('q') | KeyCode::Esc) =>
                {
                    return Ok(());
                }

                Event::Mouse(m) => {
                    pointer = Some((m.column, m.row));
                    match m.kind {
                        MouseEventKind::Moved => counts.2 += 1,
                        MouseEventKind::ScrollUp
                        | MouseEventKind::ScrollDown
                        | MouseEventKind::ScrollLeft
                        | MouseEventKind::ScrollRight => counts.1 += 1,
                        _ => counts.0 += 1,
                    }
                    // Motion is noisy and says nothing on its own -- the
                    // crosshair already shows where the pointer is. Only
                    // events you *did* are worth a line.
                    if !matches!(m.kind, MouseEventKind::Moved) {
                        log.insert(0, describe(&m));
                        log.truncate(HISTORY);
                    }
                }

                _ => {}
            }

            if !poll(Duration::ZERO)? {
                break;
            }
        }

        draw(out, pointer, &log, counts)?;
    }
}

fn describe(m: &MouseEvent) -> String {
    let kind = match m.kind {
        MouseEventKind::Down(b) => format!("down {}", button(b)),
        MouseEventKind::Up(b) => format!("up   {}", button(b)),
        MouseEventKind::Drag(b) => format!("drag {}", button(b)),
        MouseEventKind::Moved => "moved".into(),
        MouseEventKind::ScrollUp => "wheel up".into(),
        MouseEventKind::ScrollDown => "wheel down".into(),
        MouseEventKind::ScrollLeft => "wheel left".into(),
        MouseEventKind::ScrollRight => "wheel right".into(),
    };
    let mods = if m.modifiers.is_empty() {
        String::new()
    } else {
        format!("  [{}]", m.modifiers)
    };
    // Both bases, every line: the 0-based pair is what the app's code sees,
    // the 1-based pair is what the wire carries and what `tmux send-keys`
    // wants back.
    format!(
        "{kind:<12} col {:>3}  row {:>3}   (1-based {},{}){mods}",
        m.column,
        m.row,
        m.column + 1,
        m.row + 1
    )
}

fn button(b: MouseButton) -> &'static str {
    match b {
        MouseButton::Left => "left",
        MouseButton::Middle => "middle",
        MouseButton::Right => "right",
    }
}

fn draw(
    out: &mut io::Stdout,
    pointer: Option<(u16, u16)>,
    log: &[String],
    counts: (u32, u32, u32),
) -> io::Result<()> {
    let (w, h) = crossterm::terminal::size()?;
    queue!(out, Clear(ClearType::All))?;

    // Rulers, so a reported coordinate can be read straight off the screen
    // rather than counted. Row 0 is the tens digit, row 1 the units.
    queue!(out, SetForegroundColor(Color::DarkGrey))?;
    let tens: String = (0..w)
        .map(|c| if c % 10 == 0 { char::from_digit((c as u32 / 10) % 10, 10).unwrap() } else { ' ' })
        .collect();
    let units: String = (0..w)
        .map(|c| char::from_digit((c as u32) % 10, 10).unwrap())
        .collect();
    queue!(out, MoveTo(0, 0), Print(tens))?;
    queue!(out, MoveTo(0, 1), Print(units))?;
    for r in 2..h {
        queue!(out, MoveTo(0, r), Print(format!("{:>3}", r)))?;
    }
    queue!(out, ResetColor)?;

    // Crosshair *before* the text, so the text wins where they overlap.
    // Drawn after, the horizontal line sat exactly on top of the newest log
    // line -- the one event you most want to read -- and the vertical line cut
    // every other line in half. A probe that hides its own output is worse
    // than one with a broken-looking crosshair.
    //
    // Full-width and full-height rather than a single cell: the question is
    // usually "is the column right, or the row", and one line each answers
    // them independently against the rulers.
    if let Some((c, r)) = pointer {
        queue!(out, SetForegroundColor(Color::DarkGrey))?;
        for x in 0..w {
            queue!(out, MoveTo(x, r), Print("─"))?;
        }
        for y in 2..h {
            queue!(out, MoveTo(c, y), Print("│"))?;
        }
        queue!(out, ResetColor)?;
    }

    let header = format!(
        " mouse probe — {w}x{h} — clicks {} · scrolls {} · moves {} — q to quit ",
        counts.0, counts.1, counts.2
    );
    queue!(
        out,
        MoveTo(5, 2),
        SetForegroundColor(Color::Black),
        SetBackgroundColor(Color::Magenta),
        Print(header),
        ResetColor
    )?;

    for (i, line) in log.iter().enumerate() {
        let y = 4 + i as u16;
        if y >= h {
            break;
        }
        // Newest brightest, so the eye lands on the event you just made.
        let colour = if i == 0 { Color::Yellow } else { Color::DarkGrey };
        queue!(out, MoveTo(5, y), SetForegroundColor(colour), Print(line), ResetColor)?;
    }

    // The marker itself goes on last: the lines may yield to the log, but
    // where the pointer *is* must never be hidden.
    if let Some((c, r)) = pointer {
        queue!(
            out,
            MoveTo(c, r),
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Magenta),
            Print("┼"),
            ResetColor
        )?;
        let label = format!(" {c},{r} ");
        // Kept on screen when the pointer is near the right edge.
        let lx = if c as usize + 1 + label.len() < w as usize {
            c + 1
        } else {
            c.saturating_sub(label.len() as u16)
        };
        queue!(
            out,
            MoveTo(lx, r.saturating_sub(1)),
            SetForegroundColor(Color::Magenta),
            Print(label),
            ResetColor
        )?;
    }

    out.flush()
}
