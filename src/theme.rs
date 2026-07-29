//! Every colour the app uses, and nothing else.
//!
//! `markdown` and `tree` describe what things *are*; `ui` decides how they look
//! and takes the values from here. Keeping them in one place is what makes the
//! colour policy checkable rather than aspirational -- see the test in `ui` that
//! walks [`ALL`].
//!
//! **Only ANSI-16 names and `Reset` belong here.** This machine's terminal is
//! configured `theme = light:"...",dark:"..."`, so it follows the macOS
//! appearance and flips *under the running app*. ANSI names are remapped by the
//! user's theme; `Indexed` and `Rgb` are fixed values that can only ever suit
//! one background. `White`/`Black` for text are effectively fixed too: an
//! earlier version rendered the task title white-on-white in light mode.

use ratatui::style::Color;

/// Whatever the terminal's default foreground is.
pub const PLAIN: Color = Color::Reset;
/// Secondary text: tree connectors, timestamps, the untouched part of a meter.
pub const DIM: Color = Color::DarkGray;

/// The four task states, matching the dex CLI exactly so the two tools cannot
/// contradict each other. Taken from dex 0.16.0 `dist/cli/formatting.js`
/// (`getTaskStatusDisplay`) and `~/.local/bin/dex-report` (`glyphcolor`):
///
/// ```text
/// todo         yellow  33      in progress  blue   34
/// done         green   32      blocked      red    31
/// ```
///
/// These land on the **status glyph only**, never the task name -- which is
/// what stops a mostly-unstarted tree becoming a wall of yellow, and is what
/// dex itself does.
pub const TODO: Color = Color::Yellow;
pub const ACTIVE: Color = Color::Blue;
pub const DONE: Color = Color::Green;
pub const BLOCKED: Color = Color::Red;

/// The bright end of the in-progress breath. In-progress markers alternate
/// between [`ACTIVE`] and this, bolded, every `pulse::HALF_PERIOD`; the glyph
/// itself never changes shape, so the marker column cannot jitter.
pub const ACTIVE_PULSE: Color = Color::LightBlue;

/// Inline code and other literal spans in rendered markdown.
pub const CODE: Color = Color::Cyan;

/// The selection gutter: where you are, as opposed to what anything *is*.
///
/// Magenta is the one ANSI hue carrying no meaning here -- the four states take
/// yellow, blue, green and red, and [`CODE`] takes cyan -- so a selected row can
/// never be misread as a state. A test asserts that separation.
///
/// The unfocused pane dims *within the hue* rather than falling back to [`DIM`],
/// which is exactly the colour of the `│└├` tree connectors sitting one cell to
/// the gutter's right; an unfocused cursor would disappear into them.
pub const ACCENT: Color = Color::LightMagenta;
pub const ACCENT_DIM: Color = Color::Magenta;

/// Every colour above, for the policy test. A colour missing from this list is
/// unguarded, so add new ones here as well.
#[cfg(test)]
pub const ALL: [(&str, Color); 10] = [
    ("PLAIN", PLAIN),
    ("DIM", DIM),
    ("TODO", TODO),
    ("ACTIVE", ACTIVE),
    ("ACTIVE_PULSE", ACTIVE_PULSE),
    ("DONE", DONE),
    ("BLOCKED", BLOCKED),
    ("CODE", CODE),
    ("ACCENT", ACCENT),
    ("ACCENT_DIM", ACCENT_DIM),
];
