//! Every colour decision, in one table.
//!
//! Two kinds of colour are available and the choice matters. Named/indexed
//! colours (`Color::Yellow`, `Color::Indexed(n)`) resolve through the user's own
//! terminal theme, so the app looks native in whatever scheme they run — but you
//! do not control the exact hue. `Color::Rgb` gives you the precise colour and
//! ignores their theme entirely, which can clash badly with a light background.
//!
//! Default is theme-adaptive. Pick with `DEXTUI_THEME=<name>`.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub name: &'static str,
    /// One line on how the palette is meant to read.
    pub about: &'static str,

    // Task state
    pub pending: Color,
    pub active: Color,
    pub done: Color,
    pub blocked: Color,

    // Structure
    pub chrome: Color,
    pub label: Color,
    pub title: Color,
    pub selection: Color,
    /// Render the selected row with REVERSED instead of a fixed background.
    /// A fixed dark band is unreadable on a light terminal; inverting whatever
    /// the current colours are is correct in both, with no detection needed.
    pub reverse_selection: bool,

    // Progress meter
    pub meter_full: Color,
    pub meter_empty: Color,

    // Header segments. Backgrounds so powerline separators have something to
    // cut between; foregrounds are chosen for contrast against them.
    pub head_app_bg: Color,
    pub head_app_fg: Color,
    pub head_ctx_bg: Color,
    pub head_ctx_fg: Color,
    pub head_info_bg: Color,
    pub head_info_fg: Color,

    // Description markup
    pub md_marker: Color,
    pub md_heading: Color,
    pub md_code: Color,
    pub md_quote: Color,
}

/// What shipped first: bright cyan chrome competing with the content.
/// Named "original" rather than "current", which read as "the one in use".
pub const ORIGINAL: Palette = Palette {
    name: "original",
    about: "the first palette shipped: cyan chrome, colour everywhere",
    pending: Color::Reset,
    active: Color::Yellow,
    done: Color::Green,
    blocked: Color::Red,
    chrome: Color::Cyan,
    label: Color::DarkGray,
    title: Color::Reset,
    selection: Color::Indexed(238),
    reverse_selection: false,
    meter_full: Color::Green,
    meter_empty: Color::DarkGray,
    head_app_bg: Color::Cyan,
    head_app_fg: Color::Black,
    head_ctx_bg: Color::Indexed(238),
    head_ctx_fg: Color::White,
    head_info_bg: Color::Indexed(235),
    head_info_fg: Color::Gray,
    md_marker: Color::DarkGray,
    md_heading: Color::Reset,
    md_code: Color::Cyan,
    md_quote: Color::DarkGray,
};

/// The default, and the only palette that is correct in BOTH light and dark
/// terminals. Uses nothing but `Reset` and the ANSI-16 names, which the user's
/// own terminal theme remaps per mode, plus REVERSED for the selected row.
///
/// Deliberately restrained rather than colourless: chrome recedes to grey so it
/// stops competing with content, while task state keeps its colour.
pub const CALM: Palette = Palette {
    name: "calm",
    about: "adapts to light and dark terminals (default)",
    pending: Color::Reset,
    active: Color::Yellow,
    done: Color::Green,
    blocked: Color::Red,
    chrome: Color::DarkGray,
    label: Color::DarkGray,
    // Never White: that is invisible on a light background.
    title: Color::Reset,
    selection: Color::Reset,
    reverse_selection: true,
    meter_full: Color::Green,
    meter_empty: Color::DarkGray,
    head_app_bg: Color::Blue,
    head_app_fg: Color::White,
    head_ctx_bg: Color::DarkGray,
    head_ctx_fg: Color::White,
    head_info_bg: Color::Reset,
    head_info_fg: Color::DarkGray,
    md_marker: Color::DarkGray,
    md_heading: Color::Reset,
    md_code: Color::Cyan,
    md_quote: Color::DarkGray,
};

/// Structure runs cool and dim, live work runs warm. Temperature carries the
/// hierarchy so hue is not doing two jobs at once.
pub const TEMPERATURE: Palette = Palette {
    name: "temperature",
    about: "cool dim structure, warm active work (dark terminals only)",
    pending: Color::Indexed(250),
    active: Color::Indexed(215),
    done: Color::Indexed(108),
    blocked: Color::Indexed(174),
    chrome: Color::Indexed(238),
    label: Color::Indexed(243),
    title: Color::Indexed(253),
    selection: Color::Indexed(236),
    reverse_selection: false,
    meter_full: Color::Indexed(215),
    meter_empty: Color::Indexed(237),
    head_app_bg: Color::Indexed(215),
    head_app_fg: Color::Indexed(235),
    head_ctx_bg: Color::Indexed(239),
    head_ctx_fg: Color::Indexed(253),
    head_info_bg: Color::Indexed(236),
    head_info_fg: Color::Indexed(245),
    md_marker: Color::Indexed(240),
    md_heading: Color::Indexed(253),
    md_code: Color::Indexed(109),
    md_quote: Color::Indexed(245),
};

