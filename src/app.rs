//! All mutable view state, plus the rules for surviving a background refresh.
//!
//! Immediate-mode rendering pays off here: the selection is just an id we own,
//! so a refresh cannot lose it inside a widget. Only the genuine product rule --
//! what to select when the selected task is deleted elsewhere -- needs code.

use std::collections::{HashMap, HashSet};

use crate::config::Config;
use crate::dex::Task;
use crate::tree::{self, Filter, Node, Progress, Sort};

/// A simple char-indexed editable buffer. Enough for a task name or a result note.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    pub value: String,
    /// Cursor position in characters, not bytes.
    pub cursor: usize,
}

impl TextInput {
    pub fn new(initial: &str) -> Self {
        Self {
            value: initial.to_string(),
            cursor: initial.chars().count(),
        }
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len())
    }

    pub fn insert(&mut self, c: char) {
        let b = self.byte_at(self.cursor);
        self.value.insert(b, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value.chars().count());
    }
}

/// What a prompt should do once the user accepts it.
#[derive(Debug, Clone)]
pub enum Pending {
    Complete { id: String },
    CreateName { parent: Option<String> },
    CreateDescription { parent: Option<String>, name: String },
    EditName { id: String },
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub title: String,
    pub label: String,
    pub input: TextInput,
    pub pending: Pending,
}

#[derive(Debug, Clone)]
pub enum Mode {
    Normal,
    Search,
    Prompt(Prompt),
    /// Delete confirmation.
    Confirm { id: String, message: String },
    /// Offered when dex refuses to complete a task with unfinished subtasks.
    ForceComplete { id: String, result: String, message: String },
    Error(String),
    Help,
}

/// Which pane the movement keys drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Detail,
    /// Not yet reachable -- no key sets it until the task that wires it, but
    /// it already has to exist so `App`'s new fields and match arms compile.
    #[allow(dead_code)]
    Repos,
}

/// How many panes are drawn. A single ordered ladder, so first-fit can only
/// ever shed -- see `the_pane_ladder_is_monotone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panes {
    One,
    Two,
    Three,
}

/// Something in the header you can click.
///
/// Published by the renderer each frame from **what it actually drew**, the same
/// way `divider_x` is, so the header's degradation ladder needs no second copy
/// of itself here: a rung that dropped the menu simply publishes no zones for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderZone {
    /// Left cycles the order, right reverses it -- mirroring `o` and `O`.
    Sort,
    /// One word of the menu. Picks that filter outright.
    Filter(Filter),
    /// The lone filter name a narrow header falls back to. With no options on
    /// screen there is nothing to pick, so a click advances instead.
    FilterCycle,
    /// A numbered pane tab. Only drawn in zoom mode, so only clickable there.
    Pane(Focus),
}

/// What the header reports about the whole store. See [`App::counts`] for why
/// `ready + blocked` deliberately does not equal `pending`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub completed: usize,
    pub pending: usize,
    /// Started and unfinished.
    pub active: usize,
    /// Has at least one blocker that exists and is not completed.
    pub blocked: usize,
    /// Pending, unstarted, unblocked, and with no unfinished children.
    pub ready: usize,
    /// Completed over total, rounded down.
    pub percent: usize,
}

pub struct App {
    pub tasks: Vec<Task>,
    pub by_id: HashMap<String, Task>,
    pub tree: Vec<Node>,
    /// Subtree completion per task, from the unfiltered list.
    pub progress: HashMap<String, Progress>,
    pub expanded: HashSet<String>,
    pub selected: Option<String>,
    pub filter: Filter,
    pub sort: Sort,
    pub sort_reversed: bool,
    pub query: TextInput,
    pub mode: Mode,
    pub status: String,
    pub store_label: String,
    pub should_quit: bool,
    /// Set by `e`; the main loop picks it up and hands off to $EDITOR, which
    /// cannot happen mid-draw because the terminal has to be released first.
    pub pending_editor: Option<String>,
    /// Set by `,`; the main loop opens the config file in $EDITOR and reloads.
    pub pending_config_edit: bool,
    /// Width of the tree pane as a percentage. Dragged with the mouse.
    pub split_percent: u16,
    pub dragging_split: bool,
    /// Geometry the renderer publishes so mouse maths can be exact rather than
    /// re-derived from assumptions about the layout.
    pub divider_x: u16,
    pub body_top: u16,
    pub body_bottom: u16,
    pub terminal_width: u16,
    /// The list's scroll offset, kept across frames so a click maps to the row
    /// actually under the cursor.
    pub tree_offset: usize,
    pub focus: Focus,
    /// (vertical, horizontal) offset into the detail pane.
    pub detail_scroll: (u16, u16),
    /// Wrapping and horizontal scrolling are mutually exclusive: wrapping
    /// removes the overflow there would be anything to scroll to. Prose wants
    /// wrap on, wide tables want it off, hence a toggle rather than a setting.
    pub wrap: bool,
    /// Written by the renderer each frame so input can clamp scrolling to
    /// content it cannot otherwise measure (wrapped height depends on width).
    pub detail_content_height: u16,
    pub detail_viewport_height: u16,
    /// Set when a refresh arrives while a dialog is open; applied on close.
    pub pending_refresh: bool,
    /// Whether in-progress rows spin at all. From the config, and the opt-out
    /// reaches the event loop's timeout rather than only the colour.
    pub animate: bool,
    /// Which frame of the in-progress rotation the renderer should draw.
    pub spin_frame: usize,
    /// Terminal width below which only the focused pane is drawn. From the
    /// config; 0 disables the behaviour entirely.
    pub single_pane_below: u16,
    /// Terminal width at or above which the repo pane is drawn as a third pane.
    /// From the config; 0 disables the behaviour entirely.
    pub repos_pane_above: u16,
    /// A manual answer to "zoomed?", set by `z`, which outranks the width rule.
    /// `None` means decide by width. Pressing the key is an explicit decision,
    /// so it holds until it is pressed again rather than being undone by a
    /// resize -- a layout that changed on its own would read as a fault.
    pub zoom: Option<bool>,
    /// Clickable regions of the header row, as `(first_x, last_x, what)`.
    /// Rewritten every frame; empty while the search box owns the row, so a
    /// click can never act on a menu that is not on screen.
    pub header_zones: Vec<(u16, u16, HeaderZone)>,
    /// Which worktree's store the task tree is showing.
    #[allow(dead_code)] // not yet consumed by the renderer -- a later task draws the pane
    pub selected_worktree: Option<String>,
    /// Task selection per worktree path, so switching back returns the cursor.
    /// Session-only: this is view state, not something to persist.
    #[allow(dead_code)] // not yet consumed by the renderer -- a later task draws the pane
    pub task_memory: HashMap<String, String>,
    /// Registered repos with their worktrees, and whether each is expanded.
    pub repos: Vec<crate::repos::Repo>,
    pub selected_repo_row: usize,
    #[allow(dead_code)] // not yet consumed by the renderer -- a later task draws the pane
    pub registry: crate::registry::Registry,
}

