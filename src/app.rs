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
    Complete {
        id: String,
    },
    CreateName {
        parent: Option<String>,
    },
    CreateDescription {
        parent: Option<String>,
        name: String,
    },
    EditName {
        id: String,
    },
    /// A path typed into the sidebar's "save a repo" prompt.
    ///
    /// The only prompt that is not about a task, which is why it carries
    /// nothing: everything it needs is what was typed.
    SaveRepo,
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
    Confirm {
        id: String,
        message: String,
    },
    /// Offered when dex refuses to complete a task with unfinished subtasks.
    ForceComplete {
        id: String,
        result: String,
        message: String,
    },
    Error(String),
    Help,
}

/// Which pane the movement keys drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Detail,
    /// The repo/worktree sidebar. `1` goes straight there, and `Tab` includes
    /// it in the cycle whenever it is on screen -- see `focus_cycle`.
    Repos,
}

/// A draggable pane boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Divider {
    /// Between the repo sidebar and the task tree.
    Repos,
    /// Between the task tree and the detail pane.
    Split,
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
    /// One word of the sort menu. Picks that sort outright.
    Sort(Sort),
    /// The lone sort name a narrow header falls back to. Left cycles the order,
    /// right reverses it -- mirroring `o` and `O`.
    SortCycle,
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
    /// The store directory the task list came from -- what `store_label` is a
    /// display name *of*. Kept alongside it because a label is ambiguous (two
    /// projects can share a directory name) and because an in-flight refresh
    /// has to be matched against the store it read, not against how that store
    /// is spelled on screen. See `Msg::Tasks` in `main.rs`.
    pub store_dir: String,
    pub should_quit: bool,
    /// Set by `e`; the main loop picks it up and hands off to $EDITOR, which
    /// cannot happen mid-draw because the terminal has to be released first.
    pub pending_editor: Option<String>,
    /// Set by `,`; the main loop opens the config file in $EDITOR and reloads.
    pub pending_config_edit: bool,
    /// Set by `enter`/`l` in the repo pane; the main loop picks it up and
    /// swaps which store the task panes read. Handled the same way as
    /// `pending_editor`: `handle_key` only ever sees `&Arc<Dex>`, so replacing
    /// what it points to has to happen one level up, in `main`.
    pub pending_store: Option<String>,
    /// Width of the tree pane as a percentage. Dragged with the mouse.
    pub split_percent: u16,
    /// Which divider is being dragged, if any.
    pub dragging: Option<Divider>,
    /// The sidebar's width in cells. A `Length` in the layout rather than a
    /// share, so it does not grow with the terminal -- see `set_repos_width`.
    pub repos_width: u16,
    /// Geometry the renderer publishes so mouse maths can be exact rather than
    /// re-derived from assumptions about the layout.
    pub divider_x: u16,
    /// The first column *after* the repo sidebar, or 0 when the sidebar is not
    /// drawn as its own pane. Published for the same reason `divider_x` is:
    /// the sidebar's width is a `ui` constant, and mouse maths that re-derived
    /// it here would be a second copy free to drift from the layout.
    pub repos_right: u16,
    pub body_top: u16,
    pub body_bottom: u16,
    pub terminal_width: u16,
    /// The list's scroll offset, kept across frames so a click maps to the row
    /// actually under the cursor.
    pub tree_offset: usize,
    /// Set by `select` whenever `self.selected` actually changes; consumed by
    /// the very next `draw_tree` frame, which is the one and only frame that
    /// tells `ratatui::List` to scroll the real selection into view.
    ///
    /// This exists because that scroll-into-view is not something `draw_tree`
    /// can safely do unconditionally. `ratatui::List` re-derives `tree_offset`
    /// from whatever `ListState` calls selected every single time it renders,
    /// snapping the offset back the moment that row would fall outside the
    /// window -- and a running task's spinner redraws many times a second
    /// with no selection change at all, so "unconditionally" means "on every
    /// one of those frames too." A wheel scroll that moved `tree_offset` away
    /// from the selected row would survive exactly one frame before the very
    /// next animation tick pulled it straight back -- indistinguishable from
    /// the scroll never having worked. Limiting the reveal to the frame where
    /// the selection actually moved is what makes a scroll that carries the
    /// cursor off-screen stick.
    pub needs_tree_reveal: bool,
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
    /// Vertical offset into the `?` dialog. `HELP` is longer and wider than a
    /// small terminal, and `centered` clamps the dialog to the frame, so
    /// without this the text simply stopped at the border with nothing on
    /// screen to say it had -- the same silent truncation the fixed 74x16 box
    /// before it was replaced for.
    pub help_scroll: u16,
    /// Written by the renderer, for the same reason `detail_content_height` is:
    /// the help wraps, so its height depends on the dialog's width and only the
    /// renderer can measure it.
    pub help_content_height: u16,
    pub help_viewport_height: u16,
    /// Set when a repo is added mid-run, so the main loop can give its stores
    /// a watcher and a first read. `App` owns view state, not I/O -- the same
    /// division `pending_store` follows.
    pub repos_changed: bool,
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
    /// Whether the sidebar is shown, set by `b`/`1` and seeded at startup from
    /// `Config::repos_open`. Plain `bool`, not `Option`: there used to be a
    /// third state, `None`, meaning "decide by width" -- but once every path
    /// that sets this (`App::new`, a `,` reload) always supplies a concrete
    /// `repos_open` value, width-decided was no longer reachable by anything
    /// other than a test poking the field, which is a sign the state itself
    /// should go rather than be kept alive for its own sake. `repos_pane_above`
    /// still matters, in `room_for_three` -- once shown, does it get a third
    /// pane, or does the detail yield -- just never for *whether* it is shown.
    pub repos_visible: bool,
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
    pub selected_worktree: Option<String>,
    /// Task selection per worktree path, so switching back returns the cursor.
    /// Session-only: this is view state, not something to persist.
    pub task_memory: HashMap<String, String>,
    /// Registered repos with their worktrees, and whether each is expanded.
    pub repos: Vec<crate::repos::Repo>,
    pub selected_repo_row: usize,
    /// The repo pane's own scroll offset, carried across frames the same way
    /// `tree_offset` is -- without it, `G`/`PageDown` could select a row
    /// below the visible area with nothing on screen ever moving to show it.
    pub repos_offset: usize,
    /// Set by Ctrl-L: repaint every cell rather than only what changed.
    ///
    /// ratatui's `draw` diffs against the buffer *it* last drew, so anything
    /// that corrupts the screen from outside the app -- a terminal that drops
    /// output, a multiplexer redrawing a pane, another process writing over it
    /// -- leaves cells ratatui believes are already correct and will therefore
    /// never rewrite. The screen then stays wrong indefinitely, because the
    /// app also only draws when something *it* knows about has changed.
    pub force_redraw: bool,
    pub registry: crate::registry::Registry,
    /// The repo dextui was launched in, and the store it resolved there.
    ///
    /// Both fixed for the run: `here` means where you are, which switching
    /// stores does not change. Empty on a launch that resolved no repo.
    pub here_path: Option<String>,
    pub here_store: String,
    /// Every sidebar store's task list, keyed by store directory.
    ///
    /// This is what lets moving the sidebar cursor change the panes as
    /// immediately as moving the tree cursor changes the detail -- one model
    /// for both, rather than two that look identical and are not. A switch is
    /// a lookup here, not a `dex list`.
    ///
    /// It costs nothing extra to keep: the startup join and every watcher
    /// update already fetch the whole list for each store and used to reduce
    /// it to counts on arrival, discarding exactly the thing a switch then
    /// paid ~180ms to fetch again.
    pub store_tasks: HashMap<String, Vec<Task>>,
}

