//! Starting values, read from `~/.config/dextui/config.toml`.
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
    animate: Option<bool>,
    single_pane_below: Option<u16>,
    split_percent: Option<u16>,
    repos_width: Option<u16>,
    repos_pane_above: Option<u16>,
    repos_open: Option<bool>,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub sort: Sort,
    pub sort_reversed: bool,
    pub filter: Filter,
    pub wrap: bool,
    pub icons: Icons,
    /// Whether in-progress rows spin. Off restores the pre-animation event
    /// loop exactly -- see `pulse::poll_timeout`.
    pub animate: bool,
    /// Terminal width below which only one pane is drawn at a time.
    ///
    /// Two panes below about this need every column for borders and indents and
    /// leave no room for either. `0` never switches, so the split is always
    /// shown; a value above any terminal you use always switches.
    pub single_pane_below: u16,
    /// The tree's share of the space it splits with the detail pane, as a
    /// percentage.
    ///
    /// A percentage rather than a width because both panes hold content that
    /// genuinely wants more room -- task names and prose -- unlike the
    /// sidebar. Of the region the two *share*, so it means the same thing
    /// whether or not the sidebar is on screen.
    pub split_percent: u16,
    /// The repo sidebar's width, in columns.
    ///
    /// Cells rather than a percentage: it holds repo and branch names, so a
    /// share of a wide terminal would be mostly empty space.
    pub repos_width: u16,
    /// Terminal width at or above which the repo pane is drawn as a third pane.
    ///
    /// Three panes need roughly this much before each is worth having.
    ///
    /// `0` means "never *automatically*", not "no repo pane": `1` and `b` still
    /// reach it, and at a width with room for two panes the sidebar takes one
    /// and the detail yields. Not the same as `single_pane_below = 0`, which
    /// really does turn its behaviour off -- see `App::laid_out`.
    pub repos_pane_above: u16,
    /// Whether the repo sidebar starts shown.
    ///
    /// `false` (the default) behaves exactly as if `b` had already been
    /// pressed: hidden regardless of `repos_pane_above` or terminal width,
    /// which is what the common case -- running `dextui` in a single repo's
    /// root and reading `./.dex` -- wants, since there is nothing for the
    /// sidebar to add there. `true` behaves as if `1` had: shown regardless of
    /// width. `repos_pane_above` still governs whether it fits as a *third*
    /// pane once shown, either way -- this only decides the starting state
    /// `b`/`1` then toggle from.
    pub repos_open: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sort: Sort::Priority,
            sort_reversed: false,
            filter: Filter::Pending,
            wrap: true,
            icons: icons::UNICODE,
            animate: true,
            single_pane_below: 80,
            split_percent: 45,
            repos_width: 26,
            repos_pane_above: 110,
            repos_open: false,
        }
    }
}