impl App {
    pub fn new(tasks: Vec<Task>, store_label: String, cfg: Config) -> Self {
        let mut app = Self {
            by_id: index(&tasks),
            tasks,
            tree: Vec::new(),
            progress: HashMap::new(),
            expanded: HashSet::new(),
            selected: None,
            filter: cfg.filter,
            sort: cfg.sort,
            sort_reversed: cfg.sort_reversed,
            query: TextInput::default(),
            mode: Mode::Normal,
            status: String::new(),
            store_label,
            should_quit: false,
            pending_editor: None,
            pending_config_edit: false,
            split_percent: 45,
            dragging_split: false,
            divider_x: 0,
            body_top: 0,
            body_bottom: 0,
            terminal_width: 0,
            tree_offset: 0,
            focus: Focus::Tree,
            detail_scroll: (0, 0),
            wrap: cfg.wrap,
            detail_content_height: 0,
            detail_viewport_height: 0,
            pending_refresh: false,
            animate: cfg.animate,
            spin_frame: 0,
            single_pane_below: cfg.single_pane_below,
            repos_pane_above: cfg.repos_pane_above,
            zoom: None,
            header_zones: Vec::new(),
            selected_worktree: None,
            task_memory: HashMap::new(),
            repos: Vec::new(),
            selected_repo_row: 0,
            registry: crate::registry::Registry::default(),
        };

        // Everything is "new" on first load, so the collapse-new-tasks rule would
        // otherwise open onto a single collapsed root. Expand once up front.
        app.progress = tree::subtree_progress(&app.tasks);
        app.expand_all();
        app.rebuild();
        app.selected = app.first_visible_id();
        app
    }

    pub fn expand_all(&mut self) {
        // Built unfiltered, so a task stays expanded once a filter that hid its
        // children is cleared again.
        let full = tree::build(&self.tasks, "", Filter::All, self.sort, self.sort_reversed);
        self.expanded = tree::flatten(&full)
            .iter()
            .filter(|n| !n.children.is_empty())
            .map(|n| n.task.id.clone())
            .collect();
    }

    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    pub fn rebuild(&mut self) {
        self.tree = tree::build(
            &self.tasks,
            &self.query.value,
            self.filter,
            self.sort,
            self.sort_reversed,
        );

        // A selection filtered out of view must not linger invisibly -- and
        // having *no* selection while rows are on screen is the same fault seen
        // from the other side. Gating this on the selection being `Some` meant
        // that once a filter matched nothing, the selection went to `None` and
        // could never come back: every later rebuild skipped the repair, so the
        // detail pane read "No tasks match the current filter" against a tree
        // full of them.
        //
        // Note this only ever *establishes* a selection, never moves a live one,
        // so the rule that a refresh must not disturb the user still holds.
        let still_visible = self
            .selected
            .as_ref()
            .is_some_and(|sel| self.visible_ids().contains(sel));
        if !still_visible {
            self.selected = self.first_visible_id();
        }
    }

    fn visible_ids(&self) -> HashSet<String> {
        tree::flatten(&self.tree)
            .iter()
            .map(|n| n.task.id.clone())
            .collect()
    }

    fn first_visible_id(&self) -> Option<String> {
        tree::visible_rows(&self.tree, &self.expanded)
            .first()
            .map(|r| r.node.task.id.clone())
    }

    pub fn selected_task(&self) -> Option<&Task> {
        self.selected.as_ref().and_then(|id| self.by_id.get(id))
    }

    pub fn row_ids(&self) -> Vec<String> {
        tree::visible_rows(&self.tree, &self.expanded)
            .iter()
            .map(|r| r.node.task.id.clone())
            .collect()
    }

    pub fn selected_row(&self) -> Option<usize> {
        let sel = self.selected.as_ref()?;
        self.row_ids().iter().position(|id| id == sel)
    }

    pub fn move_selection(&mut self, delta: isize) {
        let rows = self.row_ids();
        if rows.is_empty() {
            return;
        }

        let current = self.selected_row().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.select(Some(rows[next].clone()));
    }

    /// A wheel or trackpad drag over the tree. The *content* slides with the
    /// gesture and the cursor holds its place on screen, which is exactly what
    /// the detail pane does.
    ///
    /// Moving only the selection is the obvious implementation and reads as
    /// backwards. Mid-list the view does not move at all, so the only thing the
    /// eye can track is the cursor -- and the cursor travels *against* the
    /// fingers, while the detail pane's text travels with them. One drag, two
    /// directions, in panes an inch apart.
    ///
    /// The offset clamps against the row count, not the viewport height, which
    /// this type does not know. Overshooting is harmless: the list widget pulls
    /// the offset back far enough to keep the selection visible and the renderer
    /// writes the corrected value into `tree_offset`.
    pub fn scroll_tree(&mut self, delta: isize) {
        let rows = self.row_ids();
        if rows.is_empty() {
            return;
        }
        let last = rows.len() as isize - 1;
        self.tree_offset = (self.tree_offset as isize + delta).clamp(0, last) as usize;
        self.move_selection(delta);
    }

    pub fn select_first(&mut self) {
        let id = self.row_ids().first().cloned();
        self.select(id);
    }

    pub fn select_last(&mut self) {
        let id = self.row_ids().last().cloned();
        self.select(id);
    }

    /// Right arrow: open the node, or step into it if already open.
    /// Returns whether it did anything, so a caller can fall through to
    /// something else when the row has no children to open or step into.
    pub fn expand_selected(&mut self) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        let has_kids = tree::flatten(&self.tree)
            .iter()
            .any(|n| n.task.id == id && !n.children.is_empty());