/// Fixed hues via truecolor. Precise and consistent everywhere, at the cost of
/// ignoring the user's terminal theme -- assumes a dark background.
pub const EMBER: Palette = Palette {
    name: "ember",
    about: "fixed truecolor; ignores your terminal theme (dark only)",
    pending: Color::Rgb(0xC8, 0xC6, 0xC0),
    active: Color::Rgb(0xE8, 0x9B, 0x3C),
    done: Color::Rgb(0x6E, 0x9E, 0x78),
    blocked: Color::Rgb(0xC7, 0x5A, 0x5A),
    chrome: Color::Rgb(0x3A, 0x38, 0x36),
    label: Color::Rgb(0x7A, 0x75, 0x6E),
    title: Color::Rgb(0xF2, 0xEF, 0xE8),
    selection: Color::Rgb(0x2A, 0x27, 0x24),
    reverse_selection: false,
    meter_full: Color::Rgb(0xE8, 0x9B, 0x3C),
    meter_empty: Color::Rgb(0x3A, 0x38, 0x36),
    head_app_bg: Color::Rgb(0xE8, 0x9B, 0x3C),
    head_app_fg: Color::Rgb(0x1E, 0x1C, 0x1A),
    head_ctx_bg: Color::Rgb(0x3A, 0x38, 0x36),
    head_ctx_fg: Color::Rgb(0xF2, 0xEF, 0xE8),
    head_info_bg: Color::Rgb(0x2A, 0x27, 0x24),
    head_info_fg: Color::Rgb(0x9A, 0x95, 0x8E),
    md_marker: Color::Rgb(0x6A, 0x66, 0x60),
    md_heading: Color::Rgb(0xF2, 0xEF, 0xE8),
    md_code: Color::Rgb(0x8F, 0xAE, 0xB8),
    md_quote: Color::Rgb(0x8A, 0x85, 0x7E),
};

pub const ALL: [Palette; 4] = [CALM, ORIGINAL, TEMPERATURE, EMBER];

/// Resolves `DEXTUI_THEME`, falling back to the default on an unknown name.
pub fn from_env() -> Palette {
    match std::env::var("DEXTUI_THEME").ok().as_deref() {
        Some("original") => ORIGINAL,
        Some("temperature") => TEMPERATURE,
        Some("ember") => EMBER,
        _ => CALM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Colours that resolve through the user's terminal theme, so they are
    /// correct whether the background is light or dark. `Indexed` and `Rgb` are
    /// fixed values and cannot be; `White`/`Black` are effectively fixed too.
    fn is_adaptive(c: Color) -> bool {
        !matches!(c, Color::Indexed(_) | Color::Rgb(..))
    }

    #[test]
    fn the_default_palette_works_on_light_and_dark_terminals() {
        // Regression: `calm` shipped with title = White, which is invisible on a
        // light background, and a fixed dark selection band that made the
        // selected row unreadable. Ghostty here follows the macOS appearance, so
        // a palette that assumes one background is wrong half the time.
        let p = CALM;
        for (name, c) in [
            ("pending", p.pending),
            ("active", p.active),
            ("done", p.done),
            ("blocked", p.blocked),
            ("chrome", p.chrome),
            ("label", p.label),
            ("title", p.title),
            ("meter_full", p.meter_full),
            ("meter_empty", p.meter_empty),
            ("md_marker", p.md_marker),
            ("md_heading", p.md_heading),
            ("md_code", p.md_code),
            ("md_quote", p.md_quote),
        ] {
            assert!(is_adaptive(c), "{name} is a fixed colour: {c:?}");
        }

        assert_ne!(p.title, Color::White, "White is invisible on light");
        assert_ne!(p.md_heading, Color::White, "White is invisible on light");
        assert!(
            p.reverse_selection,
            "the default must invert the selection rather than paint a fixed band"
        );
    }

    #[test]
    fn from_env_defaults_to_the_adaptive_palette() {
        // Unknown or unset values must land on the safe one, never a dark-only.
        assert_eq!(from_env().name, CALM.name);
    }

    #[test]
    fn every_palette_is_listed_and_named_uniquely() {
        let mut names: Vec<&str> = ALL.iter().map(|p| p.name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate palette name");
        assert!(ALL.iter().any(|p| p.name == CALM.name));
    }

    #[test]
    fn dark_only_palettes_say_so() {
        // They are legitimate choices, but must not surprise a light-mode user.
        for p in [TEMPERATURE, EMBER] {
            assert!(
                p.about.contains("dark"),
                "{} does not warn that it is dark-only",
                p.name
            );
        }
    }
}