impl App {
    /// `store_dir` is the directory dex resolved, not a display name: the
    /// label is derived from it here, the same way `load_store` does it, so
    /// the two can never be set to different stores. See `App::store_dir`.
    pub fn new(tasks: Vec<Task>, store_dir: String, cfg: Config) -> Self {
        // Captured before the field takes ownership: `here_store` is the store
        // this run *launched* with, and never changes with `load_store`.
        let here_store = store_dir.clone();
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
            store_label: crate::dex::store_label(&store_dir),
            store_dir,
            should_quit: false,
            pending_editor: None,
            pending_config_edit: false,
            pending_store: None,
            split_percent: cfg.split_percent,
            dragging: None,
            repos_width: cfg.repos_width,
            divider_x: 0,
            repos_right: 0,
            body_top: 0,
            body_bottom: 0,
            terminal_width: 0,
            tree_offset: 0,
            // `true` so the first frame reveals the initial selection -- moot
            // in practice, since `App::new` always starts it at row 0 with the
            // offset already there too, but there is no earlier "selection
            // changed" edge to have set this from.
            needs_tree_reveal: true,
            focus: Focus::Tree,
            detail_scroll: (0, 0),
            wrap: cfg.wrap,
            detail_content_height: 0,
            detail_viewport_height: 0,
            help_scroll: 0,
            help_content_height: 0,
            help_viewport_height: 0,
            repos_changed: false,
            pending_refresh: false,
            animate: cfg.animate,
            spin_frame: 0,
            single_pane_below: cfg.single_pane_below,
            repos_pane_above: cfg.repos_pane_above,
            repos_visible: cfg.repos_open,
            zoom: None,
            header_zones: Vec::new(),
            selected_worktree: None,
            task_memory: HashMap::new(),
            repos: Vec::new(),
            selected_repo_row: 0,
            repos_offset: 0,
            force_redraw: false,
            registry: crate::registry::Registry::default(),
            here_path: None,
            here_store,
            store_tasks: HashMap::new(),
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
    /// gesture; the selected task does not change, exactly as the detail pane's
    /// own text scrolls under a stationary reading position.
    ///
    /// This used to move the selection by the same delta as the offset, so the
    /// cursor held its *screen row* while the task underneath it changed --
    /// which reads as "the wheel keeps picking a different task." A person
    /// scrolling a list wants to look further down it, not to have their
    /// selection wander off to whatever task the gesture happened to land the
    /// view on. The selected task now stays selected, on or off screen, until
    /// something that is actually a selection gesture -- a keypress, a click --
    /// changes it.
    ///
    /// The offset clamps against the row count, not the viewport height, which
    /// this type does not know. Overshooting is harmless: it does not touch
    /// `self.selected`, so `needs_tree_reveal` stays however the last real
    /// selection change left it -- see that field's doc for why leaving it
    /// alone here (rather than setting it) is what makes the scroll stick
    /// instead of snapping back on the next animation frame.
    pub fn scroll_tree(&mut self, delta: isize) {
        let rows = self.row_ids();
        if rows.is_empty() {
            return;
        }
        let last = rows.len() as isize - 1;
        self.tree_offset = (self.tree_offset as isize + delta).clamp(0, last) as usize;
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
            && self.row_ids().contains(&parent)
        {
            self.selected = Some(parent);
        }
    }

    /// Applies a freshly fetched task list without disturbing the user.
    pub fn apply_tasks(&mut self, next: Vec<Task>) {
        let next_ids: HashSet<String> = next.iter().map(|t| t.id.clone()).collect();

        // Keep expansion only for tasks that still exist. Tasks added since the
        // last refresh are absent here, so new work arrives collapsed and an
        // agent creating subtasks cannot explode the tree under the cursor.
        //
        // The exception is a task that has children *and did not a moment ago*
        // -- whether because it is brand new or because it just sprouted them.
        // Both are the same event to a reader: a thing that was one row is now
        // a branch, and the subtasks are the entire reason it changed. Hiding
        // them behind a twisty that was not there a second ago costs a click to
        // see what showed up.
        //
        // The rule is deliberately about gaining the *first* children, not
        // about having any, and the dividing line is whether there is intent to
        // overrule. A leaf has no twisty and `expanded` never held it, so
        // leaving it collapsed preserves nothing. A parent you collapsed by
        // hand is the opposite: that is a decision, and a fifth child arriving
        // must not undo it.
        //
        // `self.by_id` still holds the *previous* refresh at this point, which
        // is what makes "did not have children a moment ago" answerable at all.
        self.expanded.retain(|id| next_ids.contains(id));
        for t in &next {
            let had_children = self
                .by_id
                .get(&t.id)
                .is_some_and(|prev| !prev.children.is_empty());
            if !t.children.is_empty() && !had_children {
                self.expanded.insert(t.id.clone());
            }
        }

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
                && next_ids.contains(after)
            {
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
    /// Why the tree is empty, phrased for the pane that has to say so -- or
    /// `None` when it is not empty and nothing needs explaining.
    ///
    /// An empty tree has four causes that look identical and are not. The one
    /// this exists for is the last: **the query is answered, by tasks the
    /// filter is hiding.** Drawing nothing there reads as the search being
    /// broken, and most sharply when the query is an id, because an id is a
    /// thing you know exists -- you copied it from the pane next door.
    ///
    /// It names the filter and counts what it is hiding rather than saying
    /// something vague, on the same reasoning as `Filter::name`: a filter
    /// silently hiding tasks with nothing on screen saying so is the most
    /// confusing state this app has.
    ///
    /// Deliberately *not* a bypass. Letting an id match jump the filter would
    /// put a completed task in a tree whose header says `pending`, and a pane
    /// contradicting its own header is the failure this codebase treats as
    /// worse than showing nothing. One keypress, named on screen, is the
    /// cheaper answer -- and it is what GitHub and Jira do with a closed issue
    /// under an `is:open` search.
    pub fn empty_reason(&self) -> Option<String> {
        if !self.tree.is_empty() {
            return None;
        }
        if self.tasks.is_empty() {
            return Some("No tasks yet.\n\nPress n to create one.".into());
        }

        let q = self.query.value.trim();
        if q.is_empty() {
            return Some(format!(
                "No tasks match the {} filter.\n\nPress f to change it.",
                self.filter.name()
            ));
        }

        let hidden = self
            .tasks
            .iter()
            .filter(|t| tree::matches_query(t, q))
            .count();
        Some(if hidden == 0 {
            format!("Nothing matches \"{q}\".\n\nPress esc to clear the search.")
        } else {
            // "Press f to show it" was the first wording and it was not true.
            // `f` cycles pending -> active -> all, so from `pending` one press
            // lands on `active`, which hides a completed task just as firmly.
            // Naming `all` is the only advice that always works, since it is
            // the one filter that hides nothing -- and being told to press a
            // key that does not fix it is worse than being told nothing.
            format!(
                "\"{q}\" matches {hidden} task{}, hidden by the {} filter.\n\n\
                 Press f until the filter reads all.",
                if hidden == 1 { "" } else { "s" },
                self.filter.name(),
            )
        })
    }

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
        // Reuses `self.by_id` rather than building a fresh index: this runs
        // every frame for the header, and `counts_for` below is the version
        // that pays the indexing cost, for a task list with no `App` of its
        // own to cache one.
        counts_from(&self.tasks, &self.by_id)
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
        // A percentage **of the region the two panes share**, which is the
        // body minus the sidebar -- so both ends of the fraction describe the
        // same span. Measuring the pointer from the body's left edge while the
        // layout measured the percentage from the sidebar's right was what
        // threw the divider a full sidebar-width past the pointer the moment
        // you grabbed it.
        let span = total_width.saturating_sub(self.repos_right);
        if span == 0 {
            return;
        }
        let x = column.saturating_sub(self.repos_right);
        let pct = (x as f32 / span as f32 * 100.0).round() as i32;
        self.split_percent = pct.clamp(20, 80) as u16;
    }

    /// Sets the sidebar's width from a dragged column.
    ///
    /// A width rather than a percentage, unlike the tree/detail split: the
    /// sidebar holds names, not prose, so it neither wants nor needs to grow
    /// with the terminal -- which is the same reason it is a `Length` in the
    /// layout.
    pub fn set_repos_width(&mut self, column: u16, total_width: u16) {
        // Never wider than half the terminal, so the pane it exists to
        // navigate *to* cannot be squeezed out by the pane doing the
        // navigating.
        let cap = (total_width / 2).max(Self::REPOS_WIDTH_MIN);
        self.repos_width = column.clamp(Self::REPOS_WIDTH_MIN, cap);
    }

    /// Narrow enough to be useful, wide enough to still show a branch name.
    pub const REPOS_WIDTH_MIN: u16 = 12;

    /// Which divider `column` is on or beside, if any -- grabbable without
    /// demanding single-cell precision.
    pub fn divider_at(&self, column: u16) -> Option<Divider> {
        // The sidebar's edge is tested first: with the sidebar collapsed to
        // its minimum on a narrow terminal the two dividers can be within a
        // cell of each other, and the one you cannot otherwise reach should
        // win.
        if self.repos_right > 0 && column.abs_diff(self.repos_right.saturating_sub(1)) <= 1 {
            return Some(Divider::Repos);
        }
        if self.divider_x > 0 && column.abs_diff(self.divider_x) <= 1 {
            return Some(Divider::Split);
        }
        None
    }

    pub fn in_body(&self, row: u16) -> bool {
        row >= self.body_top && row < self.body_bottom
    }

    /// Which item a pane's list drew on `row`, given the offset it was drawn
    /// with -- or `None` if `row` is not one of its item rows at all.
    ///
    /// Both borders have to be excluded, and only one of them is excluded by
    /// arithmetic that looks obviously right. `in_body` bounds the *body*,
    /// whose last row is the pane's bottom border, so `row - (body_top + 1)`
    /// mapped that border to one index past the last item drawn: clicking
    /// `└───┘` selected a task that was not on screen, and then scrolled the
    /// list to reveal what you had supposedly just clicked. The top border
    /// escaped only because `checked_sub` happens to reject it.
    fn list_row_index(&self, row: u16, offset: usize) -> Option<usize> {
        if row + 1 >= self.body_bottom {
            return None;
        }
        row.checked_sub(self.body_top + 1)
            .map(|r| r as usize + offset)
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
            HeaderZone::Sort(s) if secondary => {
                self.sort = s;
                self.sort_reversed = !self.sort_reversed;
            }
            HeaderZone::Sort(s) => self.sort = s,
            HeaderZone::SortCycle if secondary => self.sort_reversed = !self.sort_reversed,
            HeaderZone::SortCycle => self.sort = self.sort.next(),
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
        let Some(index) = self.list_row_index(row, self.tree_offset) else {
            return;
        };

        let rows = self.row_ids();
        if let Some(id) = rows.get(index) {
            self.select(Some(id.clone()));
        }
    }

    /// The selection gutter `ui::draw_tree` draws before every row's prefix --
    /// two cells whether or not the cursor is on that row, so a name cannot
    /// shift out of the column its siblings sit in.
    const TREE_GUTTER: u16 = 2;

    /// The columns a tree row's expand/collapse marker occupies, given the
    /// tree-drawing prefix it was rendered with.
    ///
    /// This mirrors `ui::draw_tree`'s spans and has to keep mirroring them: the
    /// pane's left border, the gutter, the prefix, then the marker. `repos_right`
    /// is the tree's own `x` -- 0 in every layout that draws no sidebar, which is
    /// exactly what those layouts publish, so no separate field is needed.
    ///
    /// The zone is the whole `"{marker} "` span rather than the glyph alone. One
    /// cell is a poor thing to ask a pointer for, and the pad space is part of
    /// the same span in the render, so nothing else has a claim on it. It stops
    /// there deliberately: the branch character to its left is tree drawing, and
    /// widening onto it would start eating clicks meant to select.
    fn marker_zone(&self, prefix: &str) -> std::ops::RangeInclusive<u16> {
        // Character count is a cell count here: every glyph a prefix is built
        // from -- `│`, `├`, `└`, space -- is one cell wide, and so is every
        // tier's marker, which `icons` pins with a test.
        let x = self.repos_right + 1 + Self::TREE_GUTTER + prefix.chars().count() as u16;
        x..=x + 1
    }

    /// A left-click in the task tree: selects the row, and *also* opens or
    /// closes it when the click landed on that row's expand/collapse marker.
    ///
    /// It selects either way. Toggling without moving the selection is the other
    /// tenable design -- it is what file explorers do -- but here it would let
    /// you collapse a node the selection is *inside*, hiding the cursor and
    /// leaving the detail pane describing a task no visible row points at.
    /// Selecting the row you clicked keeps the selection on screen by
    /// construction, and matches the keyboard, where `-`/`+` and the arrows only
    /// ever act on the cursor.
    pub fn click_tree(&mut self, column: u16, row: u16) {
        self.select_at_row(row);

        let Some(index) = self.list_row_index(row, self.tree_offset) else {
            return;
        };

        // Scoped so the borrow of `tree`/`expanded` ends before the toggle.
        let hit = {
            let rows = tree::visible_rows(&self.tree, &self.expanded);
            match rows.get(index) {
                // A leaf has a marker drawn in that column too, but nothing to
                // open -- so a click there stays an ordinary select.
                Some(r) if r.has_children && self.marker_zone(&r.prefix).contains(&column) => {
                    Some(r.node.task.id.clone())
                }
                _ => None,
            }
        };

        // `remove` reports whether it was there, so this is the toggle.
        if let Some(id) = hit
            && !self.expanded.remove(&id)
        {
            self.expanded.insert(id);
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
        self.repos_visible = cfg.repos_open;
        // The file's values are a *starting* layout, and a reload is the one
        // moment they are meant to replace what dragging has done -- otherwise
        // saving an edit to either would appear to do nothing.
        self.split_percent = cfg.split_percent;
        self.repos_width = cfg.repos_width;
        self.rebuild();
    }

    /// The panes `Tab` walks, left to right as they are drawn.
    ///
    /// The sidebar joins the cycle exactly when it is shown -- the same
    /// predicate that decides whether it is drawn as a third pane, so the key
    /// can never land on a pane that is not there, and the two cannot drift.
    fn focus_cycle(&self) -> &'static [Focus] {
        if self.repos_shown() {
            &[Focus::Repos, Focus::Tree, Focus::Detail]
        } else {
            &[Focus::Tree, Focus::Detail]
        }
    }

    /// Moves focus one pane along the cycle; `Tab` forward, `Shift-Tab` back.
    ///
    /// This used to alternate the tree and the detail only, on the grounds
    /// that `Tab`'s contract was "the other of two panes" and a third
    /// destination would make it ambiguous which one it returned to. That was
    /// true when the sidebar was a place you visited with `3` and left again;
    /// it stopped being true once the sidebar drove the other two panes and
    /// earned a number of its own. An ordered cycle answers the ambiguity the
    /// old reasoning worried about -- with a direction, "back" is never in
    /// doubt.
    pub fn cycle_focus(&mut self, forward: bool) {
        let cycle = self.focus_cycle();
        let Some(i) = cycle.iter().position(|f| *f == self.focus) else {
            // Focused on a pane no longer in the cycle -- the sidebar, hidden
            // from under you. Land on the first rather than computing an
            // offset from a position that does not exist.
            self.focus = cycle[0];
            return;
        };
        let n = cycle.len();
        self.focus = cycle[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }];
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
    ///
    /// Also true whenever the repo pane is focused at a width in the
    /// `Panes::Two` gap -- wide enough to split, not wide enough for a third
    /// pane. That width reserves no room for the sidebar at all, so without
    /// this the repo pane would simply not be drawn: reachable by resizing
    /// alone with no keypress in between (hold `Focus::Repos` at `Three`,
    /// then narrow past `repos_pane_above`), which a key-handler-only fix
    /// cannot close because no key is pressed. Framing it as "one pane,
    /// chosen by focus" rather than a separate flag also means nothing needs
    /// to be undone when focus or width changes back -- unlike pinning
    /// `zoom`, which stays forced long after the terminal that required it is
    /// gone, `zoom` itself still outranks this (checked first, same as the
    /// width rule), so `z` remains the escape hatch out of it.
    pub fn single_pane(&self) -> bool {
        if let Some(z) = self.zoom {
            return z;
        }
        if self.single_pane_below > 0 && self.terminal_width < self.single_pane_below {
            return true;
        }
        // Focused on a pane the layout has no slot for. Previously this asked
        // specifically about the sidebar; asking whether the *focused* pane is
        // drawn at all is the same rule stated generally, and it now also
        // covers the detail pane being the one displaced.
        !self.laid_out().contains(&self.focus)
    }

    /// Whether the sidebar is shown at all: `repos_open` at startup, `b`/`1`
    /// afterward. Width has no say here any more -- see `repos_visible`'s doc
    /// -- only in `room_for_three`, a separate question about a sidebar that
    /// is *already* shown: does it get to be a third pane, or does the detail
    /// yield to it. Conflating the two used to mean showing the sidebar at a
    /// width that fits two panes added a third anyway, cramming three into
    /// room already decided was enough for two.
    fn repos_shown(&self) -> bool {
        self.repos_visible
    }

    /// Whether the width reserves room for three panes side by side, once the
    /// sidebar is already shown by `repos_shown`.
    fn room_for_three(&self) -> bool {
        self.repos_pane_above > 0 && self.terminal_width >= self.repos_pane_above
    }

    /// The panes the width would lay out, left to right, before zoom or a
    /// focus that none of them holds is taken into account.
    fn laid_out(&self) -> Vec<Focus> {
        if !self.repos_shown() {
            return vec![Focus::Tree, Focus::Detail];
        }
        if self.room_for_three() {
            return vec![Focus::Repos, Focus::Tree, Focus::Detail];
        }
        // Room for two, and the sidebar is one of them. **The detail yields**,
        // not the tree: the sidebar's whole job is choosing which store the
        // *tree* shows, so those two side by side is the pairing that makes
        // asking for the sidebar worth anything. The detail is a keypress away
        // and the pane most often being read rather than acted on.
        vec![Focus::Repos, Focus::Tree]
    }

    /// The panes actually drawn, left to right.
    pub fn drawn_panes(&self) -> Vec<Focus> {
        if self.single_pane() {
            return vec![self.focus];
        }
        self.laid_out()
    }

    /// See [`Panes`]. How *many* panes are drawn -- the shape of the layout.
    pub fn panes(&self) -> Panes {
        match self.drawn_panes().len() {
            1 => Panes::One,
            2 => Panes::Two,
            _ => Panes::Three,
        }
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

    /// Shows or hides the repo sidebar, whatever it started as.
    ///
    /// Hiding the pane you are standing in has to move you somewhere, or the
    /// movement keys would drive a pane that is not on screen -- and `Tab`
    /// deliberately never lands on the sidebar, so the tree is the only place
    /// to go back to.
    pub fn toggle_repos(&mut self) {
        let showing = self.repos_shown();
        self.repos_visible = !showing;
        if showing && self.focus == Focus::Repos {
            self.focus = Focus::Tree;
        }
    }

    /// Moves to the repo sidebar, revealing it if it was hidden.
    ///
    /// A key that reaches a pane has to be able to bring it back, or `b` would
    /// be a way to lose the sidebar with `1` silently refusing to return it.
    pub fn show_repos(&mut self) {
        self.repos_visible = true;
        self.focus = Focus::Repos;
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
    ///
    /// The sidebar is tested first and only when it is actually drawn
    /// (`repos_right > 0`, which `Panes::Three` alone sets). Without that arm
    /// its columns all answered `Focus::Tree`, since they sit left of
    /// `divider_x` -- so clicking a repo row moved the *task* selection and
    /// the wheel over the sidebar scrolled the tree, both of which are the
    /// selection-disturbing behaviour this app exists to avoid.
    pub fn pane_at(&self, column: u16) -> Focus {
        if self.single_pane() {
            return self.focus;
        }
        if self.repos_right > 0 && column < self.repos_right {
            return Focus::Repos;
        }
        // `divider_x == 0` means no tree/detail boundary was drawn -- the
        // sidebar-plus-tree layout. Testing it explicitly rather than letting
        // `column < 0` fall through is what stops every click there being
        // answered as the detail pane, which is not on screen at all.
        if self.divider_x > 0 && column >= self.divider_x {
            return Focus::Detail;
        }
        Focus::Tree
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

    /// How far the help can scroll before its last line is on screen. Zero
    /// means the whole dialog fits, which is what the hint row and the overflow
    /// markers both key off.
    pub fn help_max_scroll(&self) -> u16 {
        self.help_content_height
            .saturating_sub(self.help_viewport_height)
    }

    /// Clamped like the detail pane's, and against a height only the renderer
    /// can know -- so before the first frame this is a no-op, which is correct:
    /// you cannot scroll a dialog you have not been shown.
    pub fn scroll_help(&mut self, dy: i32) {
        let max = self.help_max_scroll() as i32;
        self.help_scroll = (self.help_scroll as i32).saturating_add(dy).clamp(0, max) as u16;
    }

    /// Always from the top: `?` is asked by someone looking for a key, and
    /// resuming where the last reading stopped hides the first ten of them.
    pub fn open_help(&mut self) {
        self.help_scroll = 0;
        self.mode = Mode::Help;
    }

    /// Selecting a different task must not leave you halfway down the old one.
    ///
    /// The single choke point every selection change goes through, which is
    /// what lets `needs_tree_reveal` cover all of them -- a keypress, a click,
    /// a worktree switch's remembered cursor -- for free, by being set exactly
    /// where the thing it needs to know about (`self.selected` actually
    /// changing) is already being decided.
    fn select(&mut self, id: Option<String>) {
        if id != self.selected {
            self.detail_scroll = (0, 0);
            self.needs_tree_reveal = true;
        }
        self.selected = id;
    }

    pub fn is_modal(&self) -> bool {
        !matches!(self.mode, Mode::Normal | Mode::Search)
    }

    /// Switches which store the task panes read, remembering where the cursor
    /// was in the worktree being left.
    pub fn select_worktree(&mut self, path: &str) {
        if self.selected_worktree.as_deref() == Some(path) {
            return;
        }
        if let (Some(old), Some(sel)) = (self.selected_worktree.clone(), self.selected.clone()) {
            self.task_memory.insert(old, sel);
        }
        self.selected_worktree = Some(path.to_string());
        // Through `select`, not a bare field write: the task now selected
        // belongs to a different store, so any scroll position left over from
        // the previous one has to go the same way it does whenever the
        // selection changes for any other reason.
        let remembered = self.task_memory.get(path).cloned();
        self.select(remembered);
    }

    /// Loads a different store into a running app.
    ///
    /// Deliberately **not** `apply_tasks`: that method's whole job is
    /// preserving a selection and an expansion set the *same* store made, by
    /// resolving `self.selected`/`self.expanded` against the new task list.
    /// Across a store switch those ids belong to an entirely different store,
    /// so comparing them is meaningless -- and since real dex ids are short
    /// slugs, `next_ids.contains(&sel)` succeeding by coincidence is exactly
    /// the kind of hard-to-notice bug that deserves its own code path rather
    /// than a repurposed one.
    ///
    /// Follows `App::new`'s first-load rule instead: everything here is new,
    /// so expand it -- CLAUDE.md records the collapsed-single-root version of
    /// this as a bug that has already shipped once.
    /// Takes the store *directory* and derives the label from it, rather than
    /// taking the label: `store_dir` is what a refresh is tagged with (see
    /// `Msg::Tasks`) and `store_label` is what the header shows, and the one
    /// thing that must never happen is those two describing different stores.
    /// One argument, set in one place, cannot.
    pub fn load_store(&mut self, tasks: Vec<Task>, store_dir: String) {
        self.by_id = index(&tasks);
        self.tasks = tasks;
        self.store_label = crate::dex::store_label(&store_dir);
        self.store_dir = store_dir;
        self.progress = tree::subtree_progress(&self.tasks);
        self.expand_all();
        self.rebuild();
        // Deliberately NOT `self.selected = self.first_visible_id()` here.
        // The real call sequence a store switch drives is `select_worktree`
        // (which restores a remembered task id from `task_memory` for the
        // worktree being entered) followed immediately by this method, so an
        // unconditional reset here would silently make Task 6's per-worktree
        // cursor memory dead code every time -- it would never survive past
        // the `load_store` that always follows it in practice. `rebuild`
        // already keeps `self.selected` exactly when it both exists in the
        // new store and is visible under the current filter, and replaces it
        // with the first visible id otherwise -- the same rule every other
        // selection change in this app follows, so there is nothing left to
        // repeat here.
        self.tree_offset = 0;
        self.detail_scroll = (0, 0);
    }

    /// Rebuilt from `self.repos` on every call, exactly as the task tree is
    /// rebuilt every frame -- a cached `Vec<Row>` would go stale the moment the
    /// repo list changed underneath it, since `Row` carries bare indices.
    pub fn repo_rows(&self) -> Vec<crate::repos::Row> {
        crate::repos::rows(&self.repos, self.here_repo())
    }

    /// The repo that goes under `here`: the one dextui was **launched in**.
    ///
    /// Deliberately not "the store being read". Keying it off the current
    /// store meant switching into a saved repo moved `here` onto it and the
    /// repo you actually came from vanished -- abrupt, and wrong about what
    /// the word means. Where you are in the filesystem does not change because
    /// you looked at another project's tasks, so this is fixed for the run.
    ///
    /// Shown only when there is a store to show. Launched somewhere with no
    /// `.dex`, `here` would be a heading over a repo with nothing in it.
    /// Checked live rather than at startup, so creating the first task makes
    /// the section appear without a relaunch.
    fn here_repo(&self) -> Option<usize> {
        let path = self.here_path.as_deref()?;
        if !std::path::Path::new(&self.here_store).is_dir() {
            return None;
        }
        self.repos.iter().position(|r| r.path == path)
    }

    /// The nearest row the cursor may rest on, searching in the direction of
    /// travel and then back the other way.
    ///
    /// Headings are labels, so the cursor has to pass over them rather than
    /// land on them -- and `j` at the bottom of a section must not stick.
    /// Searching outward in both directions also means a cursor left on a
    /// heading by a list that changed under it recovers instead of freezing.
    fn nearest_selectable(&self, from: usize, forward: bool) -> Option<usize> {
        let rows = self.repo_rows();
        if rows.is_empty() {
            return None;
        }
        let n = rows.len();
        let ahead: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new(from..n)
        } else {
            Box::new((0..=from.min(n - 1)).rev())
        };
        for i in ahead {
            if rows[i].selectable() {
                return Some(i);
            }
        }
        // Nothing that way -- a heading at the very end. Turn round.
        let back: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new((0..from.min(n)).rev())
        } else {
            Box::new(from.min(n - 1)..n)
        };
        back.into_iter().find(|i| rows[*i].selectable())
    }

    /// Moves the sidebar cursor, clamped the same way `move_selection` clamps
    /// the tree's -- a no-op on an empty list rather than a panic.
    pub fn move_repo_row(&mut self, delta: isize) {
        let len = self.repo_rows().len();
        if len == 0 {
            return;
        }
        let target = (self.selected_repo_row as isize + delta).clamp(0, len as isize - 1) as usize;
        if let Some(i) = self.nearest_selectable(target, delta >= 0) {
            self.selected_repo_row = i;
        }
    }

    /// Selects the sidebar row drawn on `row`, if any -- the mouse half of
    /// `move_repo_row`, and the exact counterpart of `select_at_row` in the
    /// tree, down to the `+1` that skips the pane's top border and the use of
    /// the offset the renderer last published.
    ///
    /// Selecting only. Switching store stays on `enter`/`l`, so a stray click
    /// in the sidebar cannot cost a ~180ms dex call and replace both other
    /// panes.
    pub fn select_repo_at_row(&mut self, row: u16) {
        let Some(index) = self.list_row_index(row, self.repos_offset) else {
            return;
        };
        // Past the last row is dead space: it must not move the cursor to the
        // end, which is what clamping would do. A heading is dead space too --
        // clicking a label should no more move the cursor than clicking below
        // the list does.
        if self.repo_rows().get(index).is_some_and(|r| r.selectable()) {
            self.selected_repo_row = index;
        }
    }

    /// A wheel over the sidebar. Slides the content and holds the cursor on
    /// its screen row, exactly as `scroll_tree` does and for the same reason --
    /// two panes an inch apart must not answer one gesture in two directions.
    pub fn scroll_repos(&mut self, delta: isize) {
        let len = self.repo_rows().len();
        if len == 0 {
            return;
        }
        let last = len as isize - 1;
        self.repos_offset = (self.repos_offset as isize + delta).clamp(0, last) as usize;
        self.move_repo_row(delta);
    }

    pub fn select_first_repo_row(&mut self) {
        if let Some(i) = self.nearest_selectable(0, true) {
            self.selected_repo_row = i;
        }
    }

    pub fn select_last_repo_row(&mut self) {
        let last = self.repo_rows().len().saturating_sub(1);
        if let Some(i) = self.nearest_selectable(last, false) {
            self.selected_repo_row = i;
        }
    }

    /// The repo owning the row under the sidebar cursor -- a worktree row
    /// resolves to its parent, since `D` unregisters the whole entry, not one
    /// worktree inside it.
    pub fn selected_repo(&self) -> Option<&crate::repos::Repo> {
        let index = match self.repo_rows().get(self.selected_repo_row)? {
            crate::repos::Row::Repo { index } => *index,
            crate::repos::Row::Worktree { repo, .. } => *repo,
            crate::repos::Row::Heading(_) | crate::repos::Row::Hint(_) => return None,
        };
        self.repos.get(index)
    }

    /// The exact worktree path under the sidebar cursor -- a repo row
    /// resolves to its own (main) worktree, which `git worktree list` always
    /// reports first and which shares the repo's own registered path.
    pub fn selected_worktree_path(&self) -> Option<String> {
        match self.repo_rows().get(self.selected_repo_row)? {
            crate::repos::Row::Heading(_) | crate::repos::Row::Hint(_) => None,
            crate::repos::Row::Repo { index } => self.repos.get(*index).map(|r| r.path.clone()),
            crate::repos::Row::Worktree { repo, index } => self
                .repos
                .get(*repo)
                .and_then(|r| r.worktrees.get(*index))
                .map(|w| w.path.clone()),
        }
    }

    /// The dex store behind a sidebar path.
    ///
    /// Not always `<path>/.dex`: the global row's path *is* its store, since
    /// dex's out-of-repo fallback is a bare directory rather than a checkout
    /// with a `.dex` in it. Everything that turns a sidebar selection into a
    /// store goes through here so that exception lives in one place --
    /// getting it wrong is silent, per the `--storage-path` rule.
    pub fn store_for_path(&self, path: &str) -> String {
        for r in &self.repos {
            let wt = r.worktrees.iter().find(|w| w.path == path);
            if wt.is_some() || r.path == path {
                return r.store(wt);
            }
        }
        crate::repos::store_dir(path)
    }

    /// Every store the sidebar can reach, deduplicated.
    ///
    /// One definition rather than two: startup builds its watcher fleet and
    /// its cache from this, and so does anything that adds a repo mid-run, so
    /// the two cannot disagree about what "every store" means.
    pub fn sidebar_stores(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in &self.repos {
            let stores = r
                .worktrees
                .iter()
                .map(|w| r.store(Some(w)))
                .chain(r.worktrees.is_empty().then(|| r.store(None)))
                .collect::<Vec<_>>();
            for dir in stores {
                if !out.contains(&dir) {
                    out.push(dir);
                }
            }
        }
        out
    }

    /// Puts the sidebar cursor on whichever row is the store being read, so
    /// the pane opens pointing at what the other two panes are showing.
    pub fn select_current_store_row(&mut self) {
        let rows = self.repo_rows();
        let found = rows.iter().position(|row| match row {
            crate::repos::Row::Heading(_) | crate::repos::Row::Hint(_) => false,
            crate::repos::Row::Repo { index } => self.repos[*index].store(None) == self.store_dir,
            crate::repos::Row::Worktree { repo, index } => {
                let r = &self.repos[*repo];
                r.store(Some(&r.worktrees[*index])) == self.store_dir
            }
        });
        if let Some(i) = found {
            self.selected_repo_row = i;
        }
    }

    /// A cached store's counts, for the sidebar. `None` means it has not been
    /// read yet -- a repo registered mid-run, or one whose read failed -- which
    /// is a different thing from a store with no tasks and must stay tellable
    /// apart.
    pub fn counts_for_store(&self, store_dir: &str) -> Option<Counts> {
        self.store_tasks.get(store_dir).map(|t| counts_for(t))
    }

    /// Registers a repo. Returns whether the registry changed, so a duplicate
    /// can be reported rather than looking inert.
    ///
    /// Goes through `Registry::add_and_save` rather than mutating
    /// `self.registry` directly: that re-reads the file fresh before writing,
    /// so this process's own possibly-stale in-memory copy cannot clobber a
    /// registration another dextui instance made in the meantime, and it
    /// refuses outright rather than saving anything when the file cannot be
    /// read back honestly.
    pub fn register_repo_path(&mut self, repo_path: &str) -> Result<bool, String> {
        self.registry.add_and_save(repo_path)
    }

    /// Unregistering is a view operation: it never touches the worktree, the
    /// branch or the store, only the entry and the row it drew.
    ///
    /// Returns `Err` -- rather than swallowing the failure -- when the
    /// removal could not actually be persisted, and leaves `self.repos`
    /// untouched in that case. Applying the in-memory removal on a save that
    /// failed would make the row disappear for this session only to reappear
    /// at the next launch, with nothing on screen to explain why.
    pub fn unregister_repo_path(&mut self, repo_path: &str) -> Result<bool, String> {
        let changed = self.registry.remove_and_save(repo_path)?;
        if changed {
            // No special case for "the repo you are reading" any more.
            // Unsaving the launch repo moves it back up into `here` -- the
            // exact reverse of what `a` does -- rather than taking its row
            // away, so the old guard and the row-that-is-neither state it
            // produced are both unnecessary. Any *other* repo does lose its
            // row, which is what the `retain` below is for.
            if let Some(row) = self.repos.iter_mut().find(|r| r.path == repo_path) {
                row.registered = false;
            }
            self.repos
                .retain(|r| r.registered || Some(&r.path) == self.here_path.as_ref());
            self.selected_repo_row = self
                .selected_repo_row
                .min(self.repo_rows().len().saturating_sub(1));
        }
        Ok(changed)
    }
}

fn index(tasks: &[Task]) -> HashMap<String, Task> {
    tasks.iter().map(|t| (t.id.clone(), t.clone())).collect()
}

/// The shared arithmetic behind `App::counts()` and `counts_for()` -- see
/// `App::counts()` for the precedence rules (mirrors `dex list --ready` /
/// `--blocked`, not `dex status`) and why `ready + blocked` does not sum to
/// `pending`.
fn counts_from(tasks: &[Task], by_id: &HashMap<String, Task>) -> Counts {
    let mut c = Counts {
        total: tasks.len(),
        ..Default::default()
    };

    for t in tasks {
        if t.completed {
            c.completed += 1;
            continue;
        }
        c.pending += 1;

        // Same precedence as status.js: started wins over everything else.
        if t.is_in_progress() {
            c.active += 1;
        } else if crate::dex::is_blocked(t, by_id) {
            c.blocked += 1;
        } else if !has_incomplete_children(t, by_id) {
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
fn has_incomplete_children(t: &Task, by_id: &HashMap<String, Task>) -> bool {
    t.children
        .iter()
        .filter_map(|id| by_id.get(id))
        .any(|c| !c.completed)
}

/// Counts for a task list that belongs to no running `App` -- another
/// registered worktree's store, read only to keep the sidebar's numbers
/// current. Builds its own index rather than reusing `App::by_id`, since the
/// tasks belong to an entirely different store; see `App::counts()` for the
/// version that avoids paying that cost every frame for the selected store.
pub fn counts_for(tasks: &[Task]) -> Counts {
    counts_from(tasks, &index(tasks))
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
        assert_eq!(
            app.selected.as_deref(),
            Some("b"),
            "reordering is not moving"
        );
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
        assert!(
            !small.single_pane(),
            "z must be able to force the split back"
        );
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
        assert!(
            !narrow(80).single_pane(),
            "the threshold itself still splits"
        );
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
        app.repos_visible = true; // exercise all three rungs, not just two

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
        app.repos_visible = true; // shown throughout; only room_for_three moves

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
        // Shown throughout, so `repos_pane_above = 0` is what is under test:
        // shown but never promoted to a third pane, not simply hidden.
        app.repos_visible = true;

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

    fn ladder(width: u16) -> App {
        let mut app = counted(vec![task("a", None, &[])]);
        app.single_pane_below = 80;
        app.repos_pane_above = 110;
        app.terminal_width = width;
        app
    }

    /// The bug this closes: `Panes::Two` (the 80-110 gap by default)
    /// reserves no room for the sidebar at all, so before this, focusing it
    /// there left `j`/`k`/`G`/`enter` driving a cursor nothing on screen
    /// showed. Framed as "one pane, chosen by focus" rather than a forced
    /// `zoom`, so there is nothing to undo when focus or width changes back.
    ///
    /// Note what this does *not* cover, and why the distinction is real:
    /// focus arriving on the sidebar without anyone having asked for the
    /// sidebar -- a resize stranding it -- zooms it. Pressing `1` goes through
    /// `show_repos`, which records the request in `repos_visible`, and that
    /// gets you the sidebar beside the tree instead. Same width, different
    /// outcome, because "I want this pane" and "focus ended up here" are
    /// different things.
    #[test]
    fn repos_focus_becomes_a_single_pane_when_the_ladder_has_no_room_for_it() {
        let mut app = ladder(90);
        app.focus = Focus::Tree;
        assert_eq!(
            app.panes(),
            Panes::Two,
            "fixture should land squarely in the gap"
        );

        app.focus = Focus::Repos;

        assert_eq!(app.panes(), Panes::One, "must actually become visible");
        assert_eq!(app.zoom, None, "must not reach for zoom to get there");

        // Asked for rather than stranded: now it shares the width with the
        // tree instead of taking all of it.
        app.show_repos();
        assert_eq!(app.drawn_panes(), vec![Focus::Repos, Focus::Tree]);
    }

    /// No key is pressed here at all, only a resize. An *asked-for* sidebar
    /// (`repos_visible`, set once and not reconsidered by width the way it
    /// used to be) must not vanish when the room for three panes goes away --
    /// it steps down to sharing the width with the tree instead, the same
    /// place `showing_the_sidebar_where_only_two_fit_displaces_the_detail`
    /// reaches by a keypress. Losing the sidebar to a resize nobody asked for
    /// would be exactly the silent-disappearance bug this area exists to
    /// prevent, just arrived at from the wide side instead of the narrow one.
    #[test]
    fn narrowing_into_the_gap_while_already_repo_focused_keeps_it_visible() {
        let mut app = ladder(200);
        app.show_repos(); // asked for, not stranded -- see the test above
        assert_eq!(
            app.panes(),
            Panes::Three,
            "fixture should start with room to spare"
        );

        app.terminal_width = 90; // resize alone, no key event

        assert_eq!(
            app.panes(),
            Panes::Two,
            "must not vanish on a resize with no keypress"
        );
        assert_eq!(app.drawn_panes(), vec![Focus::Repos, Focus::Tree]);
    }

    /// Already has room: forcing a single pane here would take away the tree
    /// and detail panes for no reason.
    #[test]
    fn repos_focus_does_not_override_the_ladder_once_there_is_room() {
        let mut app = ladder(200);
        app.show_repos(); // asked for, not stranded -- see the test above
        assert_eq!(app.panes(), Panes::Three);
    }

    /// `zoom` still outranks everything, including the new focus-based rule
    /// -- pressing `z` remains the documented way to force the split away
    /// from a repo-focused gap, exactly as it already does for the width
    /// rule.
    #[test]
    fn z_still_forces_the_split_away_from_a_repos_focused_gap() {
        let mut app = ladder(90);
        app.focus = Focus::Repos;
        assert_eq!(app.panes(), Panes::One, "fixture should start single-pane");

        app.toggle_zoom();

        assert_eq!(app.zoom, Some(false));
        assert_eq!(
            app.panes(),
            Panes::Two,
            "z must be able to force the split back"
        );
    }

    /// The same monotonicity rule `the_pane_ladder_is_monotone` pins for the
    /// default focus must also hold with the repo pane focused throughout --
    /// nothing about a wider terminal should ever draw fewer panes. Distinct
    /// coverage from that test: `single_pane`'s *other* clause is in play here
    /// (focus on a pane the layout has no slot for), not just the width rungs.
    #[test]
    fn the_ladder_is_monotone_with_the_repo_pane_focused_too() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.single_pane_below = 80;
        app.repos_pane_above = 110;
        app.show_repos(); // asked for; sets focus to Focus::Repos too

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

    /// The bug this closes: with three panes the sidebar's own columns sit
    /// left of `divider_x`, so `pane_at` called every one of them
    /// `Focus::Tree` -- and the click handler then moved the *task* selection
    /// when you clicked a repo row, while the wheel over the sidebar scrolled
    /// the task tree.
    #[test]
    fn the_sidebar_owns_its_own_columns() {
        let mut app = ladder(200);
        app.divider_x = 90;
        app.repos_right = 26;

        for col in [0, 1, 25] {
            assert_eq!(app.pane_at(col), Focus::Repos, "column {col}");
        }
        for col in [26, 60, 89] {
            assert_eq!(app.pane_at(col), Focus::Tree, "column {col}");
        }
        for col in [90, 150, 199] {
            assert_eq!(app.pane_at(col), Focus::Detail, "column {col}");
        }
    }

    /// `repos_right` is only set where the sidebar is actually drawn, so the
    /// two-pane layout keeps answering exactly as it did -- a stale width from
    /// an earlier wide frame would be an invisible dead zone down the left of
    /// the tree.
    #[test]
    fn without_a_sidebar_pane_no_column_belongs_to_it() {
        let mut app = narrow(100);
        app.divider_x = 45;
        app.repos_right = 0;
        for col in [0, 1, 44] {
            assert_eq!(app.pane_at(col), Focus::Tree, "column {col}");
        }
    }

    /// The sidebar always carries the store being read, so the cursor opens on
    /// it rather than on whatever sorted first.
    #[test]
    fn the_sidebar_cursor_starts_on_the_store_being_read() {
        let mut app = app_with_repos();
        app.store_dir = "/x/two-feat/.dex".into();

        app.select_current_store_row();

        // By what it resolves to, not by index: the row's *position* moves
        // when the current repo is lifted into `here`, and an index would
        // pin the layout rather than the behaviour.
        assert_eq!(app.selected_worktree_path().as_deref(), Some("/x/two-feat"));
    }

    /// `store_for_path` is the single place that knows a row's store is not
    /// always `<path>/.dex` -- the global row's path *is* its store, and
    /// pointing dex at a directory that does not exist is silent.
    #[test]
    fn a_paths_store_is_resolved_through_the_row_that_owns_it() {
        let mut app = app_with_repos();
        assert_eq!(app.store_for_path("/x/one-feat"), "/x/one-feat/.dex");

        app.repos.push(crate::repos::Repo {
            name: "global".into(),
            path: "/cfg/dex/local".into(),
            worktrees: vec![],
            open: true,
            registered: false,
            is_global: true,
        });
        assert_eq!(app.store_for_path("/cfg/dex/local"), "/cfg/dex/local");

        // A path the sidebar has never heard of still gets the ordinary
        // derivation, which is what `selftest` and the older tests rely on.
        assert_eq!(app.store_for_path("/elsewhere"), "/elsewhere/.dex");
    }

    /// Unsaving the repo you launched in keeps its row: `here` renders it
    /// whether or not it is saved, so `D` only moves it out of `saved`. That
    /// is what made the old "keep the row you are reading" guard unnecessary.
    #[test]
    fn unsaving_the_here_repo_keeps_its_row() {
        with_isolated_registry("app-unsave-here", || {
            let mut app = app_with_repos();
            app.registry = crate::registry::Registry::default();
            app.register_repo_path("/x/one").unwrap();
            app.register_repo_path("/x/two").unwrap();

            assert!(app.unregister_repo_path("/x/two").unwrap());

            assert!(
                app.repos.iter().any(|r| r.path == "/x/two"),
                "the repo you are in was dropped"
            );
            assert!(
                app.repo_rows()
                    .contains(&crate::repos::Row::Repo { index: 1 })
            );
            assert_eq!(app.registry.repos, vec!["/x/one".to_string()]);
        });
    }

    /// A saved repo you are *not* in loses its row entirely, since neither
    /// section has anywhere to put it.
    #[test]
    fn unsaving_another_repo_drops_its_row() {
        with_isolated_registry("app-unsave-other", || {
            let mut app = app_with_repos();
            app.registry = crate::registry::Registry::default();
            app.register_repo_path("/x/one").unwrap();
            app.register_repo_path("/x/two").unwrap();

            assert!(app.unregister_repo_path("/x/one").unwrap());

            assert!(!app.repos.iter().any(|r| r.path == "/x/one"));
        });
    }

    /// The typical session is one repo read from its own directory, where the
    /// sidebar has nothing to add -- it must not appear just because the
    /// terminal happens to be wide, unless `repos_open` in the config says so.
    #[test]
    fn a_fresh_app_hides_the_sidebar_even_when_wide_enough_for_it() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.repos_pane_above = 110;
        app.terminal_width = 200;
        assert_eq!(app.panes(), Panes::Two, "the sidebar must start hidden");
    }

    /// `repos_open = true` is the config's way of starting with the sidebar
    /// shown -- as if `1` had already been pressed -- for anyone whose
    /// workflow wants it every launch rather than pressed for each session.
    #[test]
    fn repos_open_in_the_config_shows_the_sidebar_from_the_first_frame() {
        let cfg = Config {
            repos_open: true,
            ..Config::default()
        };
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), cfg);
        app.repos_pane_above = 110;
        app.terminal_width = 200;
        assert_eq!(app.panes(), Panes::Three, "repos_open = true must show it");
    }

    /// Reloading a config is the one moment file values are meant to replace
    /// what the runtime toggles have done -- otherwise flipping `repos_open`
    /// and pressing `,` would silently do nothing.
    #[test]
    fn reloading_config_applies_the_new_repos_open_value() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.repos_pane_above = 110;
        app.terminal_width = 200;
        assert_eq!(app.panes(), Panes::Two, "starts hidden");

        app.toggle_repos(); // simulate having pressed `b` mid-session
        assert_eq!(app.panes(), Panes::Three);

        let cfg = Config {
            repos_pane_above: 110,
            ..Config::default()
        };
        app.apply_config(cfg);
        assert_eq!(
            app.panes(),
            Panes::Two,
            "reload must restore the file's repos_open"
        );
    }

    /// `b` outranks the width rule, the way `z` does for zoom -- and toggles
    /// the *effective* state, so the first press always does the visible thing
    /// rather than appearing inert at a width that had already decided.
    #[test]
    fn b_shows_and_hides_the_sidebar_at_any_width() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.repos_pane_above = 110;
        app.single_pane_below = 0;
        app.terminal_width = 140;
        app.repos_visible = true; // starts shown
        assert_eq!(app.panes(), Panes::Three, "wide enough for the sidebar");

        app.toggle_repos();
        assert_eq!(app.panes(), Panes::Two, "b did not hide it");

        app.toggle_repos();
        assert_eq!(app.panes(), Panes::Three, "b did not bring it back");

        // And the other direction, from a width that had already hidden it.
        // Still two panes -- that is all this width fits -- but the sidebar is
        // now one of them, which is what asking for it has to mean.
        app.repos_visible = false;
        app.terminal_width = 90;
        assert_eq!(app.drawn_panes(), vec![Focus::Tree, Focus::Detail]);
        app.toggle_repos();
        assert_eq!(
            app.drawn_panes(),
            vec![Focus::Repos, Focus::Tree],
            "the first press must do something, without cramming in a third pane"
        );
    }

    /// The reported behaviour: at a width the app has already judged fits two
    /// panes, asking for the sidebar added a third rather than displacing one.
    #[test]
    fn showing_the_sidebar_where_only_two_fit_displaces_the_detail() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.repos_pane_above = 110;
        app.single_pane_below = 80;
        app.terminal_width = 100; // wide enough to split, not for three

        assert_eq!(app.drawn_panes(), vec![Focus::Tree, Focus::Detail]);

        app.show_repos();

        assert_eq!(app.drawn_panes(), vec![Focus::Repos, Focus::Tree]);
        assert_eq!(app.panes(), Panes::Two, "still only two panes wide");
    }

