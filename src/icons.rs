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
//! **The spinner is braille, and the reasoning that once forbade it was wrong
//! in an instructive way.** FiraCode Nerd Font contains no braille at all — 0 of
//! its 11,992 codepoints — so `⠋⠙⠹`, what `ora` and therefore yarn and npm use,
//! is font-fallbacked by macOS to Apple Braille, whose advance measures **1.111
//! cells**. That measurement is correct and was taken twice.
//!
//! The inference drawn from it was not. An advance is what the *font* asks for;
//! a terminal lays out its own fixed grid and snaps the glyph into one cell
//! regardless, so the marker column does not move. Confirmed by eye in Ghostty
//! against `scripts/glyph-check.py`, whose `|` bars would go ragged if any of
//! this were mis-measured. The same snapping is why `done` (U+F070B, a
//! *double*-width Material Design glyph at 2.000 cells) has always looked right.
//!
//! So: measure the font, but verify against the terminal. The number was never
//! in doubt; what it implied was.
//!
//! If a terminal that honours the advance ever turns up, the fallbacks are
//! ready — `ASCII_SPIN` for any font, and Nerd Fonts 3.3.0's 6-frame arc at
//! U+EE06–EE0B, native at exactly 1.00 cells but present in the nerd tier alone.
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
    /// The still marker for in-progress work: used wherever the state has to be
    /// named rather than watched -- the header counts, the help, the legend --
    /// and as the resting frame when `animate` is off. Always `spin[0]`.
    pub active: &'static str,
    /// The rotation, one glyph per frame. Every frame must measure exactly one
    /// cell in the tier's font, or the marker column shifts as it turns.
    pub spin: &'static [&'static str],
    pub done: &'static str,
    pub blocked: &'static str,

    // Header
    pub app: &'static str,
    pub project: &'static str,

    /// The selected row's left-margin rail. A reserved column on every row,
    /// painted only on the selected one.
    pub gutter: &'static str,

    // Meter
    pub meter: Meter,
}

