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
    EditDescription { id: String, name: String },
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
            focus: Focus::Tree,
            detail_scroll: (0, 0),
            wrap: cfg.wrap,
            detail_content_height: 0,
            detail_viewport_height: 0,
            pending_refresh: false,
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

        // A selection filtered out of view must not linger invisibly.
        if let Some(sel) = self.selected.clone()
            && !self.visible_ids().contains(&sel) {
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

    pub fn select_first(&mut self) {
        let id = self.row_ids().first().cloned();
        self.select(id);
    }

    pub fn select_last(&mut self) {
        let id = self.row_ids().last().cloned();
        self.select(id);
    }

    /// Right arrow: open the node, or step into it if already open.
    pub fn expand_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let has_kids = tree::flatten(&self.tree)
            .iter()
            .any(|n| n.task.id == id && !n.children.is_empty());

        if has_kids && !self.expanded.contains(&id) {
            self.expanded.insert(id);
        } else if has_kids {
            self.move_selection(1);
        }
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

    /// Pending and in-progress totals across the whole store, for the header.
    pub fn counts(&self) -> (usize, usize) {
        let pending = self.tasks.iter().filter(|t| !t.completed).count();
        let active = self
            .tasks
            .iter()
            .filter(|t| !t.completed && t.started_at.is_some())
            .count();
        (pending, active)
    }

    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.rebuild();
    }

    pub fn toggle_sort_direction(&mut self) {
        self.sort_reversed = !self.sort_reversed;
        self.rebuild();
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Tree => Focus::Detail,
            Focus::Detail => Focus::Tree,
        };
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
}