    /// The detail yields rather than the tree, and is still reachable: focusing
    /// a pane the layout has no slot for zooms it, which is the same rule that
    /// already covered the sidebar in this width band.
    #[test]
    fn the_displaced_detail_pane_is_still_reachable() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.repos_pane_above = 110;
        app.single_pane_below = 80;
        app.terminal_width = 100;
        app.show_repos();

        app.show_detail();

        assert_eq!(app.drawn_panes(), vec![Focus::Detail]);
        assert!(app.single_pane());
    }

    /// With room for three, asking for the sidebar still gets all three.
    #[test]
    fn a_wide_terminal_still_shows_every_pane() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.repos_pane_above = 110;
        app.terminal_width = 140;
        app.show_repos();
        assert_eq!(
            app.drawn_panes(),
            vec![Focus::Repos, Focus::Tree, Focus::Detail]
        );
    }

    /// Hiding the pane you are standing in has to move you somewhere, or the
    /// movement keys would drive a pane that is not drawn. `Tab` never lands
    /// on the sidebar, so the tree is the only place to go back to.
    #[test]
    fn hiding_the_sidebar_while_it_has_focus_returns_to_the_tree() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.repos_pane_above = 110;
        app.terminal_width = 140;
        app.repos_visible = true; // starts shown, so toggling is what hides it
        app.focus = Focus::Repos;

        app.toggle_repos();

        assert_eq!(app.focus, Focus::Tree);
        assert_eq!(app.panes(), Panes::Two);
    }

    /// `b` must not be a way to lose the sidebar for good: the key that
    /// reaches it has to bring it back.
    #[test]
    fn focusing_the_sidebar_reveals_it_when_b_had_hidden_it() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.repos_pane_above = 110;
        app.single_pane_below = 0;
        app.terminal_width = 140;
        app.repos_visible = true; // starts shown, so toggle_repos is the `b` that hides it
        app.toggle_repos();
        assert_eq!(app.panes(), Panes::Two);

        app.show_repos();

        assert_eq!(app.focus, Focus::Repos);
        assert_eq!(
            app.panes(),
            Panes::Three,
            "1 could not bring the sidebar back"
        );
    }

    /// The reported bug: switching into a saved repo made the repo you came
    /// from disappear, because `here` tracked the store being read. Where you
    /// are in the filesystem does not change because you looked at another
    /// project's tasks.
    #[test]
    fn here_stays_on_the_launch_repo_after_switching_stores() {
        let mut app = app_with_repos();
        let before = app.repo_rows();
        assert_eq!(before[0], crate::repos::Row::Heading("here"));
        assert_eq!(
            before[1],
            crate::repos::Row::Repo { index: 1 },
            "launched in two"
        );

        // Switch to the other repo's store, as `enter` on a saved row does.
        app.load_store(Vec::new(), "/x/one/.dex".into());

        let after = app.repo_rows();
        assert_eq!(after[0], crate::repos::Row::Heading("here"));
        assert_eq!(
            after[1],
            crate::repos::Row::Repo { index: 1 },
            "`here` followed the store instead of staying put: {after:?}"
        );
    }

    /// Launched somewhere with no `.dex`, `here` would be a heading over a
    /// repo with nothing in it. Checked live rather than at startup, so
    /// creating the first task makes the section appear without a relaunch.
    #[test]
    fn here_is_hidden_when_there_is_no_store_where_you_launched() {
        let mut app = app_with_repos();
        app.here_store = "/nonexistent-store-for-tests/.dex".into();

        let rows = app.repo_rows();

        assert!(
            !rows.contains(&crate::repos::Row::Heading("here")),
            "`here` shown with no store behind it: {rows:?}"
        );
        assert!(
            !rows.contains(&crate::repos::Row::Repo { index: 1 }),
            "unsaved and no store: it has no section at all: {rows:?}"
        );

        // The store appearing is enough to bring it back -- no relaunch.
        app.here_store = std::env::temp_dir().to_string_lossy().into_owned();
        assert!(
            app.repo_rows()
                .contains(&crate::repos::Row::Repo { index: 1 })
        );
    }

    /// Headings are labels, so the cursor passes over them rather than landing
    /// on one -- `j` at the end of a section must not stick.
    #[test]
    fn the_sidebar_cursor_steps_over_section_headings() {
        let mut app = app_with_repos();
        let rows = app.repo_rows();
        assert!(
            rows.iter().any(|r| !r.selectable()),
            "fixture should have headings: {rows:?}"
        );

        // Walk the whole list in both directions; never rest on a label.
        for _ in 0..rows.len() + 2 {
            app.move_repo_row(1);
            assert!(app.repo_rows()[app.selected_repo_row].selectable());
        }
        for _ in 0..rows.len() + 2 {
            app.move_repo_row(-1);
            assert!(app.repo_rows()[app.selected_repo_row].selectable());
        }

        app.select_first_repo_row();
        assert!(
            app.repo_rows()[app.selected_repo_row].selectable(),
            "g landed on a label"
        );
        app.select_last_repo_row();
        assert!(
            app.repo_rows()[app.selected_repo_row].selectable(),
            "G landed on a label"
        );
    }

    /// Clicking a label should no more move the cursor than clicking below the
    /// list does.
    #[test]
    fn clicking_a_section_heading_does_nothing() {
        let mut app = app_with_repos();
        let heading = app
            .repo_rows()
            .iter()
            .position(|r| !r.selectable())
            .expect("fixture should have a heading");

        let before = app.selected_repo_row;
        app.select_repo_at_row(app.body_top + 1 + heading as u16);

        assert_eq!(app.selected_repo_row, before);
    }

    /// A click in the sidebar selects the row under it, exactly as the tree
    /// does -- and, crucially, leaves the task selection alone.
    #[test]
    fn clicking_a_sidebar_row_selects_it_and_leaves_the_tasks_alone() {
        let mut app = app_with_repos();
        let task_before = app.selected.clone();

        app.select_repo_at_row(app.body_top + 1 + 3); // fourth row: Repo(two)

        assert_eq!(app.selected_repo_row, 3);
        assert_eq!(
            app.selected, task_before,
            "a sidebar click moved the task selection"
        );
    }

    /// Dead space below the last row must do nothing -- not jump the cursor to
    /// the end, which is what clamping would do. The same rule
    /// `clicking_empty_header_space_changes_nothing` pins for the header.
    #[test]
    fn clicking_below_the_last_sidebar_row_changes_nothing() {
        let mut app = app_with_repos();
        app.selected_repo_row = 1;

        app.select_repo_at_row(app.body_top + 1 + 50);

        assert_eq!(app.selected_repo_row, 1);
    }

    /// Through the offset the renderer last published, so a click lands on the
    /// row actually drawn rather than the one that would be there unscrolled.
    #[test]
    fn a_scrolled_sidebar_click_addresses_the_row_actually_drawn() {
        let mut app = app_with_repos();
        app.repos_offset = 2;

        app.select_repo_at_row(app.body_top + 1);

        assert_eq!(app.selected_repo_row, 2);
    }

    /// The wheel slides the sidebar's content and the cursor keeps its screen
    /// row -- the same gesture `scroll_tree` gives the tree, so two panes an
    /// inch apart cannot answer one drag in two directions.
    #[test]
    fn the_sidebar_wheel_moves_the_content_and_the_cursor_together() {
        let mut app = app_with_repos();

        let start = app.selected_repo_row;

        app.scroll_repos(2);
        assert_eq!(app.repos_offset, 2);
        assert!(
            app.selected_repo_row > start,
            "the cursor should travel with the content"
        );
        assert!(app.repo_rows()[app.selected_repo_row].selectable());

        app.scroll_repos(-2);
        assert_eq!(app.repos_offset, 0);
        assert!(app.repo_rows()[app.selected_repo_row].selectable());
    }

    /// An empty sidebar has nothing to slide, and must not underflow trying.
    #[test]
    fn scrolling_an_empty_sidebar_does_nothing() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.scroll_repos(3);
        assert_eq!(app.repos_offset, 0);
        assert_eq!(app.selected_repo_row, 0);
    }

    /// There is no divider to grab when only one pane is drawn, and a stale one
    /// would be an invisible drag target in the middle of the screen.
    #[test]
    fn there_is_nothing_to_drag_in_one_pane_mode() {
        let mut app = narrow(60);
        app.divider_x = 0;
        for col in [0, 1, 30, 59] {
            assert!(
                app.divider_at(col).is_none(),
                "column {col} looked like a divider"
            );
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
        let mut app = clickable(vec![(4, 11, HeaderZone::SortCycle)]);
        let order = app.sort;

        assert!(app.click_header(5, false));
        assert_eq!(app.sort, order.next());
        assert!(!app.sort_reversed, "cycling must not also reverse");

        let after_cycle = app.sort;
        assert!(app.click_header(5, true));
        assert!(app.sort_reversed);
        assert_eq!(app.sort, after_cycle, "reversing must not also cycle");
    }

    #[test]
    fn clicking_a_sort_word_selects_that_sort() {
        let mut app = clickable(vec![(4, 10, HeaderZone::Sort(Sort::Updated))]);
        app.sort = Sort::Priority;

        assert!(app.click_header(6, false));
        assert_eq!(app.sort, Sort::Updated);
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
        let mut app = clickable(vec![(40, 47, HeaderZone::SortCycle)]);
        let before = (
            app.filter,
            app.sort,
            app.sort_reversed,
            app.selected.clone(),
        );

        assert!(!app.click_header(3, false));
        assert!(!app.click_header(39, false));
        assert!(!app.click_header(48, false));

        assert_eq!(
            (
                app.filter,
                app.sort,
                app.sort_reversed,
                app.selected.clone()
            ),
            before
        );
    }

    #[test]
    fn a_zone_covers_its_last_column() {
        let app = clickable(vec![(10, 12, HeaderZone::SortCycle)]);
        assert_eq!(app.header_zone_at(9), None);
        assert_eq!(app.header_zone_at(10), Some(HeaderZone::SortCycle));
        assert_eq!(app.header_zone_at(12), Some(HeaderZone::SortCycle));
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
        assert_eq!(
            app.counts().ready,
            1,
            "nothing is holding the parent up now"
        );
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

    /// The reported case. A completed task's id, pasted while the filter is
    /// `pending`, matched nothing visible and the screen said only that the
    /// filter was hiding something -- so searching for a task you had just
    /// copied the id of looked like the search was broken.
    #[test]
    fn a_query_hidden_by_the_filter_says_so_and_counts_it() {
        let mut done = task("b4d5gfpl", None, &[]);
        done.name = "Ship the release workflow".into();
        done.completed = true;
        let mut app = counted(vec![done, task("other", None, &[])]);

        app.filter = Filter::Pending;
        app.query.value = "b4d5gfpl".into();
        app.rebuild();

        let msg = app
            .empty_reason()
            .expect("the tree is empty, so there is a reason");
        assert!(
            msg.contains("1 task,"),
            "it should count the matches: {msg}"
        );
        assert!(msg.contains("pending"), "it should name the filter: {msg}");
        assert!(
            msg.contains("Press f"),
            "it should say which key fixes it: {msg}"
        );
        // Not "press f to show it": from `pending` one press lands on
        // `active`, which hides a completed task just as firmly.
        assert!(
            msg.contains("until the filter reads all"),
            "only `all` is guaranteed to show it, so that is what it must name: {msg}"
        );
    }

    /// The other half of the same question, and the reason it is worth
    /// distinguishing: a query nothing answers must not claim the filter is
    /// hiding anything, or the advice it gives is a wild goose chase.
    #[test]
    fn a_query_nothing_answers_does_not_blame_the_filter() {
        let mut app = counted(vec![task("a", None, &[])]);

        app.filter = Filter::Pending;
        app.query.value = "zzzznope".into();
        app.rebuild();

        let msg = app.empty_reason().expect("the tree is empty");
        assert!(msg.contains("Nothing matches"), "{msg}");
        assert!(
            !msg.contains("filter"),
            "there is nothing for the filter to hide: {msg}"
        );
    }

    /// Plural, because "matches 3 tasks, press f to show it" reads as a bug in
    /// the message rather than as a message about a bug.
    #[test]
    fn the_hidden_count_is_pluralised() {
        let done = |id: &str| {
            let mut t = task(id, None, &[]);
            t.completed = true;
            t
        };
        let mut app = counted(vec![done("zz1"), done("zz2"), done("zz3")]);

        app.filter = Filter::Pending;
        app.query.value = "zz".into();
        app.rebuild();

        let msg = app.empty_reason().expect("the tree is empty");
        assert!(msg.contains("3 tasks,"), "{msg}");
    }

    /// A tree with rows in it has nothing to explain, and a message drawn over
    /// one would be covering the thing it describes.
    #[test]
    fn a_tree_with_rows_has_no_empty_reason() {
        let app = counted(vec![task("a", None, &[])]);
        assert!(app.empty_reason().is_none());
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

    /// A task that shows up already having children was created together with
    /// them -- an agent or a script that finished before dextui's next
    /// refresh -- and collapsing it would hide the exact subtasks that make it
    /// worth looking at. This used to collapse, on the reasoning that an agent
    /// creating subtasks must not explode the tree under the cursor; but the
    /// cursor is never *on* a task that did not exist a moment ago, so nothing
    /// under it moves. A pre-existing parent gaining a new child is the case
    /// that reasoning actually protects, and `expansion_is_dropped_only_for_
    /// tasks_that_disappeared` below still pins that it is left alone.
    #[test]
    fn a_new_parent_arrives_expanded() {
        let mut app = app_with(vec![task("a", None, &[])], "a");

        app.apply_tasks(vec![
            task("a", None, &[]),
            task("newparent", None, &["kid"]),
            task("kid", Some("newparent"), &[]),
        ]);

        assert!(app.expanded.contains("newparent"));
    }

    /// A brand-new leaf has no children to reveal, so it must not spuriously
    /// join `expanded` -- there would be nothing wrong with it doing so, but a
    /// set that only ever grows for tasks that can actually use it is the
    /// simpler invariant to keep believing.
    #[test]
    fn a_new_leaf_task_does_not_join_expanded() {
        let mut app = app_with(vec![task("a", None, &[])], "a");

        app.apply_tasks(vec![task("a", None, &[]), task("newleaf", None, &[])]);

        assert!(!app.expanded.contains("newleaf"));
    }

    #[test]
    fn expansion_is_dropped_only_for_tasks_that_disappeared() {
        let mut app = app_with(
            vec![
                task("a", None, &["k"]),
                task("k", Some("a"), &[]),
                task("gone", None, &[]),
            ],
            "a",
        );
        app.expanded.insert("gone".to_string());

        app.apply_tasks(vec![task("a", None, &["k"]), task("k", Some("a"), &[])]);

        assert!(app.expanded.contains("a"));
        assert!(!app.expanded.contains("gone"));
    }

    /// The auto-expand above must not resurrect a parent the user deliberately
    /// collapsed -- it only applies to a task that is *new*, and a parent that
    /// merely gains one more child among ones it already had is not new.
    #[test]
    fn an_existing_parent_gaining_a_child_keeps_its_own_expand_state() {
        let mut app = app_with(
            vec![task("a", None, &["k1"]), task("k1", Some("a"), &[])],
            "a",
        );
        app.expanded.remove("a"); // deliberately collapsed by the user

        app.apply_tasks(vec![
            task("a", None, &["k1", "k2"]),
            task("k1", Some("a"), &[]),
            task("k2", Some("a"), &[]),
        ]);

        assert!(
            !app.expanded.contains("a"),
            "an existing parent must not be re-expanded"
        );
    }

    /// A leaf that becomes a parent is not the same case as a parent gaining
    /// another child, and the difference is whether there is any user intent to
    /// preserve.
    ///
    /// You cannot collapse a leaf -- it has no twisty, and `expanded` never
    /// held it -- so leaving it collapsed the moment it sprouts children
    /// preserves nothing. It just hides the subtasks that are the entire reason
    /// the task changed, behind a twisty that was not there a second ago.
    ///
    /// This is the case `apply_tasks` used to miss: its test was
    /// `!by_id.contains_key(id)`, which asks "is this task new", and a leaf
    /// becoming a parent is not new.
    #[test]
    fn a_leaf_that_becomes_a_parent_arrives_expanded() {
        let mut app = app_with(vec![task("a", None, &[]), task("b", None, &[])], "b");
        assert!(
            !app.expanded.contains("a"),
            "a leaf is not in `expanded` to begin with"
        );

        // An agent adds subtasks to a task that had none.
        app.apply_tasks(vec![
            task("a", None, &["k1", "k2"]),
            task("k1", Some("a"), &[]),
            task("k2", Some("a"), &[]),
            task("b", None, &[]),
        ]);

        assert!(
            app.expanded.contains("a"),
            "a task that just gained its first children should show them"
        );
    }

    /// The other half, and the reason the rule is about the *first* children
    /// rather than about having any: an explicit collapse is real intent and a
    /// refresh must not overrule it. Distinct from the test above, where there
    /// was no intent to overrule.
    #[test]
    fn a_collapsed_parent_gaining_more_children_stays_collapsed() {
        let mut app = app_with(
            vec![
                task("a", None, &["k1"]),
                task("k1", Some("a"), &[]),
                task("b", None, &[]),
            ],
            "b",
        );
        app.expanded.remove("a"); // the user collapsed it on purpose

        app.apply_tasks(vec![
            task("a", None, &["k1", "k2"]),
            task("k1", Some("a"), &[]),
            task("k2", Some("a"), &[]),
            task("b", None, &[]),
        ]);

        assert!(
            !app.expanded.contains("a"),
            "gaining a further child must not undo a deliberate collapse"
        );
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
            assert!(
                !app.pulse_tick(std::time::Duration::from_millis(ms), 10),
                "{ms}ms"
            );
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

    /// Switching to a worktree with no remembered task moves the selection to
    /// `None` -- a genuinely different task (or nothing) than whatever was on
    /// screen before, so a scroll position left over from the old one would
    /// hide the new pane's content rather than show it.
    #[test]
    fn switching_worktrees_resets_the_detail_scroll() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.selected_worktree = Some("/x/one".into());
        app.selected = Some("a".into());
        app.detail_scroll = (12, 3);

        app.select_worktree("/x/two");

        assert_eq!(
            app.detail_scroll,
            (0, 0),
            "a stale scroll would hide the new content"
        );
    }

    #[test]
    fn load_store_replaces_the_task_list_and_expands_everything() {
        let mut app = counted(vec![task("old", None, &[])]);
        app.selected = Some("old".into());
        app.tree_offset = 4;
        app.detail_scroll = (7, 2);

        app.load_store(
            vec![task("root", None, &["kid"]), task("kid", Some("root"), &[])],
            "other".into(),
        );

        // The exact bug CLAUDE.md records as having shipped once: a store
        // switch that leaves the new tree collapsed to a single root because
        // it reused `apply_tasks`'s "only expand what is genuinely new"
        // rule against a tree where every id looks new by definition.
        assert!(
            app.expanded.contains("root"),
            "the new store opened collapsed"
        );
        assert_eq!(app.row_ids().len(), 2, "the child must be visible too");
        assert_eq!(app.store_label, "other");
        assert_eq!(
            app.selected.as_deref(),
            Some("root"),
            "old-store id must not linger"
        );
        assert_eq!(app.tree_offset, 0);
        assert_eq!(app.detail_scroll, (0, 0));
    }

    /// `load_store` must not confuse an id from the store being left with one
    /// that merely looks the same by coincidence -- there is no cross-store id
    /// comparison here at all, unlike `apply_tasks`, which is the whole reason
    /// this is a separate method.
    #[test]
    fn load_store_does_not_carry_over_expansion_from_the_old_store() {
        let mut app = counted(vec![task("shared-name", None, &["kid"])]);
        app.expanded.insert("shared-name".into());

        // A different store that happens to reuse the same id -- ids are
        // short slugs and a real collision across independent stores is
        // exactly the coincidence `apply_tasks` cannot be trusted to notice.
        app.load_store(vec![task("shared-name", None, &[])], "other".into());

        assert!(
            !app.expanded.contains("shared-name"),
            "a leaf must not be recorded as expanded just because an old id matched"
        );
    }

    /// The bug this closes: the previous `load_store` unconditionally reset
    /// `self.selected`, which made this pass only when `load_store` was
    /// called in isolation -- never through the real sequence `switch_store`
    /// actually drives, `select_worktree` immediately followed by
    /// `load_store`. Going through both here, in that order, is exactly the
    /// gap that let the regression through a unit test aimed at `load_store`
    /// alone.
    #[test]
    fn switching_back_to_a_worktree_restores_the_remembered_task_through_the_real_flow() {
        let mut app = counted(vec![task("a", None, &[]), task("b", None, &[])]);
        app.selected_worktree = Some("/x/one".into());
        app.selected = Some("b".into());

        // Leave "/x/one" for "/x/two" -- select_worktree remembers "b" for
        // "/x/one" in task_memory before this pair of calls runs again below.
        app.select_worktree("/x/two");
        app.load_store(vec![task("c", None, &[])], "two".into());

        // Return to "/x/one": select_worktree restores "b" from task_memory
        // first, and load_store must not stomp over it afterward.
        app.select_worktree("/x/one");
        app.load_store(
            vec![task("a", None, &[]), task("b", None, &[])],
            "one".into(),
        );

        assert_eq!(
            app.selected.as_deref(),
            Some("b"),
            "task_memory's restore must survive load_store, not just select_worktree alone"
        );
    }

    fn wt(path: &str, branch: &str, main: bool) -> crate::worktree::Worktree {
        crate::worktree::Worktree {
            path: path.to_string(),
            branch: branch.to_string(),
            is_main: main,
            is_locked: false,
            is_detached: false,
        }
    }

    fn repo(name: &str) -> crate::repos::Repo {
        crate::repos::Repo {
            name: name.to_string(),
            path: format!("/x/{name}"),
            worktrees: vec![
                wt(&format!("/x/{name}"), "main", true),
                wt(&format!("/x/{name}-feat"), "feat", false),
            ],
            open: true,
            registered: true,
            is_global: false,
        }
    }

    fn app_with_repos() -> App {
        let mut app = counted(vec![task("a", None, &[])]);
        app.repos = vec![repo("one"), repo("two")];
        // The geometry a real frame publishes. Left at its `App::new` default
        // of 0/0 these tests could not tell a click on an item row from one on
        // the pane's bottom border -- which is the shape of the bug
        // `clicking_a_row_selects_the_task_drawn_on_it_and_nothing_otherwise`
        // exists for, and the reason it had to be checked against a rendered
        // frame rather than against numbers picked here.
        app.body_top = 1;
        app.body_bottom = 21;
        // `here` is the repo the run launched in. `here_store` has to be a
        // directory that really exists, since the section is hidden when there
        // is no store where you are -- a temp dir is the cheapest real one.
        //
        // Unsaved, because that is now what puts a repo under `here` at all:
        // saving one moves it into `saved`. This fixture exists to give the
        // sidebar two populated sections, so it has to be the state that
        // produces them.
        app.repos[1].registered = false;
        app.here_path = Some("/x/two".into());
        app.here_store = std::env::temp_dir().to_string_lossy().into_owned();
        app
    }

    /// Puts the cursor on the first row matching `want`, so these say what
    /// they mean rather than pinning a layout: sections move rows about, and
    /// an index would be asserting the arrangement instead of the resolution.
    fn cursor_on(app: &mut App, want: crate::repos::Row) {
        app.selected_repo_row = app
            .repo_rows()
            .iter()
            .position(|r| *r == want)
            .unwrap_or_else(|| panic!("no such row {want:?} in {:?}", app.repo_rows()));
    }

    #[test]
    fn a_repo_row_resolves_to_its_own_main_worktree() {
        let mut app = app_with_repos();
        cursor_on(&mut app, crate::repos::Row::Repo { index: 0 });
        assert_eq!(app.selected_worktree_path().as_deref(), Some("/x/one"));
        assert_eq!(app.selected_repo().unwrap().name, "one");
    }

    #[test]
    fn a_worktree_row_resolves_to_that_exact_worktree() {
        let mut app = app_with_repos();
        cursor_on(&mut app, crate::repos::Row::Worktree { repo: 0, index: 1 });
        assert_eq!(app.selected_worktree_path().as_deref(), Some("/x/one-feat"));
        // But the *repo* it belongs to is still "one", not a worktree-shaped
        // thing -- D forgets the entry, not one worktree inside it.
        assert_eq!(app.selected_repo().unwrap().name, "one");
    }

    #[test]
    fn a_worktree_row_deep_in_the_second_repo_resolves_to_the_second_repo() {
        let mut app = app_with_repos();
        cursor_on(&mut app, crate::repos::Row::Worktree { repo: 1, index: 0 });
        assert_eq!(app.selected_repo().unwrap().name, "two");
    }

    #[test]
    fn an_empty_repo_list_resolves_nothing_rather_than_panicking() {
        let app = counted(vec![task("a", None, &[])]);
        assert_eq!(app.selected_worktree_path(), None);
        assert!(app.selected_repo().is_none());
    }

    #[test]
    fn repo_row_movement_clamps_at_both_ends() {
        let mut app = app_with_repos();
        let last = app.repo_rows().len() - 1;

        app.move_repo_row(-100);
        assert!(app.selected_repo_row <= last, "must not go negative");
        assert!(app.repo_rows()[app.selected_repo_row].selectable());

        app.move_repo_row(100);
        assert_eq!(
            app.selected_repo_row, last,
            "must not run past the last row"
        );
    }

    #[test]
    fn repo_row_movement_on_an_empty_list_does_nothing() {
        let mut app = counted(vec![task("a", None, &[])]);
        app.selected_repo_row = 0;
        app.move_repo_row(5);
        assert_eq!(app.selected_repo_row, 0);
    }

    #[test]
    fn g_and_shift_g_jump_to_the_first_and_last_repo_row() {
        let mut app = app_with_repos();
        let rows = app.repo_rows();
        let first = rows.iter().position(|r| r.selectable()).unwrap();
        let last = rows.iter().rposition(|r| r.selectable()).unwrap();

        app.select_last_repo_row();
        assert_eq!(app.selected_repo_row, last);

        app.select_first_repo_row();
        assert_eq!(
            app.selected_repo_row, first,
            "g must clear the `here` label"
        );
    }

    /// `crate::test_support::with_isolated_registry`, not a copy of its own:
    /// this module and `registry.rs`'s own tests both mutate the same
    /// process-wide `XDG_CONFIG_HOME`, and two independent locks -- one per
    /// module -- would not actually exclude each other from it. Only one
    /// shared lock, used by both, does.
    use crate::test_support::with_isolated_registry;

    #[test]
    fn registering_adds_the_repo_and_reports_the_change() {
        with_isolated_registry("app-register-add", || {
            let mut app = counted(vec![task("a", None, &[])]);
            app.registry = crate::registry::Registry::default();

            assert!(app.register_repo_path("/x/dextui").unwrap());
            assert_eq!(app.registry.repos, vec!["/x/dextui".to_string()]);
        });
    }

    #[test]
    fn registering_a_known_repo_is_reported_not_duplicated() {
        with_isolated_registry("app-register-duplicate", || {
            let mut app = counted(vec![task("a", None, &[])]);
            app.registry = crate::registry::Registry::default();
            app.register_repo_path("/x/dextui").unwrap();

            assert!(
                !app.register_repo_path("/x/dextui").unwrap(),
                "a duplicate must report that nothing changed"
            );
            assert_eq!(app.registry.repos.len(), 1);
        });
    }

    /// Unregistering is a view operation. It must never touch the worktree,
    /// the branch or the store -- only the entry and the row.
    #[test]
    fn unregistering_removes_only_the_entry() {
        with_isolated_registry("app-unregister-entry", || {
            let mut app = counted(vec![task("a", None, &[])]);
            app.registry = crate::registry::Registry::default();
            app.register_repo_path("/x/one").unwrap();
            app.register_repo_path("/x/two").unwrap();

            assert!(app.unregister_repo_path("/x/one").unwrap());
            assert_eq!(app.registry.repos, vec!["/x/two".to_string()]);
        });
    }

    /// Removing a repo that is actually loaded into `app.repos` must also
    /// drop its row, and clamp the cursor rather than leave it pointing past
    /// the end of a now-shorter list.
    #[test]
    fn unregistering_a_loaded_repo_drops_its_row_and_clamps_the_cursor() {
        with_isolated_registry("app-unregister-loaded-repo", || {
            let mut app = app_with_repos(); // "one" then "two", 6 rows total
            // Through `register_repo_path`, not a bare `registry.add`: the
            // latter only mutates the in-memory copy, and `unregister_repo_path`
            // now re-reads the file fresh (see `Registry::remove_and_save`),
            // so anything this test wants it to find has to actually be saved.
            app.register_repo_path("/x/one").unwrap();
            app.register_repo_path("/x/two").unwrap();
            app.selected_repo_row = app.repo_rows().len() - 1;

            // "one" is not the repo this run launched in, so nothing keeps it.
            assert!(app.unregister_repo_path("/x/one").unwrap());

            assert_eq!(app.repos.len(), 1, "the repo's own row must be gone too");
            assert_eq!(app.repos[0].name, "two");
            assert!(
                app.selected_repo_row < app.repo_rows().len(),
                "cursor left pointing past the end: {} vs {} rows",
                app.selected_repo_row,
                app.repo_rows().len()
            );
        });
    }

    /// The bug this closes: `unregister_repo_path` used to discard the save
    /// error and report success unconditionally, so a removal that never
    /// reached disk would silently reappear at the next launch. Simulated
    /// here the same way `registry.rs`'s own tests provoke a non-`NotFound`
    /// read error: a directory sitting where the registry file belongs.
    #[test]
    fn a_failed_unregister_save_is_reported_and_the_row_is_kept() {
        with_isolated_registry("app-unregister-save-fails", || {
            let mut app = app_with_repos();
            app.register_repo_path("/x/one").unwrap();

            // Break the file *after* the successful registration above, so
            // the failure is specific to this save, not to setup.
            let p = crate::registry::path().unwrap();
            std::fs::remove_file(&p).unwrap();
            std::fs::create_dir_all(&p).unwrap();

            let before = app.repos.len();
            let err = app.unregister_repo_path("/x/one").unwrap_err();
            assert!(!err.is_empty());
            assert_eq!(
                app.repos.len(),
                before,
                "the row must survive a failed save"
            );
        });
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

    /// With no sidebar on screen the cycle is the two panes it always was.
    #[test]
    fn tab_moves_focus_between_the_panes() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.repos_pane_above = 0;
        assert_eq!(app.focus, Focus::Tree);
        app.cycle_focus(true);
        assert_eq!(app.focus, Focus::Detail);
        app.cycle_focus(true);
        assert_eq!(app.focus, Focus::Tree);
    }

    /// Left to right, and wrapping -- the same order as the `[1] [2] [3]`
    /// keys, because two ways of reaching the same three panes disagreeing
    /// about their order would be worse than either alone.
    #[test]
    fn tab_walks_all_three_panes_when_the_sidebar_is_shown() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.repos_pane_above = 110;
        app.terminal_width = 140;
        app.repos_visible = true; // shown, not the hidden default

        let mut seen = vec![app.focus];
        for _ in 0..3 {
            app.cycle_focus(true);
            seen.push(app.focus);
        }
        assert_eq!(
            seen,
            vec![Focus::Tree, Focus::Detail, Focus::Repos, Focus::Tree],
            "tab should wrap through all three, in drawn order"
        );

        // And shift-tab is exactly the inverse.
        let mut back = vec![app.focus];
        for _ in 0..3 {
            app.cycle_focus(false);
            back.push(app.focus);
        }
        assert_eq!(
            back,
            vec![Focus::Tree, Focus::Repos, Focus::Detail, Focus::Tree]
        );
    }

    /// The cycle follows what is drawn, so hiding the sidebar with `b` takes
    /// it out -- tab must never land on a pane that is not there.
    #[test]
    fn tab_skips_the_sidebar_once_it_is_hidden() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.repos_pane_above = 110;
        app.terminal_width = 140;
        app.repos_visible = true; // starts shown, so toggle_repos is the `b` that hides it
        app.toggle_repos();

        for _ in 0..4 {
            app.cycle_focus(true);
            assert_ne!(app.focus, Focus::Repos, "tab landed on a hidden sidebar");
        }
    }

    /// Focused on the sidebar when it leaves the cycle, tab has no position to
    /// step from. It must still go somewhere sensible rather than computing an
    /// offset from an index that does not exist.
    #[test]
    fn tab_from_a_pane_that_has_left_the_cycle_lands_on_the_first() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        app.repos_pane_above = 0;
        app.focus = Focus::Repos;

        app.cycle_focus(true);

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

    /// The reported bug: grabbing the divider jumped it a full sidebar-width
    /// to the right before the drag had moved anywhere.
    ///
    /// The layout is `[Length(repos_width), Percentage(p), Fill(1)]`, so the
    /// divider lands at `repos_width + p% of W` -- but the percentage was
    /// computed from the raw column, which silently assumed the tree started
    /// at the body's left edge. It does, in two panes; it does not once the
    /// sidebar is there.
    #[test]
    fn dragging_the_split_puts_the_divider_where_the_pointer_is() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);
        app.repos_right = 26;

        app.set_split(60, 100);

        // Where the renderer will put it: the sidebar, then that share of what
        // is left over.
        let span = 100 - app.repos_right;
        let landed = app.repos_right + span * app.split_percent / 100;
        assert_eq!(
            landed,
            60,
            "the divider moved {} cells from the pointer",
            landed as i32 - 60
        );
    }

    /// Widening the sidebar must cost both panes, not one. The percentage is
    /// of the region the two share, so their *ratio* survives the sidebar
    /// changing size -- before this the tree's width was pinned to the whole
    /// body and the detail pane absorbed every cell the sidebar took.
    #[test]
    fn widening_the_sidebar_costs_both_panes_proportionally() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);
        app.split_percent = 50;

        let widths = |repos: u16| {
            let span = 100 - repos;
            let tree = span * 50 / 100;
            (tree, span - tree)
        };

        let (tree_narrow, detail_narrow) = widths(20);
        let (tree_wide, detail_wide) = widths(50);

        assert!(
            tree_wide < tree_narrow,
            "the tree kept its width: {tree_wide}"
        );
        assert!(detail_wide < detail_narrow, "the detail should shrink too");
        assert_eq!(
            tree_narrow - tree_wide,
            detail_narrow - detail_wide,
            "an even split should lose evenly"
        );
    }

    /// And with no sidebar the arithmetic is unchanged, which is why this was
    /// invisible until a third pane existed.
    #[test]
    fn dragging_the_split_is_unchanged_without_a_sidebar() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);
        app.repos_right = 0;

        app.set_split(60, 100);

        assert_eq!(app.split_percent, 60);
    }

    /// The sidebar boundary is its own draggable divider.
    #[test]
    fn the_sidebar_edge_is_grabbable_and_resizes_it() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);
        app.repos_right = 26;

        for col in [24, 25, 26] {
            assert_eq!(
                app.divider_at(col),
                Some(Divider::Repos),
                "column {col} should grab the sidebar edge"
            );
        }

        app.set_repos_width(40, 100);
        assert_eq!(app.repos_width, 40);
    }

    /// Neither end may be dragged away: too narrow and a branch name is
    /// unreadable, too wide and the pane the sidebar exists to navigate *to*
    /// is squeezed out by the one doing the navigating.
    #[test]
    fn the_sidebar_cannot_be_dragged_to_either_extreme() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);

        app.set_repos_width(0, 100);
        assert_eq!(app.repos_width, App::REPOS_WIDTH_MIN);

        app.set_repos_width(99, 100);
        assert_eq!(app.repos_width, 50, "never past half the terminal");
    }

    #[test]
    fn the_divider_is_grabbable_without_pixel_precision() {
        let mut app = App::new(vec![task("a", None, &[])], "t".into(), Config::default());
        geo(&mut app);

        for col in [44, 45, 46] {
            assert_eq!(
                app.divider_at(col),
                Some(Divider::Split),
                "column {col} should grab the split"
            );
        }
        for col in [10, 43, 47, 90] {
            assert!(app.divider_at(col).is_none(), "column {col} should not");
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
            vec![
                task("a", None, &[]),
                task("b", None, &[]),
                task("c", None, &[]),
            ],
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
        assert_eq!(
            app.selected.as_deref(),
            Some("a"),
            "selection moved to nothing"
        );
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
        assert_eq!(
            app.selected.as_deref(),
            Some("b"),
            "border click moved selection"
        );
    }

    #[test]
    fn a_scrolled_list_maps_clicks_through_the_offset() {
        // Without honouring the offset, every click would address the top of the
        // list rather than what is actually drawn.
        let mut app = App::new(
            vec![
                task("a", None, &[]),
                task("b", None, &[]),
                task("c", None, &[]),
            ],
            "t".into(),
            Config::default(),
        );
        geo(&mut app);
        app.tree_offset = 2;

        app.select_at_row(app.body_top + 1);
        assert_eq!(app.selected.as_deref(), Some("c"));
    }

    /// `p` -> `c` -> `g`, one chain, so every row is the last of its siblings
    /// and the prefixes are `└`, `  └`, `    └`. With the border and the
    /// two-cell gutter that puts the markers at columns 4, 6 and 8.
    fn nested() -> App {
        let mut app = App::new(
            vec![
                task("p", None, &["c"]),
                task("c", Some("p"), &["g"]),
                task("g", Some("c"), &[]),
            ],
            "t".into(),
            Config::default(),
        );
        geo(&mut app);
        // `App::new` opens everything on first load, so this starts expanded.
        assert_eq!(app.row_ids(), vec!["p", "c", "g"], "fixture is not open");
        app
    }

    #[test]
    fn clicking_the_marker_closes_the_row_and_clicking_it_again_reopens_it() {
        let mut app = nested();

        app.click_tree(4, app.body_top + 1);
        assert_eq!(app.row_ids(), vec!["p"], "the marker did not close the row");
        assert_eq!(app.selected.as_deref(), Some("p"), "and it must select too");

        app.click_tree(4, app.body_top + 1);
        assert_eq!(app.row_ids(), vec!["p", "c", "g"], "it did not reopen");
    }

    /// The pad space after the glyph is part of the marker's own span, so the
    /// zone is two cells -- a one-cell pointer target is a poor thing to ask
    /// for. The branch character before it is tree drawing and stays out.
    #[test]
    fn the_marker_zone_is_the_glyph_and_its_pad_space_and_nothing_else() {
        for (column, toggles) in [(3, false), (4, true), (5, true), (6, false)] {
            let mut app = nested();
            app.click_tree(column, app.body_top + 1);
            assert_eq!(
                app.row_ids().len() == 1,
                toggles,
                "column {column} should {} have toggled",
                if toggles { "" } else { "not" }
            );
        }
    }

    /// The zone is offset by the row's own indentation. Without that, column 4
    /// would be "the marker" on every row, so clicking a nested row's twisty
    /// would land in dead space while clicking its indent would open it.
    #[test]
    fn a_nested_rows_marker_moves_right_with_its_indent() {
        let mut app = nested();
        // `p`'s marker column, but on `c`'s row -- indentation, not a marker.
        app.click_tree(4, app.body_top + 2);
        assert_eq!(
            app.row_ids(),
            vec!["p", "c", "g"],
            "indent acted as a marker"
        );
        assert_eq!(app.selected.as_deref(), Some("c"), "it should still select");

        app.click_tree(6, app.body_top + 2);
        assert_eq!(app.row_ids(), vec!["p", "c"], "c's own marker did nothing");
    }

    /// A leaf has a glyph drawn in the marker column too, but nothing to open.
    #[test]
    fn clicking_a_leafs_marker_only_selects() {
        let mut app = nested();
        let before = app.expanded.clone();

        app.click_tree(8, app.body_top + 3);
        assert_eq!(app.selected.as_deref(), Some("g"));
        assert_eq!(app.expanded, before, "a leaf was recorded as expanded");
    }

    /// The tree does not start at column 0 when the sidebar is drawn, and the
    /// zone has to move with it -- otherwise the marker is unclickable in the
    /// three-pane layout, which is the default on a wide terminal.
    #[test]
    fn the_marker_zone_follows_the_tree_past_the_sidebar() {
        let mut app = nested();
        app.divider_x = 90;
        app.repos_right = 26;

        app.click_tree(4, app.body_top + 1);
        assert_eq!(app.row_ids().len(), 3, "column 4 is inside the sidebar");

        app.click_tree(30, app.body_top + 1);
        assert_eq!(
            app.row_ids(),
            vec!["p"],
            "the zone did not shift with the pane"
        );
    }

    /// The same gesture has to do the same thing in both panes: content slides,
    /// nothing about *what is selected* changes. `scroll_detail` never touched
    /// a selection to begin with; this pins that `scroll_tree` doesn't either.
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
            app.selected.as_deref(),
            Some("f"),
            "the wheel must not reselect"
        );

        app.scroll_tree(-2);
        assert_eq!(app.tree_offset, 3, "the tree did not come back");
        assert_eq!(app.selected.as_deref(), Some("f"));
    }

    /// The offset clamps at the ends of the list -- it has nothing to scroll
    /// into past the first or last row -- but the selection set before
    /// scrolling began is untouched throughout, including while the offset
    /// has run past where that task is drawn.
    #[test]
    fn scrolling_past_the_ends_of_the_tree_stops() {
        let tasks: Vec<Task> = ('a'..='e')
            .map(|c| task(&c.to_string(), None, &[]))
            .collect();
        let mut app = App::new(tasks, "t".into(), Config::default());
        geo(&mut app);
        app.select(Some("a".into()));

        app.scroll_tree(50);
        assert_eq!(
            app.selected.as_deref(),
            Some("a"),
            "the wheel must not reselect"
        );
        assert!(
            app.tree_offset <= 4,
            "offset ran past the list: {}",
            app.tree_offset
        );

        app.scroll_tree(-50);
        assert_eq!(app.selected.as_deref(), Some("a"));
        assert_eq!(app.tree_offset, 0);
    }

    /// `scroll_tree` must never ask `draw_tree` to reveal the selection --
    /// doing so even once would hand `ratatui::List` the real (unmoved)
    /// selected index and let it pull `tree_offset` straight back, which is
    /// the bug this field exists to prevent. See `App::needs_tree_reveal`.
    #[test]
    fn scroll_tree_does_not_ask_for_a_reveal() {
        let mut app = App::new(
            vec![task("a", None, &[]), task("b", None, &[])],
            "t".into(),
            Config::default(),
        );
        app.needs_tree_reveal = false; // as it would be on every frame after the first
        app.scroll_tree(1);
        assert!(
            !app.needs_tree_reveal,
            "a wheel scroll must not ask for a reveal"
        );
    }

    /// A real selection change is the one thing that must ask for a reveal --
    /// otherwise pressing `j` onto a row currently below the fold would leave
    /// the cursor drawn nowhere, since nothing else scrolls `tree_offset` to
    /// follow it.
    #[test]
    fn a_real_selection_change_asks_for_a_reveal() {
        let mut app = App::new(
            vec![task("a", None, &[]), task("b", None, &[])],
            "t".into(),
            Config::default(),
        );
        app.needs_tree_reveal = false;
        app.move_selection(1);
        assert!(
            app.needs_tree_reveal,
            "a moved selection must ask for a reveal"
        );
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