/// The inline progress meter's glyph table.
///
/// The cell arithmetic is shared across every tier; only this table and whether
/// `partial` is populated differ. Each run is `[left cap, middle, right cap]`,
/// indexed by the cell's position in the bar, so a tier that draws a real
/// capped bar (nerd) and one that stamps the same block seven times (unicode,
/// ascii) go through identical code.
#[derive(Debug, Clone, Copy)]
pub struct Meter {
    pub done: [&'static str; 3],
    pub active: [&'static str; 3],
    pub empty: [&'static str; 3],
    /// Left-aligned fractions, 1/8 .. 7/8, indexed `partial[eighths - 1]`.
    /// Empty in tiers that have no sub-cell glyphs, which makes the bar snap to
    /// whole cells there. A capped tier must leave this empty -- a fraction on
    /// the last cell would eat the right cap.
    pub partial: &'static [&'static str],
}

/// The braille "dots" rotation, as `cli-spinners` defines it and therefore what
/// ora, yarn and npm all show. Ten frames at [`crate::pulse::FRAME`].
///
/// **These are not in FiraCode Nerd Font.** The U+2800 block is entirely absent
/// from it -- 0 of its 11,992 codepoints -- so macOS substitutes AppleBraille,
/// whose advance measures **1.111 cells**. That was measured with CoreText, and
/// it is why braille was rejected twice before.
///
/// It is used anyway because a terminal snaps a glyph into its own cell rather
/// than honouring the font's advance, so the marker column does not actually
/// move. That was verified by eye in Ghostty, not reasoned about -- run
/// `scripts/glyph-check.py` and look at whether the `|` bars line up.
///
/// If a terminal ever *does* honour the advance, these will draw 11% wide.
/// [`ASCII_SPIN`] is the safe fallback, and the nerd tier could use U+EE06..EE0B
/// instead, which are native FiraCode at exactly 1.000 cells.
const BRAILLE_SPIN: &[&str] = &[
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
    "\u{2827}", "\u{2807}", "\u{280f}",
];

/// A growing dot, not the classic `-\|/` rotation.
///
/// This tier's structural vocabulary already owns most of that set: `-` is
/// `expanded`, `|` is the selection `gutter`, `.` is `pending` and `>` is the
/// still `active`. A spinner cycling through those would put tree-drawing
/// characters in the state column -- `- - Task` for an expanded row on the
/// wrong frame -- which is worse than having no rotation at all.
///
/// `*` `o` `O` collide with nothing, and swelling reads as motion just as
/// clearly as turning does. Plain ASCII, so the width is beyond doubt anywhere.
const ASCII_SPIN: &[&str] = &["*", "o", "O"];

/// Nerd Font: chevrons and Font Awesome state icons.
pub const NERD: Icons = Icons {
    tier: Tier::Nerd,
    expanded: "\u{f078}",  // fa chevron-down
    collapsed: "\u{f054}", // fa chevron-right
    leaf: " ",
    pending: "\u{f070c}", // md-rhombus_outline
    active: "\u{f04b}",   // fa-play
    spin: BRAILLE_SPIN,
    done: "\u{f070b}",    // md-rhombus
    blocked: "\u{f05e}",  // fa-ban
    app: "\u{f0ae}",     // fa tasks
    project: "\u{f07b}", // fa folder
    // Heavy vertical, deliberately NOT `│` U+2502 -- that is already both the
    // pane border and the tree indent guide, so a gutter drawn with it would be
    // indistinguishable from either. Verified present in FiraCode Nerd Font and
    // native at exactly 1.00 cells.
    gutter: "\u{2503}", // ┃
    // The progress-bar kit Nerd Fonts 3.3.0 added: open left cap / mid / right
    // cap at U+EE00-EE02, filled at U+EE03-EE05. Composing by position gives a
    // properly capped, seamless bar rather than seven stamped cells.
    //
    // Its 6-frame arc spinner at U+EE06-EE0B is native at 1.00 cells too, and
    // is deliberately unused: it exists only in this tier, and a second
    // animation model for one tier is not worth two code paths.
    meter: Meter {
        done: ["\u{ee03}", "\u{ee04}", "\u{ee05}"],
        active: ["\u{ee03}", "\u{ee04}", "\u{ee05}"],
        empty: ["\u{ee00}", "\u{ee01}", "\u{ee02}"],
        partial: &[],
    },
};

/// Plain Unicode, verified present in FiraCode Nerd Font.
pub const UNICODE: Icons = Icons {
    tier: Tier::Unicode,
    expanded: "\u{25bc}",  // ▼
    collapsed: "\u{25b6}", // ▶
    leaf: " ",
    pending: "\u{25c7}", // ◇
    active: "\u{25ba}",  // ►  (NOT ▶ U+25B6 -- that is `collapsed`)
    spin: BRAILLE_SPIN,
    done: "\u{25c6}",    // ◆
    blocked: "\u{00d7}", // ×  (✗ and ⊗ are unavailable)
    app: "",
    project: "",
    gutter: "\u{2503}", // ┃
    // dex-report's stacked bar exactly: `█` for both done and in-flight, with
    // colour doing the separating, and `░` for the untouched remainder. The
    // eighth-blocks give the outer edge sub-cell precision.
    meter: Meter {
        done: ["\u{2588}"; 3],
        active: ["\u{2588}"; 3],
        empty: ["\u{2591}"; 3],
        partial: &[
            "\u{258f}", "\u{258e}", "\u{258d}", "\u{258c}", "\u{258b}", "\u{258a}", "\u{2589}",
        ],
    },
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
    spin: ASCII_SPIN,
    done: "x",
    blocked: "!",
    app: "",
    project: "",
    gutter: "|",
    // Whole cells only -- sub-cell precision has no 7-bit representation. In
    // exchange this is the one tier where done and in-flight differ by shape as
    // well as colour, which is what it is for.
    meter: Meter {
        done: ["#"; 3],
        active: ["+"; 3],
        empty: ["."; 3],
        partial: &[],
    },
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

    /// The selection gutter is a reserved column on *every* row, drawn only on
    /// the selected one, so a two-cell glyph there would shift that row's name
    /// out of line with the rest of the tree -- the same column discipline the
    /// state markers are held to.
    #[test]
    fn every_tier_has_a_one_cell_gutter() {
        assert_eq!(NERD.gutter, "\u{2503}");
        assert_eq!(UNICODE.gutter, "\u{2503}");
        assert_eq!(ASCII.gutter, "|");

        for ic in ALL {
            assert_eq!(
                ic.gutter.chars().count(),
                1,
                "tier {}: gutter is {:?}, not a single character",
                name(ic.tier),
                ic.gutter
            );
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
        let m = ASCII.meter;
        let meter = m
            .done
            .iter()
            .chain(m.active.iter())
            .chain(m.empty.iter())
            .chain(m.partial.iter());
        for g in [ASCII.expanded, ASCII.collapsed, ASCII.leaf, ASCII.gutter]
            .iter()
            .chain(meter)
        {
            assert!(g.is_ascii(), "ascii tier: {g:?} is not 7-bit");
        }
    }

    /// Every meter codepoint was checked twice, per the project rule: present in
    /// FiraCode Nerd Font via fontTools, *and* resolving natively at exactly
    /// 1.00 cells via CoreText. Pinning the tables here is what stops a later
    /// "tidy-up" swapping in a lookalike that macOS silently font-falls back.
    #[test]
    fn the_meter_glyphs_are_the_verified_codepoints() {
        // dex-report draws done and in-flight with the same full block and lets
        // colour separate them; we follow it, so the two tiers with a solid
        // block use one glyph for both.
        assert_eq!(UNICODE.meter.done, ["\u{2588}"; 3]);
        assert_eq!(UNICODE.meter.active, ["\u{2588}"; 3]);
        assert_eq!(UNICODE.meter.empty, ["\u{2591}"; 3]);
        assert_eq!(
            UNICODE.meter.partial,
            [
                "\u{258f}", "\u{258e}", "\u{258d}", "\u{258c}", "\u{258b}", "\u{258a}", "\u{2589}"
            ]
        );

        // The Nerd Fonts 3.3.0 progress-bar kit: open cap/mid/cap, then filled.
        assert_eq!(NERD.meter.done, ["\u{ee03}", "\u{ee04}", "\u{ee05}"]);
        assert_eq!(NERD.meter.active, ["\u{ee03}", "\u{ee04}", "\u{ee05}"]);
        assert_eq!(NERD.meter.empty, ["\u{ee00}", "\u{ee01}", "\u{ee02}"]);
        assert!(NERD.meter.partial.is_empty());

        assert_eq!(ASCII.meter.done, ["#"; 3]);
        assert_eq!(ASCII.meter.active, ["+"; 3]);
        assert_eq!(ASCII.meter.empty, ["."; 3]);
        assert!(ASCII.meter.partial.is_empty());
    }

    /// A capped tier draws a different glyph at each end, so a fractional cell
    /// landing on the last position would eat the right cap and the bar would
    /// stop looking like one object. Impossible today only because the nerd kit
    /// has no eighth-blocks; this makes it impossible on purpose.
    #[test]
    fn a_capped_tier_has_no_partial_cells() {
        for ic in ALL {
            let m = ic.meter;
            let capped = [m.done, m.active, m.empty]
                .iter()
                .any(|run| run[0] != run[1] || run[1] != run[2]);
            if capped {
                assert!(
                    m.partial.is_empty(),
                    "tier {}: a capped bar cannot carry a partial cell",
                    name(ic.tier)
                );
            }
        }
    }

    /// The eighth-block codepoints *descend* as the glyph widens (U+258F is the
    /// thinnest), so the table is written out rather than computed -- and an
    /// off-by-one here would silently draw the wrong fraction with nothing else
    /// to catch it. Indexing is `partial[eighths - 1]`, so the order is the
    /// whole contract.
    #[test]
    fn the_partial_cells_run_from_thinnest_to_widest() {
        for ic in ALL {
            let p = ic.meter.partial;
            if p.is_empty() {
                continue;
            }
            assert_eq!(p.len(), 7, "tier {}: 1/8 .. 7/8 is seven cells", name(ic.tier));
            let mut prev: Option<char> = None;
            for (i, g) in p.iter().enumerate() {
                let mut cs = g.chars();
                let c = cs.next().unwrap_or_else(|| panic!("tier {}: partial {i} is empty", name(ic.tier)));
                assert!(
                    cs.next().is_none(),
                    "tier {}: partial {i} is {g:?}, not a single character",
                    name(ic.tier)
                );
                if let Some(prev) = prev {
                    assert!(
                        c < prev,
                        "tier {}: partial {i} is {g:?}, which does not widen the run",
                        name(ic.tier)
                    );
                }
                prev = Some(c);
            }
        }
    }
}