        if has_kids && !self.expanded.contains(&id) {
            self.expanded.insert(id);
        } else if has_kids {
            self.move_selection(1);
        }
        has_kids
    }

    /// Left arrow: close the node, or step out to its parent if already closed.
    pub fn collapse_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };

        if self.expanded.contains(&id) {
            self.expanded.remove(&id);
            return;
        }

        if let Some(parent) = self.by_id.get(&id).and_then(|t| t.parent_id.clone())
            && self.row_ids().contains(&parent) {
                self.selected = Some(parent);
            }
    }

    /// Applies a freshly fetched task list without disturbing the user.
    pub fn apply_tasks(&mut self, next: Vec<Task>) {
        let next_ids: HashSet<String> = next.iter().map(|t| t.id.clone()).collect();

        // Keep expansion only for tasks that still exist. Tasks added since the
        // last refresh are absent here, so new work arrives collapsed and an agent
        // creating subtasks cannot explode the tree under the cursor.
        self.expanded.retain(|id| next_ids.contains(id));

        self.selected = self.resolve_selection(&next_ids, &next);

        self.by_id = index(&next);
        self.tasks = next;
        self.progress = tree::subtree_progress(&self.tasks);
        self.rebuild();
    }

    fn resolve_selection(&self, next_ids: &HashSet<String>, next: &[Task]) -> Option<String> {
        let Some(sel) = self.selected.clone() else {
            return first_root(next);
        };

        // The common case: what was selected is still there.
        if next_ids.contains(&sel) {
            return Some(sel);
        }

        // It vanished. Prefer a sibling, so the cursor stays visually put...
        if let Some(sib) = self.nearest_sibling(&sel, next_ids) {
            return Some(sib);
        }

        // ...then climb to a surviving ancestor, keeping the same branch.
        if let Some(anc) = self.nearest_ancestor(&sel, next_ids) {
            return Some(anc);
        }

        first_root(next)
    }

    fn nearest_sibling(&self, id: &str, next_ids: &HashSet<String>) -> Option<String> {
        let removed = self.by_id.get(id)?;

        // Reconstruct the sibling order as it was before the refresh.
        let siblings: Vec<String> = match removed.parent_id.as_ref() {
            Some(p) => self.by_id.get(p).map(|t| t.children.clone())?,
            None => {
                let mut roots: Vec<&Task> = self
                    .tasks
                    .iter()
                    .filter(|t| {
                        t.parent_id
                            .as_ref()
                            .is_none_or(|p| !self.by_id.contains_key(p))
                    })
                    .collect();
                roots.sort_by(|a, b| {
                    a.priority
                        .cmp(&b.priority)
                        .then_with(|| a.created_at.cmp(&b.created_at))
                });
                roots.into_iter().map(|t| t.id.clone()).collect()
            }
        };

        let idx = siblings.iter().position(|s| s == id)?;

        // Scan outward from where it used to be: next sibling first, then previous.
        for offset in 1..=siblings.len() {
            if let Some(after) = siblings.get(idx + offset)
                && next_ids.contains(after) {
                    return Some(after.clone());
                }
            if offset <= idx {
                let before = &siblings[idx - offset];
                if next_ids.contains(before) {
                    return Some(before.clone());
                }
            }
        }

        None
    }

    fn nearest_ancestor(&self, id: &str, next_ids: &HashSet<String>) -> Option<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut cursor = Some(id.to_string());

        while let Some(current) = cursor {
            if !seen.insert(current.clone()) {
                return None;
            }
            let parent = self.by_id.get(&current)?.parent_id.clone();
            match parent {
                Some(p) if next_ids.contains(&p) => return Some(p),
                other => cursor = other,
            }
        }

        None
    }

    /// Store-wide totals for the header, from the **unfiltered** list -- like the
    /// progress rollups, so changing what is on screen never changes what the
    /// header reports.
    ///
    /// The rule mirrors the `dex list --ready` / `dex list --blocked` pair, and
    /// deliberately **not** `dex status`'s partition. dex disagrees with itself:
    /// `cli/status.js` counts a parent with unfinished children as blocked,
    /// while `list --blocked` counts only tasks with an incomplete blocker.
    /// Measured across four real stores, five of the six tasks `dex status`
    /// calls blocked have no blocker at all -- two of them contain no blocking
    /// relationship anywhere and it still reported some. Following `status.js`
    /// would also put this header at odds with the tree drawn beneath it, since
    /// the row glyph means "has an incomplete blocker".
    ///
    /// The cost, which is deliberate: `ready + blocked` does **not** sum to
    /// `pending`. A parent with unfinished children is neither -- you cannot
    /// pick up an epic, and nothing is blocking it. Do not close that gap by
    /// folding parents into either bucket; a test asserts the gap exists.
    pub fn counts(&self) -> Counts {
        let mut c = Counts {
            total: self.tasks.len(),
            ..Default::default()
        };

        for t in &self.tasks {
            if t.completed {
                c.completed += 1;
                continue;
            }
            c.pending += 1;

            // Same precedence as status.js: started wins over everything else.
            if t.is_in_progress() {
                c.active += 1;
            } else if crate::dex::is_blocked(t, &self.by_id) {
                c.blocked += 1;
            } else if !self.has_incomplete_children(t) {
                c.ready += 1;
            }
        }

        c.percent = match c.total {
            0 => 0,
            n => (c.completed * 100) / n,
        };
        c
    }

    /// Immediate children only, matching dex's `hasIncompleteChildren` -- not the
    /// progress rollup, which counts all descendants.
    fn has_incomplete_children(&self, t: &Task) -> bool {
        t.children
            .iter()
            .filter_map(|id| self.by_id.get(id))
            .any(|c| !c.completed)
    }

    /// Whether anything on screen is currently turning.
    ///
    /// `animate` is tested first on purpose, so the opt-out costs nothing at all
    /// -- otherwise turning it off would still pay for the scan on every wakeup.
    pub fn is_animating(&self) -> bool {
        self.animate && self.tasks.iter().any(|t| t.is_in_progress())
    }

    /// Advances the pulse, returning true when the frame needs repainting.
    ///
    /// That return value is the *only* redraw animation ever causes, which is
    /// what keeps the cost of this feature to a number you can state.
    ///
    /// `elapsed` is passed in rather than read from a clock so the schedule is
    /// deterministically testable, and it folds in the settle case for free:
    /// when the last in-progress task finishes, `is_animating` goes false and
    /// the marker returns to its base state in one final repaint rather than
    /// freezing bright.
    pub fn pulse_tick(&mut self, elapsed: std::time::Duration, frames: usize) -> bool {
        // Frame 0 is the resting glyph, so a stopped spinner settles on the same
        // marker the header and the help show -- rather than freezing on
        // whichever frame happened to be up.
        let want = if self.is_animating() {
            crate::pulse::frame(elapsed, frames)
        } else {
            0
        };
        if want == self.spin_frame {
            return false;
        }
        self.spin_frame = want;
        true
    }

    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.rebuild();
    }

    pub fn toggle_sort_direction(&mut self) {
        self.sort_reversed = !self.sort_reversed;
        self.rebuild();
    }

    /// Clamped so neither pane can be dragged away entirely.
    pub fn set_split(&mut self, column: u16, total_width: u16) {
        if total_width == 0 {
            return;
        }
        let pct = (column as f32 / total_width as f32 * 100.0).round() as i32;
        self.split_percent = pct.clamp(20, 80) as u16;
    }

    /// True when `column` is on (or beside) the divider, so it is grabbable
    /// without demanding single-cell precision.
    pub fn on_divider(&self, column: u16) -> bool {
        self.divider_x > 0 && column.abs_diff(self.divider_x) <= 1
    }

    pub fn in_body(&self, row: u16) -> bool {
        row >= self.body_top && row < self.body_bottom
    }

    /// What sits under `column` on the header row, if anything.
    pub fn header_zone_at(&self, column: u16) -> Option<HeaderZone> {
        self.header_zones
            .iter()
            .find(|(from, to, _)| column >= *from && column <= *to)
            .map(|(_, _, z)| *z)
    }

    /// Acts on a header click. `secondary` is the right button, which reverses
    /// the sort rather than cycling it -- mirroring `o` and `O`.
    ///
    /// Returns whether anything changed, so a click on empty header space stays
    /// genuinely inert: it must not steal focus or move the selection.
    pub fn click_header(&mut self, column: u16, secondary: bool) -> bool {
        let Some(zone) = self.header_zone_at(column) else {
            return false;
        };
        match zone {
            HeaderZone::Sort if secondary => self.sort_reversed = !self.sort_reversed,
            HeaderZone::Sort => self.sort = self.sort.next(),
            // Picking a filter with the right button would be a surprise; only
            // the sort has a second action.
            _ if secondary => return false,
            HeaderZone::Filter(f) => self.filter = f,
            HeaderZone::FilterCycle => self.filter = self.filter.next(),
            // Nothing to rebuild -- the tree is unchanged, only which pane is
            // looked at -- but returning true still marks the click as handled.
            HeaderZone::Pane(f) => {
                self.focus = f;
                return true;
            }
        }
        self.rebuild();
        true
    }

    /// Selects the task drawn on `row`, if any.
    pub fn select_at_row(&mut self, row: u16) {
        // +1 skips the pane's top border.
        let Some(index) = row
            .checked_sub(self.body_top + 1)
            .map(|r| r as usize + self.tree_offset)
        else {
            return;
        };

        let rows = self.row_ids();
        if let Some(id) = rows.get(index) {
            self.select(Some(id.clone()));
        }
    }

    /// Applies a freshly loaded config to a running session.
    ///
    /// Everything the file controls is a *starting* value, so a reload after an
    /// edit is the one moment those values are meant to replace what the runtime
    /// toggles have done — otherwise saving a change would appear to do nothing.
    pub fn apply_config(&mut self, cfg: Config) {
        self.sort = cfg.sort;
        self.sort_reversed = cfg.sort_reversed;
        self.filter = cfg.filter;
        self.wrap = cfg.wrap;
        self.animate = cfg.animate;
        self.repos_pane_above = cfg.repos_pane_above;
        self.rebuild();
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Tree => Focus::Detail,
            Focus::Detail => Focus::Tree,
            // Tab has never been the way into the repo pane -- that is a
            // dedicated key, wired in a later task -- so leaving it lands back
            // on the tree rather than bouncing between two panes that are not
            // the one Tab is documented to cross.
            Focus::Repos => Focus::Tree,
        };
    }

    /// Whether only one pane is drawn, the focused one filling the width.
    ///
    /// Two panes below this leave no room for either: borders, the tree's
    /// indent guides and the meter gutter all cost columns before a task name
    /// gets any. So `focus` stops meaning "which border is brighter" and starts
    /// meaning "which pane you are looking at" -- no new state, and the rules
    /// that keep a refresh from disturbing the user keep working unchanged.
    ///
    /// Measured against the width the renderer last published, so it follows a
    /// terminal resized under the running app.
    pub fn single_pane(&self) -> bool {
        match self.zoom {
            Some(z) => z,
            None => self.single_pane_below > 0 && self.terminal_width < self.single_pane_below,
        }
    }

    /// See [`Panes`].
    pub fn panes(&self) -> Panes {
        if self.single_pane() {
            return Panes::One;
        }
        if self.repos_pane_above > 0 && self.terminal_width >= self.repos_pane_above {
            return Panes::Three;
        }
        Panes::Two
    }

    /// Flips what you are looking at, and keeps it that way.
    ///
    /// Always toggles the *effective* state rather than a stored flag, so the
    /// first press does the visible thing whether the width had zoomed you or
    /// not -- otherwise pressing it on an already-narrow terminal would appear
    /// to do nothing.
    pub fn toggle_zoom(&mut self) {
        self.zoom = Some(!self.single_pane());
    }

    /// Moves to the detail pane. What `Enter` does, and what `Right` falls back
    /// to when there is nothing left to expand.
    pub fn show_detail(&mut self) {
        self.focus = Focus::Detail;
    }

    /// Back to the tree.
    pub fn show_tree(&mut self) {
        self.focus = Focus::Tree;
    }

    /// Which pane occupies `column`.
    ///
    /// With one pane it is whichever is on screen, whatever the column -- the
    /// mouse handlers otherwise compare against `divider_x`, which is 0 there,
    /// so every click and every wheel tick would land on the detail pane
    /// including while looking at the tree.
    pub fn pane_at(&self, column: u16) -> Focus {
        if self.single_pane() {
            self.focus
        } else if column < self.divider_x {
            Focus::Tree
        } else {
            Focus::Detail
        }
    }

    /// Wrapping on makes horizontal offset meaningless, so it is also reset.
    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        if self.wrap {
            self.detail_scroll.1 = 0;
        }
    }

    pub fn scroll_detail(&mut self, dy: i32, dx: i32) {
        let max_y = self
            .detail_content_height
            .saturating_sub(self.detail_viewport_height);

        let y = (self.detail_scroll.0 as i32 + dy).clamp(0, max_y as i32) as u16;
        // No content width is known, so the horizontal offset is only bounded
        // below; scrolling past the end simply shows blank.
        let x = if self.wrap {
            0
        } else {
            (self.detail_scroll.1 as i32 + dx).max(0) as u16
        };

        self.detail_scroll = (y, x);
    }

    pub fn detail_to_top(&mut self) {
        self.detail_scroll = (0, self.detail_scroll.1);
    }

    pub fn detail_to_bottom(&mut self) {
        let max_y = self
            .detail_content_height
            .saturating_sub(self.detail_viewport_height);
        self.detail_scroll = (max_y, self.detail_scroll.1);
    }

    /// Selecting a different task must not leave you halfway down the old one.
    fn select(&mut self, id: Option<String>) {
        if id != self.selected {
            self.detail_scroll = (0, 0);
        }
        self.selected = id;
    }

    pub fn is_modal(&self) -> bool {
        !matches!(self.mode, Mode::Normal | Mode::Search)
    }

    /// Switches which store the task panes read, remembering where the cursor
    /// was in the worktree being left.
    #[allow(dead_code)] // not yet consumed by the renderer -- a later task wires the keys
    pub fn select_worktree(&mut self, path: &str) {
        if self.selected_worktree.as_deref() == Some(path) {
            return;
        }
        if let (Some(old), Some(sel)) = (self.selected_worktree.clone(), self.selected.clone()) {
            self.task_memory.insert(old, sel);
        }
        self.selected_worktree = Some(path.to_string());
        self.selected = self.task_memory.get(path).cloned();
    }

    /// Rebuilt from `self.repos` on every call, exactly as the task tree is
    /// rebuilt every frame -- a cached `Vec<Row>` would go stale the moment the
    /// repo list changed underneath it, since `Row` carries bare indices.
    pub fn repo_rows(&self) -> Vec<crate::repos::Row> {
        crate::repos::rows(&self.repos)
    }
}

