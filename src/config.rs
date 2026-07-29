//! Starting values, read from `~/.config/dex-tui/config.toml`.
//!
//! The file is **read-only**: it sets what the app opens with, and the runtime
//! toggles (`w`, `o`, `O`, `f`) affect only the current run. Writing every
//! toggle back would mean turning wrap off for one wide table silently changed
//! your default forever, and would clobber comments in a hand-edited file.
//!
//! Precedence is defaults < file < `DEXTUI_*` environment, so an env var stays
//! useful as a one-off override without editing anything.
//!
//! A missing file is normal and silent. A malformed one is reported and then
//! ignored: refusing to start because of a typo in a preferences file would be
//! a worse failure than running with defaults.

use std::path::PathBuf;

use serde::Deserialize;

use crate::icons::{self, Icons};
use crate::tree::{Filter, Sort};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct Raw {
    sort: Option<String>,
    sort_reversed: Option<bool>,
    filter: Option<String>,
    wrap: Option<bool>,
    icons: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub sort: Sort,
    pub sort_reversed: bool,
    pub filter: Filter,
    pub wrap: bool,
    pub icons: Icons,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sort: Sort::Priority,
            sort_reversed: false,
            filter: Filter::Pending,
            wrap: true,
            icons: icons::UNICODE,
        }
    }
}

/// The per-project file, `.dex-tui.toml` at the git root.
///
/// Mirrors how dex layers `.dex/config.toml` over its global file, so "in this
/// repo, start unwrapped" is expressible. The root is found by walking up for a
/// `.git` entry rather than shelling out to git: no process spawn, and it also
/// matches worktrees, where `.git` is a file rather than a directory.
pub fn project_path() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.join(".dex-tui.toml"));
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Where the global config lives, honouring `XDG_CONFIG_HOME` like dex does.
pub fn path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("dex-tui").join("config.toml"))
}

/// Applies one file's values over `cfg`, recording anything unrecognised.
///
/// Only keys actually present are applied, so a partial project file overrides
/// exactly what it mentions and leaves the rest of the global file intact.
fn apply(cfg: &mut Config, raw: Raw, problems: &mut Vec<String>) {
    if let Some(v) = raw.sort.as_deref() {
        match parse_sort(v) {
            Some(s) => cfg.sort = s,
            None => problems.push(format!("unknown sort {v:?}")),
        }
    }
    if let Some(v) = raw.filter.as_deref() {
        match parse_filter(v) {
            Some(f) => cfg.filter = f,
            None => problems.push(format!("unknown filter {v:?}")),
        }
    }
    if let Some(v) = raw.icons.as_deref() {
        match parse_icons(v) {
            Some(i) => cfg.icons = i,
            None => problems.push(format!("unknown icons {v:?}")),
        }
    }
    if let Some(v) = raw.sort_reversed {
        cfg.sort_reversed = v;
    }
    if let Some(v) = raw.wrap {
        cfg.wrap = v;
    }
}

/// Reads one file, or `None` if it is absent. A parse failure is reported and
/// treated as absent.
fn read(path: Option<&PathBuf>, problems: &mut Vec<String>) -> Option<Raw> {
    let p = path?;
    let text = std::fs::read_to_string(p).ok()?;
    match toml::from_str::<Raw>(&text) {
        Ok(raw) => Some(raw),
        Err(e) => {
            problems.push(format!("{}: {}", p.display(), first_line(&e.to_string())));
            None
        }
    }
}

/// Loads the config, returning it plus anything worth telling the user about.
///
/// Layered defaults < global < project < environment, matching dex's own
/// precedence so the two behave the same way in the same repository.
///
/// Never fails: an unreadable or invalid file yields the layer beneath it.
pub fn load() -> (Config, Option<String>) {
    let mut cfg = Config::default();
    let mut problems: Vec<String> = Vec::new();

    if let Some(raw) = read(path().as_ref(), &mut problems) {
        apply(&mut cfg, raw, &mut problems);
    }
    if let Some(raw) = read(project_path().as_ref(), &mut problems) {
        apply(&mut cfg, raw, &mut problems);
    }

    // Environment last, so it overrides both files for a one-off run.
    if let Some(i) = std::env::var("DEXTUI_ICONS").ok().and_then(|v| parse_icons(&v)) {
        cfg.icons = i;
    }

    let problem = (!problems.is_empty()).then(|| problems.join("; "));
    (cfg, problem)
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or(s).trim().to_string()
}

pub fn parse_sort(v: &str) -> Option<Sort> {
    match v.trim().to_ascii_lowercase().as_str() {
        "priority" => Some(Sort::Priority),
        "updated" => Some(Sort::Updated),
        "created" => Some(Sort::Created),
        "name" => Some(Sort::Name),
        _ => None,
    }
}

pub fn parse_filter(v: &str) -> Option<Filter> {
    match v.trim().to_ascii_lowercase().as_str() {
        "pending" => Some(Filter::Pending),
        "active" | "in-progress" | "in_progress" => Some(Filter::InProgress),
        "all" => Some(Filter::All),
        _ => None,
    }
}

pub fn parse_icons(v: &str) -> Option<Icons> {
    match v.trim().to_ascii_lowercase().as_str() {
        "nerd" => Some(icons::NERD),
        "unicode" => Some(icons::UNICODE),
        "ascii" => Some(icons::ASCII),
        _ => None,
    }
}