/// The per-project file, `.dextui.toml` at the git root.
///
/// Mirrors how dex layers `.dex/config.toml` over its global file, so "in this
/// repo, start unwrapped" is expressible. The root is found by walking up for a
/// `.git` entry rather than shelling out to git: no process spawn, and it also
/// matches worktrees, where `.git` is a file rather than a directory.
pub fn project_path() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir.join(".dextui.toml"));
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
    Some(base.join("dextui").join("config.toml"))
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
    if let Some(v) = raw.animate {
        cfg.animate = v;
    }
    if let Some(v) = raw.single_pane_below {
        cfg.single_pane_below = v;
    }
    // Clamped, not rejected: the same bounds a drag obeys, so a hand-edited
    // 5 or 300 lands where dragging there would rather than failing to start
    // over a number. `Config` is soft everywhere else for the same reason.
    if let Some(v) = raw.split_percent {
        cfg.split_percent = v.clamp(20, 80);
    }
    if let Some(v) = raw.repos_width {
        cfg.repos_width = v.max(crate::app::App::REPOS_WIDTH_MIN);
    }
    if let Some(v) = raw.repos_pane_above {
        cfg.repos_pane_above = v;
    }
    if let Some(v) = raw.repos_open {
        cfg.repos_open = v;
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
    if let Some(v) = std::env::var("DEXTUI_ANIMATE").ok().and_then(|v| parse_bool(&v)) {
        cfg.animate = v;
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

/// Only for the environment layer: TOML already has a real boolean, so a file
/// cannot spell one wrongly. An unrecognised env value is ignored in silence,
/// matching `DEXTUI_ICONS` -- files report their problems, the environment does
/// not, because it is a one-off override rather than something you maintain.
pub fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
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

/// Printed by `config` and written by `config init`, so there is something to
/// start from rather than invent.
pub const EXAMPLE: &str = r#"# Starting values only. w / o / O / f still toggle freely at runtime; nothing
# is written back, so this file stays exactly as you left it.
#
# Layered defaults < global < project < environment:
#   global   ~/.config/dextui/config.toml
#   project  .dextui.toml at the git root
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

# Breathe the marker on in-progress tasks. Costs nothing while nothing is
# running.  (DEXTUI_ANIMATE overrides this for one run)
animate = true

# Below this terminal width, show one pane at a time instead of squeezing both:
# Enter or Right opens the detail full-width, Left or Tab goes back. Two panes
# narrower than this leave no room for either.  0 always splits.
single_pane_below = 80

# At or above this terminal width, show the repo pane as a third pane alongside
# the tree and detail panes.  0 never shows it.
repos_pane_above = 110

# Whether the repo pane starts shown, regardless of repos_pane_above or
# terminal width -- false is exactly as if b had already been pressed, true as
# if 1 had. b / 1 still toggle it freely at runtime either way. Most sessions
# are one repo read from its own directory, where the sidebar has nothing to
# add, hence false.
repos_open = false

# The task tree's share of the space it splits with the detail pane, as a
# percentage of that space -- so it means the same thing with or without the
# repo pane. Drag the divider to change it for one run; 20-80.
split_percent = 45

# The repo pane's width in columns. Columns rather than a percentage: it holds
# repo and branch names, so a share of a wide terminal is mostly empty space.
# Drag its edge to change it for one run.
repos_width = 26
"#;

/// Which file a command acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project,
}

pub fn target(scope: Scope) -> Result<PathBuf, String> {
    match scope {
        Scope::Global => path().ok_or_else(|| "could not resolve a config path (is HOME set?)".into()),
        Scope::Project => project_path()
            .ok_or_else(|| "not inside a git repository, so there is no project config".into()),
    }
}

/// Creates the config file from the template.
///
/// Refuses to overwrite: a preferences file is the sort of thing people edit and
/// then forget about, and silently replacing one would be the worst outcome of a
/// command whose whole job is convenience.
pub fn init(scope: Scope, force: bool) -> Result<PathBuf, String> {
    let p = target(scope)?;

    if p.exists() && !force {
        return Err(format!(
            "{} already exists; pass --force to overwrite it",
            p.display()
        ));
    }

    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let template = match scope {
        Scope::Global => EXAMPLE,
        Scope::Project => PROJECT_EXAMPLE,
    };
    std::fs::write(&p, template).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(p)
}

/// Ensures a file exists to open in an editor, creating it from the template on
/// first use so `,` works before any config has been written.
pub fn path_for_editing(scope: Scope) -> Result<PathBuf, String> {
    let p = target(scope)?;
    if !p.exists() {
        init(scope, false)?;
    }
    Ok(p)
}

/// The project template is deliberately near-empty: a project file exists to
/// override a handful of things, and a full copy would silently pin every
/// setting against later changes to the global file.
pub const PROJECT_EXAMPLE: &str = r#"# .dextui.toml — settings for this repository only.
#
# Layered over ~/.config/dextui/config.toml, so mention only what differs.
# Anything left out falls through to the global file. See `dextui --config`.

# wrap = false          # this repo's descriptions have wide tables
# sort = "updated"      # priority | updated | created | name
# sort_reversed = false
# filter = "pending"    # pending | active | all
# icons = "unicode"     # nerd | unicode | ascii
# animate = false       # no pulse on in-progress rows in this repo
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies `text` the way `load` layers a file, so these test the real
    /// path rather than a parallel one.
    fn applied(text: &str) -> (Config, Vec<String>) {
        let raw: Raw = toml::from_str(text).expect("test config should parse");
        let mut cfg = Config::default();
        let mut problems = Vec::new();
        apply(&mut cfg, raw, &mut problems);
        (cfg, problems)
    }

    /// The template is the first thing anyone edits, so a key it advertises
    /// that does not parse -- or one it forgets to mention -- is a
    /// documentation bug with teeth.
    #[test]
    fn both_templates_parse_and_mention_the_layout_keys() {
        for text in [EXAMPLE, PROJECT_EXAMPLE] {
            let (cfg, problems) = applied(text);
            assert!(problems.is_empty(), "template complains: {problems:?}");
            assert!((20..=80).contains(&cfg.split_percent));
        }
        assert!(EXAMPLE.contains("split_percent"), "the global template omits split_percent");
        assert!(EXAMPLE.contains("repos_width"), "the global template omits repos_width");
        assert!(EXAMPLE.contains("repos_open"), "the global template omits repos_open");
    }

    /// Clamped rather than rejected, matching what a drag would have done --
    /// `Config` is soft everywhere else, and refusing to start over a number
    /// in a preferences file is the worse failure.
    #[test]
    fn out_of_range_layout_values_are_clamped_not_refused() {
        let (cfg, problems) = applied("split_percent = 5\nrepos_width = 2\n");
        assert!(problems.is_empty(), "a silly number must not stop the app");
        assert_eq!(cfg.split_percent, 20);
        assert_eq!(cfg.repos_width, crate::app::App::REPOS_WIDTH_MIN);

        let (cfg, _) = applied("split_percent = 300\n");
        assert_eq!(cfg.split_percent, 80);
    }

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

    /// The pulse is the app's only motion, and it is calm by design, so it is on
    /// unless someone asks otherwise.
    #[test]
    fn animation_is_on_by_default() {
        assert!(Config::default().animate);
    }

    /// The typical session is one repo, read from its own directory -- the
    /// sidebar has nothing to add there, so it must not appear unasked.
    #[test]
    fn the_repo_sidebar_is_hidden_by_default() {
        assert!(!Config::default().repos_open);
    }

    #[test]
    fn repos_open_can_be_turned_on_in_a_file() {
        let mut cfg = Config::default();
        let mut problems = Vec::new();

        apply(&mut cfg, raw("repos_open = true"), &mut problems);

        assert!(cfg.repos_open);
        assert!(problems.is_empty());
    }

    #[test]
    fn animate_can_be_turned_off_in_a_file() {
        let mut cfg = Config::default();
        let mut problems = Vec::new();

        apply(&mut cfg, raw("animate = false"), &mut problems);

        assert!(!cfg.animate);
        assert!(problems.is_empty());
    }

    /// The opt-out has to be reachable per repository too: one noisy project is
    /// exactly the case where you would turn it off without touching the global
    /// file.
    #[test]
    fn a_project_file_can_override_animate() {
        let mut cfg = Config::default();
        let mut problems = Vec::new();

        apply(&mut cfg, raw("animate = true"), &mut problems);
        apply(&mut cfg, raw("animate = false"), &mut problems);

        assert!(!cfg.animate);
    }

    /// `deny_unknown_fields` means the template and the struct cannot drift
    /// apart -- but only if the template actually mentions the key.
    #[test]
    fn the_example_file_documents_animate() {
        let raw: Raw = toml::from_str(EXAMPLE).expect("EXAMPLE must parse");
        assert_eq!(raw.animate, Some(true));
    }

    /// `DEXTUI_ANIMATE` itself is deliberately not tested through `load()`: env
    /// vars are process-global and `cargo test` runs threads in parallel, so
    /// such a test would be flaky -- which is why `DEXTUI_ICONS` has none
    /// either. Keeping `load()` a thin wiring line over a tested parser is what
    /// makes that acceptable.
    #[test]
    fn parse_bool_accepts_the_usual_spellings() {
        for v in ["true", "1", "yes", "on", " TRUE ", "On"] {
            assert_eq!(parse_bool(v), Some(true), "{v:?}");
        }
        for v in ["false", "0", "no", "off", " FALSE ", "Off"] {
            assert_eq!(parse_bool(v), Some(false), "{v:?}");
        }
        for v in ["maybe", "", "2"] {
            assert_eq!(parse_bool(v), None, "{v:?}");
        }
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