fn index(tasks: &[Task]) -> HashMap<String, Task> {
    tasks.iter().map(|t| (t.id.clone(), t.clone())).collect()
}

fn first_root(tasks: &[Task]) -> Option<String> {
    let ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let mut roots: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.parent_id.as_deref().is_none_or(|p| !ids.contains(p)))
        .collect();

    roots.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    roots.first().map(|t| t.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, parent: Option<&str>, children: &[&str]) -> Task {
        Task {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            name: id.to_string(),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            children: children.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn counted(tasks: Vec<Task>) -> App {
        App::new(tasks, "demo".into(), Config::default())
    }

    /// Cycling through a filter that matches nothing used to strand the app:
    /// the selection went to `None` with nothing to select, and `rebuild`'s
    /// repair was gated on the selection being `Some`, so it never came back
    /// when rows did. The detail pane then read "No tasks match the current
    /// filter" against a tree full of tasks.
    #[test]
    fn passing_through_an_empty_filter_does_not_strand_the_selection() {
        let mut app = counted(vec![task("a", None, &[]), task("b", None, &[])]);
        app.filter = Filter::Pending;
        app.rebuild();
        assert!(app.selected.is_some(), "starts with something selected");

        // Nothing is started, so this matches no task at all.
        app.filter = Filter::InProgress;
        app.rebuild();
        assert_eq!(app.selected, None, "nothing to select is correct here");

        app.filter = Filter::Pending;
        app.rebuild();
        assert!(
            app.selected.is_some(),
            "rows are visible again, so something must be selected"
        );
    }

    /// The other half of the same rule, and the one that must not regress: a
    /// selection that is still on screen is never moved. That is the invariant
    /// the whole app is built around.
    #[test]
    fn rebuild_never_moves_a_selection_that_is_still_visible() {
        let mut app = counted(vec![task("a", None, &[]), task("b", None, &[])]);
        app.selected = Some("b".into());

        app.rebuild();
        assert_eq!(app.selected.as_deref(), Some("b"));

        app.sort_reversed = !app.sort_reversed;
        app.rebuild();
        assert_eq!(app.selected.as_deref(), Some("b"), "reordering is not moving");
    }

    fn narrow(width: u16) -> App {
        let mut app = counted(vec![task("a", None, &["b"]), task("b", Some("a"), &[])]);
        app.single_pane_below = 80;
        app.terminal_width = width;
        app.rebuild();
        app
    }

    /// The first press must do the visible thing whichever way the width had
    /// already decided -- toggling a stored flag instead would appear to do
    /// nothing on a terminal that was zoomed automatically.
    #[test]
    fn the_zoom_key_always_flips_what_you_are_looking_at() {
        let mut wide = narrow(100);
        assert!(!wide.single_pane());
        wide.toggle_zoom();
        assert!(wide.single_pane(), "z on a wide terminal must zoom");
        wide.toggle_zoom();
        assert!(!wide.single_pane(), "z again must split");

        let mut small = narrow(60);
        assert!(small.single_pane(), "the width already zoomed this one");
        small.toggle_zoom();
        assert!(!small.single_pane(), "z must be able to force the split back");
    }

    /// Pressing the key is an explicit decision, so it outranks the width until
    /// pressed again. A layout that reverted on a resize would read as a fault.
    #[test]
    fn a_resize_does_not_undo_the_zoom_key() {
        let mut app = narrow(100);
        app.toggle_zoom();
        assert!(app.single_pane());

        for w in [40u16, 80, 120, 200] {
            app.terminal_width = w;
            assert!(app.single_pane(), "{w} columns undid the manual zoom");
        }
    }

    #[test]
    fn the_split_gives_way_below_the_configured_width() {
        assert!(narrow(60).single_pane(), "60 columns should be one pane");
        assert!(!narrow(80).single_pane(), "the threshold itself still splits");
        assert!(!narrow(100).single_pane());
    }

    /// The escape hatches at both ends of the setting, which is the reason it is
    /// a width rather than a boolean.
    #[test]
    fn zero_always_splits_and_a_huge_value_never_does() {
        let mut app = narrow(20);
        app.single_pane_below = 0;
        assert!(!app.single_pane(), "0 must disable the behaviour entirely");

        let mut app = narrow(400);
        app.single_pane_below = 9999;
        assert!(app.single_pane(), "a huge value must always single-pane");
    }

    /// Widening the terminal may never take a pane away. This is the same rule
    /// `the_header_never_brings_back_what_it_has_already_dropped` enforces, and for
    /// the same reason: a two-stage size calculation once made an element reappear
    /// as the terminal *narrowed*, and nothing about a bigger window should reveal
    /// less.
    #[test]
    fn the_pane_ladder_is_monotone() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.single_pane_below = 80;
        app.repos_pane_above = 110;

        let count = |p: Panes| match p {
            Panes::One => 1,
            Panes::Two => 2,
            Panes::Three => 3,
        };

        let mut last = 0;
        for w in 40..=200u16 {
            app.terminal_width = w;
            let n = count(app.panes());
            assert!(n >= last, "widening to {w} dropped a pane: {last} -> {n}");
            last = n;
        }
    }

    #[test]
    fn the_ladder_hits_each_rung_at_its_threshold() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.single_pane_below = 80;
        app.repos_pane_above = 110;

        app.terminal_width = 79;
        assert_eq!(app.panes(), Panes::One);
        app.terminal_width = 80;
        assert_eq!(app.panes(), Panes::Two);
        app.terminal_width = 109;
        assert_eq!(app.panes(), Panes::Two);
        app.terminal_width = 110;
        assert_eq!(app.panes(), Panes::Three);
    }

    /// `0` disables a rung, matching what `single_pane_below = 0` already means.
    #[test]
    fn zero_disables_a_rung() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.single_pane_below = 0;
        app.repos_pane_above = 0;

        app.terminal_width = 200;
        assert_eq!(app.panes(), Panes::Two, "the repos rung is off");
        app.terminal_width = 20;
        assert_eq!(app.panes(), Panes::Two, "the single-pane rung is off");
    }

    /// Zoom still wins, at any width, as it does today.
    #[test]
    fn zoom_overrides_the_ladder() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.repos_pane_above = 110;
        app.terminal_width = 200;
        app.zoom = Some(true);
        assert_eq!(app.panes(), Panes::One);
    }

    /// With one pane on screen, every column belongs to it. Otherwise the mouse
    /// handlers compare against `divider_x`, which is 0 there, and every click
    /// would land on the detail pane -- including while looking at the tree.
    #[test]
    fn one_pane_owns_every_column() {
        let mut app = narrow(60);
        app.divider_x = 0;

        app.show_tree();
        for col in [0, 5, 30, 59] {
            assert_eq!(app.pane_at(col), Focus::Tree, "column {col}");
        }

        app.show_detail();
        for col in [0, 5, 30, 59] {
            assert_eq!(app.pane_at(col), Focus::Detail, "column {col}");
        }
    }

    #[test]
    fn two_panes_split_at_the_divider() {
        let mut app = narrow(100);
        app.divider_x = 45;
        assert_eq!(app.pane_at(44), Focus::Tree);
        assert_eq!(app.pane_at(45), Focus::Detail);
    }

    /// There is no divider to grab when only one pane is drawn, and a stale one
    /// would be an invisible drag target in the middle of the screen.
    #[test]
    fn there_is_nothing_to_drag_in_one_pane_mode() {
        let mut app = narrow(60);
        app.divider_x = 0;
        for col in [0, 1, 30, 59] {
            assert!(!app.on_divider(col), "column {col} looked like a divider");
        }
    }

    /// `Right` opens what it can, and only falls through to the detail when
    /// there was nothing to open -- which is what its return value is for.
    #[test]
    fn expanding_reports_whether_it_had_anything_to_do() {
        let mut app = narrow(60);

        app.selected = Some("a".into()); // has a child
        app.collapse_all();
        assert!(app.expand_selected(), "a parent has something to open");

        app.selected = Some("b".into()); // a leaf
        assert!(!app.expand_selected(), "a leaf has nothing to open");
    }

    /// Switching panes must not disturb what you were looking at -- the same
    /// rule a background refresh follows.
    #[test]
    fn crossing_to_the_detail_and_back_keeps_the_selection_and_the_tree() {
        let mut app = narrow(60);
        app.selected = Some("b".into());
        app.expanded.insert("a".into());
        let before = (app.selected.clone(), app.expanded.clone());

        app.show_detail();
        app.show_tree();

        assert_eq!((app.selected.clone(), app.expanded.clone()), before);
    }

    fn clickable(zones: Vec<(u16, u16, HeaderZone)>) -> App {
        let mut app = counted(vec![task("a", None, &[])]);
        app.header_zones = zones;
        app
    }

    #[test]
    fn clicking_a_filter_word_selects_that_filter() {
        let mut app = clickable(vec![(10, 12, HeaderZone::Filter(Filter::All))]);
        app.filter = Filter::Pending;

        assert!(app.click_header(11, false));
        assert_eq!(app.filter, Filter::All);
    }

    /// The collapsed label has nothing to pick from, so it advances instead.
    #[test]
    fn clicking_the_collapsed_filter_label_cycles() {
        let mut app = clickable(vec![(0, 6, HeaderZone::FilterCycle)]);
        app.filter = Filter::Pending;

        assert!(app.click_header(3, false));
        assert_eq!(app.filter, Filter::Pending.next());
    }

    /// Mirrors `o` and `O`: the two buttons are the two keys.
    #[test]
    fn the_sort_zone_cycles_on_left_and_reverses_on_right() {
        let mut app = clickable(vec![(4, 11, HeaderZone::Sort)]);
        let order = app.sort;

        assert!(app.click_header(5, false));
        assert_eq!(app.sort, order.next());
        assert!(!app.sort_reversed, "cycling must not also reverse");

        let after_cycle = app.sort;
        assert!(app.click_header(5, true));
        assert!(app.sort_reversed);
        assert_eq!(app.sort, after_cycle, "reversing must not also cycle");
    }

    /// Right-clicking a filter would be a surprise -- only the sort has a second
    /// action -- so it does nothing rather than picking.
    #[test]
    fn the_right_button_does_nothing_outside_the_sort_zone() {
        let mut app = clickable(vec![(10, 12, HeaderZone::Filter(Filter::All))]);
        app.filter = Filter::Pending;

        assert!(!app.click_header(11, true));
        assert_eq!(app.filter, Filter::Pending);
    }

    /// Dead space in the header must stay dead: clicking it cannot steal focus
    /// or disturb the selection, which is what the whole app is built around.
    #[test]
    fn clicking_empty_header_space_changes_nothing() {
        let mut app = clickable(vec![(40, 47, HeaderZone::Sort)]);
        let before = (app.filter, app.sort, app.sort_reversed, app.selected.clone());

        assert!(!app.click_header(3, false));
        assert!(!app.click_header(39, false));
        assert!(!app.click_header(48, false));

        assert_eq!(
            (app.filter, app.sort, app.sort_reversed, app.selected.clone()),
            before
        );
    }

    #[test]
    fn a_zone_covers_its_last_column() {
        let app = clickable(vec![(10, 12, HeaderZone::Sort)]);
        assert_eq!(app.header_zone_at(9), None);
        assert_eq!(app.header_zone_at(10), Some(HeaderZone::Sort));
        assert_eq!(app.header_zone_at(12), Some(HeaderZone::Sort));
        assert_eq!(app.header_zone_at(13), None);
    }

    /// The header mirrors the `dex list --ready` / `dex list --blocked` pair,
    /// **not** `dex status`'s partition. dex disagrees with itself here:
    /// `status.js` counts a parent with unfinished children as blocked, while
    /// `list --blocked` counts only tasks with incomplete blockers. Measured
    /// across four real stores, five of the six tasks `dex status` calls blocked
    /// have no blocker at all -- two of those stores contain no blocking
    /// relationship anywhere and it still reported 3 and 1 blocked.
    ///
    /// So "blocked" here means what the row's own glyph means, what the detail
    /// pane means, and what dex-report's red `[!]` means. One word, one
    /// definition, everywhere.
    #[test]
    fn blocked_counts_only_tasks_with_an_incomplete_blocker() {
        let mut done = task("done", None, &[]);
        done.completed = true;

        let mut live_blocker = task("open", None, &[]);
        live_blocker.name = "open".into();

        let mut stale = task("stale", None, &[]);
        stale.blocked_by = vec!["done".into()]; // blocker finished -> not blocked
        let mut real = task("real", None, &[]);
        real.blocked_by = vec!["open".into()];
        let mut ghost = task("ghost", None, &[]);
        ghost.blocked_by = vec!["nonexistent".into()]; // dangling -> not blocked

        let app = counted(vec![done, live_blocker, stale, real, ghost]);
        assert_eq!(app.counts().blocked, 1, "only `real` is blocked");
    }

    /// A parent with unfinished children is neither ready nor blocked. You
    /// cannot pick up an epic, so counting it ready would be a small lie; it has
    /// no blocker, so calling it blocked would be a bigger one.
    ///
    /// This is the deliberate cost of the rule: ready + blocked does NOT sum to
    /// pending. Asserted rather than tolerated, so nobody "fixes" the gap by
    /// quietly folding parents into one bucket or the other.
    #[test]
    fn a_parent_with_open_children_is_neither_ready_nor_blocked() {
        let parent = task("parent", None, &["kid"]);
        let kid = task("kid", Some("parent"), &[]);

        let app = counted(vec![parent, kid]);
        let c = app.counts();

        assert_eq!(c.pending, 2);
        assert_eq!(c.ready, 1, "only the child can be picked up");
        assert_eq!(c.blocked, 0, "nothing has a blocker");
        assert_ne!(c.ready + c.blocked, c.pending, "the gap is the parent");
    }

    #[test]
    fn a_parent_whose_children_are_all_done_is_ready_again() {
        let parent = task("parent", None, &["kid"]);
        let mut kid = task("kid", Some("parent"), &[]);
        kid.completed = true;

        let app = counted(vec![parent, kid]);
        assert_eq!(app.counts().ready, 1, "nothing is holding the parent up now");
    }

    /// In progress is its own bucket and is never also counted ready or blocked,
    /// matching how `status.js` partitions: it tests in-progress first.
    #[test]
    fn a_started_task_counts_as_active_and_nothing_else() {
        let mut started = task("started", None, &[]);
        started.started_at = Some("2026-01-01T00:00:00Z".into());
        started.blocked_by = vec!["open".into()];
        let open = task("open", None, &[]);

        let app = counted(vec![started, open]);
        let c = app.counts();

        assert_eq!(c.active, 1);
        assert_eq!(c.blocked, 0, "started wins over blocked");
        assert_eq!(c.ready, 1, "only `open` is ready");
    }

    /// The percentage is the one number `dex status` and `dex-report` agree on,
    /// so it must not drift: completed over total, archived aside.
    #[test]
    fn the_percentage_is_completed_over_everything() {
        let mut a = task("a", None, &[]);
        a.completed = true;
        let b = task("b", None, &[]);
        let c = task("c", None, &[]);
        let d = task("d", None, &[]);

        let app = counted(vec![a, b, c, d]);
        assert_eq!(app.counts().percent, 25);
    }

    #[test]
    fn an_empty_store_reports_zero_percent_rather_than_dividing_by_zero() {
        let app = counted(vec![]);
        assert_eq!(app.counts().percent, 0);
    }

    /// Counts come from the unfiltered list, like the progress rollups, so
    /// changing what is on screen never changes what the header reports.
    #[test]
    fn the_counts_ignore_the_current_filter() {
        let mut done = task("done", None, &[]);
        done.completed = true;
        let open = task("open", None, &[]);

        let mut app = counted(vec![done, open]);
        let before = app.counts();

        app.filter = Filter::InProgress;
        app.rebuild();

        assert_eq!(app.counts().percent, before.percent);
        assert_eq!(app.counts().ready, before.ready);
    }

    fn app_with(tasks: Vec<Task>, selected: &str) -> App {
        let mut app = App::new(tasks, "test".into(), Config::default());
        app.selected = Some(selected.to_string());
        app
    }

    #[test]
    fn selection_survives_a_refresh_that_changes_nothing() {
        let tasks = vec![task("a", None, &[]), task("b", None, &[])];
        let mut app = app_with(tasks.clone(), "b");

        app.apply_tasks(tasks);

        assert_eq!(app.selected.as_deref(), Some("b"));
    }

    #[test]
    fn selection_survives_when_unrelated_tasks_are_added() {
        // The exact scenario of an agent creating tasks while you are reading.
        let mut app = app_with(vec![task("a", None, &[]), task("b", None, &[])], "b");

        app.apply_tasks(vec![
            task("a", None, &[]),
            task("b", None, &[]),
            task("new1", None, &[]),
            task("new2", None, &[]),
        ]);

        assert_eq!(app.selected.as_deref(), Some("b"));
    }

    #[test]
    fn new_tasks_arrive_collapsed_so_the_tree_does_not_explode() {
        let mut app = app_with(vec![task("a", None, &[])], "a");

        app.apply_tasks(vec![
            task("a", None, &[]),
            task("newparent", None, &["kid"]),
            task("kid", Some("newparent"), &[]),
        ]);

        assert!(!app.expanded.contains("newparent"));
    }

    #[test]
    fn expansion_is_dropped_only_for_tasks_that_disappeared() {
        let mut app = app_with(
            vec![task("a", None, &["k"]), task("k", Some("a"), &[]), task("gone", None, &[])],
            "a",
        );
        app.expanded.insert("gone".to_string());

        app.apply_tasks(vec![task("a", None, &["k"]), task("k", Some("a"), &[])]);

        assert!(app.expanded.contains("a"));
        assert!(!app.expanded.contains("gone"));
    }

    #[test]
    fn a_deleted_selection_falls_back_to_its_next_sibling() {
        let mut app = app_with(
            vec![
                task("parent", None, &["s1", "s2", "s3"]),
                task("s1", Some("parent"), &[]),
                task("s2", Some("parent"), &[]),
                task("s3", Some("parent"), &[]),
            ],
            "s2",
        );

        app.apply_tasks(vec![
            task("parent", None, &["s1", "s3"]),
            task("s1", Some("parent"), &[]),
            task("s3", Some("parent"), &[]),
        ]);

        // Stays where the cursor visually was, rather than jumping to the top.
        assert_eq!(app.selected.as_deref(), Some("s3"));
    }

    #[test]
    fn a_deleted_last_sibling_falls_back_to_the_previous_one() {
        let mut app = app_with(
            vec![
                task("parent", None, &["s1", "s2"]),
                task("s1", Some("parent"), &[]),
                task("s2", Some("parent"), &[]),
            ],
            "s2",
        );

        app.apply_tasks(vec![
            task("parent", None, &["s1"]),
            task("s1", Some("parent"), &[]),
        ]);

        assert_eq!(app.selected.as_deref(), Some("s1"));
    }

    #[test]
    fn when_the_whole_branch_is_gone_selection_climbs_to_a_surviving_ancestor() {
        let mut app = app_with(
            vec![
                task("root", None, &["mid"]),
                task("mid", Some("root"), &["leaf"]),
                task("leaf", Some("mid"), &[]),
            ],
            "leaf",
        );

        // Both mid and leaf removed; only root survives.
        app.apply_tasks(vec![task("root", None, &[])]);

        assert_eq!(app.selected.as_deref(), Some("root"));
    }

    #[test]
    fn an_empty_store_yields_no_selection() {
        let mut app = app_with(vec![task("a", None, &[])], "a");
        app.apply_tasks(vec![]);

        assert_eq!(app.selected, None);
    }

    #[test]
    fn a_selected_root_that_is_deleted_moves_to_another_root() {
        let mut app = app_with(vec![task("r1", None, &[]), task("r2", None, &[])], "r1");
        app.apply_tasks(vec![task("r2", None, &[])]);

        assert_eq!(app.selected.as_deref(), Some("r2"));
    }

    #[test]
    fn first_load_expands_everything_so_the_tree_is_visible() {
        // Regression: the collapse-new-tasks rule once applied to first load too,
        // which opened the app onto a single collapsed root.
        let app = App::new(
            vec![task("root", None, &["kid"]), task("kid", Some("root"), &[])],
            "test".into(),
            Config::default(),
        );

        assert!(app.expanded.contains("root"));
        assert_eq!(app.row_ids().len(), 2);
    }

    fn started(id: &str) -> Task {
        Task {
            started_at: Some("2026-01-01T00:00:00Z".to_string()),
            ..task(id, None, &[])
        }
    }

    /// The strongest form of the idle-cost guard, written as an exact repaint
    /// count rather than a comment: a store with nothing running must never ask
    /// for a frame, no matter how long it sits there.
    #[test]
    fn an_idle_store_never_repaints_itself() {
        let mut app = app_with(vec![task("a", None, &[]), task("b", None, &[])], "a");

        for ms in (0..5000).step_by(37) {
            assert!(
                !app.pulse_tick(std::time::Duration::from_millis(ms), 10),
                "an idle store asked to repaint at {ms}ms"
            );
        }
    }

    /// The cost budget, written down as an exact number rather than "it ticks".
    ///
    /// A spinner is far more expensive than the colour breath it replaced: 80ms
    /// frames against a 700ms half-period is 12.5 repaints/sec rather than ~1.4,
    /// roughly nine times the work **while a task is running**. That is the
    /// price of motion in the glyph, and it is deliberate -- but it is only ever
    /// paid then, which is what `an_idle_store_never_repaints_itself` guards.
    #[test]
    fn a_running_store_repaints_once_per_frame() {
        let mut app = app_with(vec![started("a")], "a");

        let repaints = (0..2800)
            .filter(|ms| app.pulse_tick(std::time::Duration::from_millis(*ms), 10))
            .count();

        // 2800ms / 80ms, less the tick at 0 which is already the resting frame.
        assert_eq!(repaints, 34, "12.5 repaints/sec is the whole budget");
    }

    /// `started_at` survives completion, so a naive `started_at.is_some()` would
    /// animate a finished store forever.
    #[test]
    fn a_completed_task_does_not_keep_the_pulse_running() {
        let mut done = started("a");
        done.completed = true;
        let app = app_with(vec![done], "a");

        assert!(!app.is_animating());
    }

    /// The opt-out must reach the idle cost, not merely the colour.
    #[test]
    fn turning_animation_off_stops_the_tick_even_with_work_in_progress() {
        let mut app = app_with(vec![started("a")], "a");
        assert!(app.is_animating(), "fixture should animate to begin with");

        app.animate = false;

        assert!(!app.is_animating());
        for ms in (0..3000).step_by(50) {
            assert!(!app.pulse_tick(std::time::Duration::from_millis(ms), 10), "{ms}ms");
        }
    }

    /// Switching away and back must return the cursor to where it was, or the
    /// pane is tedious for exactly the comparison it exists to serve.
    #[test]
    fn each_worktree_remembers_where_you_were() {
        let mut app = counted(vec![task("a", None, &[]), task("b", None, &[])]);
        app.selected_worktree = Some("/x/one".into());
        app.selected = Some("b".into());

        app.select_worktree("/x/two");
        assert_eq!(app.selected_worktree.as_deref(), Some("/x/two"));

        app.selected = Some("a".into());
        app.select_worktree("/x/one");
        assert_eq!(
            app.selected.as_deref(),
            Some("b"),
            "returning to a worktree lost the task selection"
        );
    }

    /// The refresh invariant, extended: a refresh may not move the worktree either.
    #[test]
    fn a_refresh_never_changes_the_selected_worktree() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.selected_worktree = Some("/x/one".into());

        app.apply_tasks(vec![task("a", None, &[]), task("c", None, &[])]);

        assert_eq!(app.selected_worktree.as_deref(), Some("/x/one"));
    }

    #[test]
    fn selecting_the_same_worktree_twice_is_harmless() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.selected_worktree = Some("/x/one".into());
        app.selected = Some("a".into());

        app.select_worktree("/x/one");
        assert_eq!(app.selected.as_deref(), Some("a"));
    }

    /// Otherwise `,`-reload would silently ignore the key, which is exactly the
    /// moment a preference file is meant to win.
    #[test]
    fn reloading_the_config_carries_the_animate_setting() {
        let mut app = app_with(vec![started("a")], "a");
        assert!(app.animate);

        app.apply_config(Config {
            animate: false,
            ..Config::default()
        });

        assert!(!app.animate);
        assert!(!app.is_animating());
    }

    /// When the last in-progress task finishes, the spinner must settle rather
    /// than freeze on whichever frame happened to be up.
    #[test]
    fn the_spinner_settles_when_the_last_task_stops() {
        let mut app = app_with(vec![started("a")], "a");
        app.pulse_tick(std::time::Duration::from_millis(720), 10);
        assert_eq!(app.spin_frame, 9, "fixture should be mid-rotation");

        let mut done = started("a");
        done.completed = true;
        app.apply_tasks(vec![done]);

        assert!(
            app.pulse_tick(std::time::Duration::from_millis(720), 10),
            "the settling frame is a repaint"
        );
        assert_eq!(app.spin_frame, 0, "must return to rest, not freeze");
    }

    #[test]
    fn text_input_edits_by_character_not_byte() {
        // Multi-byte characters must not corrupt the buffer.
        let mut input = TextInput::new("héllo");
        input.backspace();
        assert_eq!(input.value, "héll");

        input.left();
        input.insert('X');
        assert_eq!(input.value, "hélXl");
    }

    #[test]
    fn tab_moves_focus_between_the_panes() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        assert_eq!(app.focus, Focus::Tree);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Detail);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Tree);
    }

    #[test]
    fn detail_scroll_is_clamped_to_the_content() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.detail_content_height = 30;
        app.detail_viewport_height = 10;

        app.scroll_detail(100, 0);
        assert_eq!(app.detail_scroll.0, 20, "cannot scroll past the last row");

        app.scroll_detail(-100, 0);
        assert_eq!(app.detail_scroll.0, 0, "cannot scroll above the first row");
    }

    #[test]
    fn content_shorter_than_the_pane_does_not_scroll() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.detail_content_height = 4;
        app.detail_viewport_height = 20;

        app.scroll_detail(5, 0);
        assert_eq!(app.detail_scroll.0, 0);
    }

    #[test]
    fn horizontal_scroll_is_ignored_while_wrapping() {
        // Wrapping removes the overflow there would be anything to scroll to,
        // so accepting an offset would just move content off-screen for nothing.
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        assert!(app.wrap);

        app.scroll_detail(0, 10);
        assert_eq!(app.detail_scroll.1, 0);

        app.toggle_wrap();
        app.scroll_detail(0, 10);
        assert_eq!(app.detail_scroll.1, 10);
    }

    #[test]
    fn turning_wrap_back_on_resets_the_horizontal_offset() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.toggle_wrap();
        app.scroll_detail(0, 12);
        assert_eq!(app.detail_scroll.1, 12);

        app.toggle_wrap();
        assert_eq!(app.detail_scroll.1, 0, "a stale offset would hide content");
    }

    #[test]
    fn selecting_a_different_task_resets_the_scroll() {
        // Otherwise you land halfway down a task you have not read yet.
        let mut app = App::new(
            vec![task("a", None, &[]), task("b", None, &[])],
            "t".into(),
            Config::default(),
        );
        app.detail_content_height = 50;
        app.detail_viewport_height = 10;
        app.scroll_detail(20, 0);
        assert_ne!(app.detail_scroll.0, 0);

        app.move_selection(1);
        assert_eq!(app.detail_scroll, (0, 0));
    }

    #[test]
    fn re_selecting_the_same_task_keeps_your_place() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.detail_content_height = 50;
        app.detail_viewport_height = 10;
        app.scroll_detail(15, 0);

        app.select_first(); // already selected
        assert_eq!(app.detail_scroll.0, 15);
    }

    fn geo(app: &mut App) {
        // Stand-in for what the renderer publishes each frame.
        app.terminal_width = 100;
        app.divider_x = 45;
        app.body_top = 1;
        app.body_bottom = 21;
    }

    #[test]
    fn the_divider_is_grabbable_without_pixel_precision() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);

        for col in [44, 45, 46] {
            assert!(app.on_divider(col), "column {col} should grab the divider");
        }
        for col in [10, 43, 47, 90] {
            assert!(!app.on_divider(col), "column {col} should not");
        }
    }

    #[test]
    fn dragging_cannot_collapse_either_pane() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);

        app.set_split(0, 100);
        assert_eq!(app.split_percent, 20, "tree pane collapsed");

        app.set_split(100, 100);
        assert_eq!(app.split_percent, 80, "detail pane collapsed");

        app.set_split(60, 100);
        assert_eq!(app.split_percent, 60);
    }

    #[test]
    fn a_zero_width_terminal_does_not_divide_by_zero() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        let before = app.split_percent;
        app.set_split(10, 0);
        assert_eq!(app.split_percent, before);
    }

    #[test]
    fn clicking_a_row_selects_the_task_drawn_there() {
        let mut app = App::new(
            vec![task("a", None, &[]), task("b", None, &[]), task("c", None, &[])],
            "t".into(),
            Config::default(),
        );
        geo(&mut app);

        // body_top is the border, so the first task is on the next row.
        app.select_at_row(app.body_top + 1);
        assert_eq!(app.selected.as_deref(), Some("a"));

        app.select_at_row(app.body_top + 3);
        assert_eq!(app.selected.as_deref(), Some("c"));
    }

    #[test]
    fn clicking_past_the_last_row_changes_nothing() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);

        app.select_at_row(app.body_top + 15);
        assert_eq!(app.selected.as_deref(), Some("a"), "selection moved to nothing");
    }

    #[test]
    fn clicking_the_border_row_selects_nothing_rather_than_the_first_task() {
        let mut app = App::new(
            vec![task("a", None, &[]), task("b", None, &[])],
            "t".into(),
            Config::default(),
        );
        geo(&mut app);
        app.select(Some("b".into()));

        app.select_at_row(app.body_top);
        assert_eq!(app.selected.as_deref(), Some("b"), "border click moved selection");
    }

    #[test]
    fn a_scrolled_list_maps_clicks_through_the_offset() {
        // Without honouring the offset, every click would address the top of the
        // list rather than what is actually drawn.
        let mut app = App::new(
            vec![task("a", None, &[]), task("b", None, &[]), task("c", None, &[])],
            "t".into(),
            Config::default(),
        );
        geo(&mut app);
        app.tree_offset = 2;

        app.select_at_row(app.body_top + 1);
        assert_eq!(app.selected.as_deref(), Some("c"));
    }

    /// The same gesture has to do the same thing in both panes. Moving only the
    /// selection left the tree's *content* stationary while the cursor travelled
    /// against the direction of the fingers -- so with the detail pane sliding
    /// with them, one drag read forwards on the right and backwards on the left.
    /// Now both slide their content, and the tree's cursor keeps its screen row.
    #[test]
    fn a_drag_slides_both_panes_the_same_way() {
        let tasks: Vec<Task> = ('a'..='j')
            .map(|c| task(&c.to_string(), None, &[]))
            .collect();
        let mut app = App::new(tasks, "t".into(), Config::default());
        geo(&mut app);
        app.detail_content_height = 100;
        app.detail_viewport_height = 10;
        app.tree_offset = 3;
        app.select(Some("f".into()));
        app.detail_scroll = (3, 0);

        let screen_row = |a: &App| a.selected_row().unwrap() - a.tree_offset;
        let before = screen_row(&app);

        // The detail pane is measured on its own: moving the tree's selection
        // deliberately resets it, so driving both from one state would prove
        // nothing about direction.
        app.scroll_detail(2, 0);
        assert_eq!(app.detail_scroll.0, 5, "the detail's content did not move");
        app.scroll_detail(-2, 0);
        assert_eq!(app.detail_scroll.0, 3, "the detail did not come back");

        // Same sign, same direction: the offset grows, so later rows come into
        // view from the bottom, exactly as later lines do on the right.
        app.scroll_tree(2);
        assert_eq!(app.tree_offset, 5, "the tree's content did not move");
        assert_eq!(
            screen_row(&app),
            before,
            "the cursor should hold its place on screen while the list slides"
        );

        app.scroll_tree(-2);
        assert_eq!(app.tree_offset, 3, "the tree did not come back");
        assert_eq!(screen_row(&app), before);
    }

    /// Scrolling past the end must not run the offset off into blank space: the
    /// selection clamps, and the offset has to clamp with it or the cursor would
    /// be scrolled out of the list it is selecting from.
    #[test]
    fn scrolling_past_the_ends_of_the_tree_stops() {
        let tasks: Vec<Task> = ('a'..='e')
            .map(|c| task(&c.to_string(), None, &[]))
            .collect();
        let mut app = App::new(tasks, "t".into(), Config::default());
        geo(&mut app);

        app.scroll_tree(50);
        assert_eq!(app.selected.as_deref(), Some("e"), "should rest on the last row");
        assert!(app.tree_offset <= 4, "offset ran past the list: {}", app.tree_offset);

        app.scroll_tree(-50);
        assert_eq!(app.selected.as_deref(), Some("a"));
        assert_eq!(app.tree_offset, 0);
    }

    #[test]
    fn in_body_excludes_the_header_and_status_rows() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);

        assert!(!app.in_body(0), "header row");
        assert!(app.in_body(1));
        assert!(app.in_body(20));
        assert!(!app.in_body(21), "status row");
    }
}