/// Printed by `--config` so there is something to copy rather than invent.
pub const EXAMPLE: &str = r#"# Starting values only. w / o / O / f still toggle freely at runtime; nothing
# is written back, so this file stays exactly as you left it.
#
# Layered defaults < global < project < environment:
#   global   ~/.config/dex-tui/config.toml
#   project  .dex-tui.toml at the git root
# A project file need only mention what it changes.

# priority | updated | created | name
sort = "priority"

# Flips the order's natural direction: newest->oldest, updated->stalest.
sort_reversed = false

# pending | active | all
filter = "pending"

# Wrap long lines in the detail pane. Turn off to scroll wide tables sideways.
wrap = true

# nerd | unicode | ascii   (DEXTUI_ICONS overrides this for one run)
icons = "unicode"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_behaviour_before_a_config_existed() {
        let c = Config::default();
        assert_eq!(c.sort, Sort::Priority);
        assert!(!c.sort_reversed);
        assert_eq!(c.filter, Filter::Pending);
        assert!(c.wrap);
        assert_eq!(c.icons.tier, icons::UNICODE.tier);
    }

    #[test]
    fn every_documented_value_parses() {
        // The example file must not advertise anything the parser rejects.
        for v in ["priority", "updated", "created", "name"] {
            assert!(parse_sort(v).is_some(), "sort {v}");
        }
        for v in ["pending", "active", "all"] {
            assert!(parse_filter(v).is_some(), "filter {v}");
        }
        for v in ["nerd", "unicode", "ascii"] {
            assert!(parse_icons(v).is_some(), "icons {v}");
        }
    }

    #[test]
    fn values_are_case_and_whitespace_insensitive() {
        assert_eq!(parse_sort("  Updated "), Some(Sort::Updated));
        assert_eq!(parse_filter("ALL"), Some(Filter::All));
    }

    #[test]
    fn unknown_values_are_rejected_rather_than_guessed() {
        assert!(parse_sort("priorty").is_none());
        assert!(parse_filter("done").is_none());
        assert!(parse_icons("emoji").is_none());
    }

    #[test]
    fn a_full_file_round_trips_through_the_parser() {
        let raw: Raw = toml::from_str(
            r#"
            sort = "updated"
            sort_reversed = true
            filter = "all"
            wrap = false
            icons = "nerd"
            "#,
        )
        .expect("valid config should parse");

        assert_eq!(raw.sort.as_deref(), Some("updated"));
        assert_eq!(raw.sort_reversed, Some(true));
        assert_eq!(raw.wrap, Some(false));
    }

    #[test]
    fn the_example_file_is_valid_and_uses_only_known_keys() {
        // deny_unknown_fields means a typo in the example would fail here.
        let raw: Raw = toml::from_str(EXAMPLE).expect("EXAMPLE must parse");
        assert_eq!(raw.sort.as_deref(), Some("priority"));
        assert_eq!(raw.icons.as_deref(), Some("unicode"));
    }

    #[test]
    fn an_unknown_key_is_an_error_not_a_silent_no_op() {
        // Better to say "I do not know that setting" than to ignore it and let
        // someone believe it took effect.
        assert!(toml::from_str::<Raw>("colour_scheme = \"dark\"").is_err());
    }

    #[test]
    fn a_partial_file_leaves_the_other_defaults_alone() {
        let raw: Raw = toml::from_str("wrap = false").unwrap();
        assert_eq!(raw.wrap, Some(false));
        assert!(raw.sort.is_none());
    }

    fn raw(toml_src: &str) -> Raw {
        toml::from_str(toml_src).expect("test fixture should parse")
    }

    #[test]
    fn a_project_file_overrides_only_what_it_mentions() {
        // The point of layering: "in this repo, start unwrapped" without
        // restating every other preference.
        let mut cfg = Config::default();
        let mut problems = Vec::new();

        apply(&mut cfg, raw("sort = \"updated\"\nfilter = \"all\""), &mut problems);
        apply(&mut cfg, raw("wrap = false"), &mut problems);

        assert!(!cfg.wrap, "project value not applied");
        assert_eq!(cfg.sort, Sort::Updated, "global value was lost");
        assert_eq!(cfg.filter, Filter::All, "global value was lost");
        assert!(problems.is_empty());
    }

    #[test]
    fn the_later_layer_wins_on_the_same_key() {
        let mut cfg = Config::default();
        let mut problems = Vec::new();

        apply(&mut cfg, raw("sort = \"name\""), &mut problems);
        apply(&mut cfg, raw("sort = \"created\""), &mut problems);

        assert_eq!(cfg.sort, Sort::Created);
    }

    #[test]
    fn an_empty_layer_changes_nothing() {
        let mut cfg = Config::default();
        let mut problems = Vec::new();
        apply(&mut cfg, raw("sort = \"name\""), &mut problems);

        apply(&mut cfg, raw(""), &mut problems);

        assert_eq!(cfg.sort, Sort::Name);
    }

    #[test]
    fn a_bad_value_is_reported_and_leaves_the_layer_beneath_intact() {
        let mut cfg = Config::default();
        let mut problems = Vec::new();

        apply(&mut cfg, raw("sort = \"updated\""), &mut problems);
        apply(&mut cfg, raw("sort = \"nonsense\""), &mut problems);

        assert_eq!(cfg.sort, Sort::Updated, "a typo should not reset to the default");
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("nonsense"));
    }

    #[test]
    fn problems_from_several_layers_are_all_reported() {
        let mut cfg = Config::default();
        let mut problems = Vec::new();

        apply(&mut cfg, raw("sort = \"bogus\""), &mut problems);
        apply(&mut cfg, raw("filter = \"bogus\""), &mut problems);

        assert_eq!(problems.len(), 2);
    }
}
