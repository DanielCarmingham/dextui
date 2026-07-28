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

    // Progress meter
    pub meter_full: Color,
    pub meter_empty: Color,

    // Description markup
    pub md_marker: Color,
    pub md_heading: Color,
    pub md_code: Color,
    pub md_quote: Color,
}

/// What shipped first: bright cyan chrome competing with the content.
pub const CURRENT: Palette = Palette {
    name: "current",
    about: "the original: cyan chrome, coloured everywhere",
    pending: Color::Reset,
    active: Color::Yellow,
    done: Color::Green,
    blocked: Color::Red,
    chrome: Color::Cyan,
    label: Color::DarkGray,
    title: Color::Reset,
    selection: Color::Indexed(238),
    meter_full: Color::Green,
    meter_empty: Color::DarkGray,
    md_marker: Color::DarkGray,
    md_heading: Color::Reset,
    md_code: Color::Cyan,
    md_quote: Color::DarkGray,
};

/// Same colours, but the frame stops shouting: chrome recedes to grey so the
/// only saturated things on screen are task state and the meter.
pub const CALM: Palette = Palette {
    name: "calm",
    about: "chrome recedes to grey; state keeps its colours",
    pending: Color::Reset,
    active: Color::Yellow,
    done: Color::Green,
    blocked: Color::Red,
    chrome: Color::DarkGray,
    label: Color::DarkGray,
    title: Color::White,
    selection: Color::Indexed(237),
    meter_full: Color::Green,
    meter_empty: Color::Indexed(238),
    md_marker: Color::DarkGray,
    md_heading: Color::White,
    md_code: Color::Cyan,
    md_quote: Color::DarkGray,
};

/// Structure runs cool and dim, live work runs warm. Temperature carries the
/// hierarchy so hue is not doing two jobs at once.
pub const TEMPERATURE: Palette = Palette {
    name: "temperature",
    about: "cool dim structure, warm active work",
    pending: Color::Indexed(250),
    active: Color::Indexed(215),
    done: Color::Indexed(108),
    blocked: Color::Indexed(174),
    chrome: Color::Indexed(238),
    label: Color::Indexed(243),
    title: Color::Indexed(253),
    selection: Color::Indexed(236),
    meter_full: Color::Indexed(215),
    meter_empty: Color::Indexed(237),
    md_marker: Color::Indexed(240),
    md_heading: Color::Indexed(253),
    md_code: Color::Indexed(109),
    md_quote: Color::Indexed(245),
};

/// Fixed hues via truecolor. Precise and consistent everywhere, at the cost of
/// ignoring the user's terminal theme -- assumes a dark background.
pub const EMBER: Palette = Palette {
    name: "ember",
    about: "fixed truecolor; ignores your terminal theme, assumes dark",
    pending: Color::Rgb(0xC8, 0xC6, 0xC0),
    active: Color::Rgb(0xE8, 0x9B, 0x3C),
    done: Color::Rgb(0x6E, 0x9E, 0x78),
    blocked: Color::Rgb(0xC7, 0x5A, 0x5A),
    chrome: Color::Rgb(0x3A, 0x38, 0x36),
    label: Color::Rgb(0x7A, 0x75, 0x6E),
    title: Color::Rgb(0xF2, 0xEF, 0xE8),
    selection: Color::Rgb(0x2A, 0x27, 0x24),
    meter_full: Color::Rgb(0xE8, 0x9B, 0x3C),
    meter_empty: Color::Rgb(0x3A, 0x38, 0x36),
    md_marker: Color::Rgb(0x6A, 0x66, 0x60),
    md_heading: Color::Rgb(0xF2, 0xEF, 0xE8),
    md_code: Color::Rgb(0x8F, 0xAE, 0xB8),
    md_quote: Color::Rgb(0x8A, 0x85, 0x7E),
};

pub const ALL: [Palette; 4] = [CURRENT, CALM, TEMPERATURE, EMBER];

/// Resolves `DEXTUI_THEME`, falling back to the default on an unknown name.
pub fn from_env() -> Palette {
    match std::env::var("DEXTUI_THEME").ok().as_deref() {
        Some("current") => CURRENT,
        Some("temperature") => TEMPERATURE,
        Some("ember") => EMBER,
        _ => CALM,
    }
}
