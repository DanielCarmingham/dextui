//! Glyph sets, in three tiers.
//!
//! Every codepoint here was verified against FiraCode Nerd Font with fontTools
//! rather than taken from a cheat sheet — which is how we found that `▾` U+25BE,
//! `▸` U+25B8 and `⊘` U+2298 are **absent** from that font despite looking like
//! safe box-drawing characters. macOS silently font-fallbacks them, so they
//! rendered at the wrong width. They are not used anywhere any more.
//!
//! Note that `✗` U+2717, `✘` U+2718 and `⊗` U+2297 are also missing, so there is
//! no pure-Unicode cross available; the unicode tier uses `×` U+00D7.
//!
//! Tier is chosen with `DEXTUI_ICONS=nerd|unicode|ascii`. The default is
//! `unicode`, because a Nerd Font cannot be reliably detected at runtime and
//! guessing wrong yields a screen full of tofu.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Nerd,
    Unicode,
    Ascii,
}

#[derive(Debug, Clone, Copy)]
pub struct Icons {
    pub tier: Tier,

    // Tree structure
    pub expanded: &'static str,
    pub collapsed: &'static str,
    pub leaf: &'static str,

    // Task state
    pub pending: &'static str,
    pub active: &'static str,
    pub done: &'static str,
    pub blocked: &'static str,

    // Header
    pub app: &'static str,
    pub project: &'static str,
    /// Solid powerline separator, or empty when unavailable.
    pub sep: &'static str,

    // Meter
    pub meter_done: &'static str,
    pub meter_active: &'static str,
    pub meter_empty: &'static str,
}

/// Nerd Font: chevrons, Font Awesome state icons, powerline separators.
pub const NERD: Icons = Icons {
    tier: Tier::Nerd,
    expanded: "\u{f078}",  // fa chevron-down
    collapsed: "\u{f054}", // fa chevron-right
    leaf: " ",
    pending: "\u{f10c}", // fa circle-o
    active: "\u{f192}",  // fa dot-circle-o
    done: "\u{f00c}",    // fa check
    blocked: "\u{f05e}", // fa ban
    app: "\u{f0ae}",     // fa tasks
    project: "\u{f07b}", // fa folder
    sep: "\u{e0b0}",     // powerline right solid
    meter_done: "\u{2593}",
    meter_active: "\u{2592}",
    meter_empty: "\u{2591}",
};

/// Plain Unicode, verified present in FiraCode Nerd Font.
pub const UNICODE: Icons = Icons {
    tier: Tier::Unicode,
    expanded: "\u{25bc}",  // ▼
    collapsed: "\u{25b6}", // ▶
    leaf: " ",
    pending: "\u{25cb}", // ○
    active: "\u{25d0}",  // ◐
    done: "\u{2713}",    // ✓
    blocked: "\u{00d7}", // ×  (✗ and ⊗ are unavailable)
    app: "",
    project: "",
    sep: "",
    meter_done: "\u{2593}",
    meter_active: "\u{2592}",
    meter_empty: "\u{2591}",
};

/// Nothing above 7-bit. For terminals or fonts where the rest cannot be trusted.
pub const ASCII: Icons = Icons {
    tier: Tier::Ascii,
    expanded: "v",
    collapsed: ">",
    leaf: " ",
    pending: " ",
    active: "~",
    done: "x",
    blocked: "!",
    app: "",
    project: "",
    sep: "",
    meter_done: "#",
    meter_active: "+",
    meter_empty: ".",
};

impl Icons {
    /// True when powerline separators can be drawn.
    pub fn has_powerline(&self) -> bool {
        !self.sep.is_empty()
    }

    pub fn marker(&self, has_children: bool, is_open: bool) -> &'static str {
        if !has_children {
            self.leaf
        } else if is_open {
            self.expanded
        } else {
            self.collapsed
        }
    }
}

pub const ALL: [Icons; 3] = [NERD, UNICODE, ASCII];

pub fn name(tier: Tier) -> &'static str {
    match tier {
        Tier::Nerd => "nerd",
        Tier::Unicode => "unicode",
        Tier::Ascii => "ascii",
    }
}

pub fn about(tier: Tier) -> &'static str {
    match tier {
        Tier::Nerd => "Nerd Font icons and powerline separators (needs a patched font)",
        Tier::Unicode => "plain Unicode; works in any modern font",
        Tier::Ascii => "7-bit only; for anywhere else",
    }
}

/// Resolves `DEXTUI_ICONS`, defaulting to the tier that cannot produce tofu.
pub fn from_env() -> Icons {
    match std::env::var("DEXTUI_ICONS").ok().as_deref() {
        Some("nerd") => NERD,
        Some("ascii") => ASCII,
        _ => UNICODE,
    }
}
