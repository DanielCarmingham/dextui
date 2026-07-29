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
//! The tier comes from the config file or `DEXTUI_ICONS`; `config` owns that
//! precedence. The default is `unicode`, because a Nerd Font cannot be reliably
//! detected at runtime and guessing wrong yields a screen full of tofu.

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

    // Meter
    pub meter_done: &'static str,
    pub meter_active: &'static str,
    pub meter_empty: &'static str,
}

/// Nerd Font: chevrons and Font Awesome state icons.
pub const NERD: Icons = Icons {
    tier: Tier::Nerd,
    expanded: "\u{f078}",  // fa chevron-down
    collapsed: "\u{f054}", // fa chevron-right
    leaf: " ",
    pending: "\u{f070c}", // md-rhombus_outline
    active: "\u{f04b}",   // fa-play
    done: "\u{f070b}",    // md-rhombus
    blocked: "\u{f05e}",  // fa-ban
    app: "\u{f0ae}",     // fa tasks
    project: "\u{f07b}", // fa folder
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
    pending: "\u{25c7}", // ◇
    active: "\u{25ba}",  // ►  (NOT ▶ U+25B6 -- that is `collapsed`)
    done: "\u{25c6}",    // ◆
    blocked: "\u{00d7}", // ×  (✗ and ⊗ are unavailable)
    app: "",
    project: "",
    meter_done: "\u{2593}",
    meter_active: "\u{2592}",
    meter_empty: "\u{2591}",
};

/// Nothing above 7-bit. For terminals or fonts where the rest cannot be trusted.
pub const ASCII: Icons = Icons {
    tier: Tier::Ascii,
    // `-`/`+` rather than `v`/`>`: the play marker needs `>`, and a collapsed
    // in-progress row would otherwise draw `> >`. The pair is the conventional
    // file-tree one anyway.
    expanded: "-",
    collapsed: "+",
    leaf: " ",
    pending: ".",
    active: ">",
    done: "x",
    blocked: "!",
    app: "",
    project: "",
    meter_done: "#",
    meter_active: "+",
    meter_empty: ".",
};

impl Icons {
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
        Tier::Nerd => "Nerd Font icons (needs a patched font)",
        Tier::Unicode => "plain Unicode; works in any modern font",
        Tier::Ascii => "7-bit only; for anywhere else",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn states(ic: &Icons) -> [(&'static str, &'static str); 4] {
        [
            ("pending", ic.pending),
            ("active", ic.active),
            ("done", ic.done),
            ("blocked", ic.blocked),
        ]
    }

    /// The in-progress marker is a play glyph, and `►` U+25BA is deliberately
    /// **not** `▶` U+25B6 -- U+25B6 is already the collapsed-node marker, and a
    /// rendered comparison showed the two are nearly indistinguishable at
    /// terminal size. They are told apart by colour and weight (dim connector,
    /// coloured status), never by shape. Swapping one for the other makes a
    /// collapsed in-progress row read as two identical triangles.
    #[test]
    fn in_progress_is_a_play_marker_distinct_from_the_collapsed_node() {
        assert_eq!(UNICODE.active, "\u{25ba}");
        assert_eq!(NERD.active, "\u{f04b}"); // fa-play
        assert_eq!(ASCII.active, ">");

        for ic in ALL {
            assert_ne!(
                ic.active,
                ic.collapsed,
                "tier {}: in-progress collides with the collapsed marker",
                name(ic.tier)
            );
        }
    }

    /// The tree marker and the status glyph sit side by side, so a glyph serving
    /// both roles makes the pair ambiguous -- `> >` for a collapsed in-progress
    /// row. Colour separates them (dim connector, coloured status) but the shape
    /// must too, for anyone reading without colour.
    #[test]
    fn no_state_glyph_doubles_as_a_tree_marker() {
        for ic in ALL {
            for (state, g) in states(&ic) {
                for (role, marker) in [("expanded", ic.expanded), ("collapsed", ic.collapsed)] {
                    assert_ne!(
                        g,
                        marker,
                        "tier {}: {state} and the {role} marker are both {g:?}",
                        name(ic.tier)
                    );
                }
            }
        }
    }

    /// Todo and done are the same shape, hollow then filled, so the state change
    /// is carried by the glyph itself and not by colour alone.
    #[test]
    fn todo_fills_in_when_done() {
        assert_eq!(UNICODE.pending, "\u{25c7}"); // hollow diamond
        assert_eq!(UNICODE.done, "\u{25c6}"); // filled diamond
        assert_eq!(NERD.pending, "\u{f070c}"); // md-rhombus_outline
        assert_eq!(NERD.done, "\u{f070b}"); // md-rhombus
    }

    /// A blank marker leaves the reader guessing whether the row has a state at
    /// all. Every state gets something visible in every tier.
    #[test]
    fn every_state_has_a_visible_marker_in_every_tier() {
        for ic in ALL {
            for (state, g) in states(&ic) {
                assert!(
                    !g.trim().is_empty(),
                    "tier {}: {state} has no visible marker",
                    name(ic.tier)
                );
            }
        }
    }

    #[test]
    fn states_are_distinguishable_within_a_tier() {
        for ic in ALL {
            let s = states(&ic);
            for i in 0..s.len() {
                for j in (i + 1)..s.len() {
                    assert_ne!(
                        s[i].1,
                        s[j].1,
                        "tier {}: {} and {} draw the same glyph",
                        name(ic.tier),
                        s[i].0,
                        s[j].0
                    );
                }
            }
        }
    }

    /// The tree is a column: every row's name starts at the same offset. A
    /// two-char marker in one state would shift that row alone.
    #[test]
    fn every_marker_is_exactly_one_character() {
        for ic in ALL {
            for (state, g) in states(&ic) {
                assert_eq!(
                    g.chars().count(),
                    1,
                    "tier {}: {state} is {g:?}, not a single character",
                    name(ic.tier)
                );
            }
        }
    }

    /// The whole point of the ascii tier is terminals and fonts where nothing
    /// above 7-bit can be trusted.
    #[test]
    fn the_ascii_tier_stays_7_bit() {
        for (state, g) in states(&ASCII) {
            assert!(
                g.is_ascii(),
                "ascii tier: {state} is {g:?}, which is not 7-bit"
            );
        }
        for g in [ASCII.expanded, ASCII.collapsed, ASCII.leaf, ASCII.meter_done] {
            assert!(g.is_ascii(), "ascii tier: {g:?} is not 7-bit");
        }
    }
}

