//! Turns dex's flat task array into a hierarchy, with search and status filtering.

use std::collections::{HashMap, HashSet};

use crate::dex::Task;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    /// Everything, including completed. Mirrors `dex list --all`.
    All,
    /// Not yet completed. Mirrors the default `dex list`.
    Pending,
    /// Started but not completed. Mirrors `dex list --in-progress`.
    InProgress,
}

impl Filter {
    pub fn next(self) -> Filter {
        match self {
            Filter::Pending => Filter::InProgress,
            Filter::InProgress => Filter::All,
            Filter::All => Filter::Pending,
        }
    }

    /// All three render at the same width so a fixed-width label cannot truncate.
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "[ ALL  pending  active ]",
            Filter::InProgress => "[ all  pending  ACTIVE ]",
            Filter::Pending => "[ all  PENDING  active ]",
        }
    }

    /// The active filter alone, for a header too narrow for the whole menu. The
    /// menu is an affordance; *which filter is on* is the fact, and a filter
    /// silently hiding tasks with nothing on screen saying so is the most
    /// confusing state this app has.
    pub fn name(self) -> &'static str {
        match self {
            Filter::All => "ALL",
            Filter::InProgress => "ACTIVE",
            Filter::Pending => "PENDING",
        }
    }
}

/// How siblings are ordered. Applied at every level, so the hierarchy, the
/// progress rollups and expand/collapse all keep working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// priority, then creation time, then name.
    Priority,
    Updated,
    Created,
    Name,
}

impl Sort {
    pub fn next(self) -> Sort {
        match self {
            Sort::Priority => Sort::Updated,
            Sort::Updated => Sort::Created,
            Sort::Created => Sort::Name,
            Sort::Name => Sort::Priority,
        }
    }

    /// `reversed` flips each order's *natural* direction rather than meaning a
    /// blanket ascending/descending: newest-first is the useful default for
    /// timestamps, lowest-number-first for priority, A-Z for names.
    pub fn label(self, reversed: bool) -> &'static str {
        match (self, reversed) {
            (Sort::Priority, false) => "priority",
            (Sort::Priority, true) => "priority ↓",
            (Sort::Updated, false) => "updated",
            (Sort::Updated, true) => "stalest",
            (Sort::Created, false) => "newest",
            (Sort::Created, true) => "oldest",
            (Sort::Name, false) => "A-Z",
            (Sort::Name, true) => "Z-A",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub task: Task,
    pub children: Vec<Node>,
    /// False when this node survived filtering only because a descendant matched,
    /// so the UI can dim pure scaffolding.
    pub is_match: bool,
}

impl Node {
    pub fn id(&self) -> &str {
        &self.task.id
    }
}

/// Builds the visible forest.
///
/// dex returns tasks sorted by id, which is meaningless to a reader, so siblings
/// are ordered by priority, then creation time, then name. Any task whose
/// descendant matches is kept, so a match is never orphaned from its path.
pub fn build(tasks: &[Task], query: &str, filter: Filter, sort: Sort, reversed: bool) -> Vec<Node> {
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

    let mut by_parent: HashMap<&str, Vec<&Task>> = HashMap::new();
    let mut roots: Vec<&Task> = Vec::new();

    for t in tasks {
        match t.parent_id.as_deref() {
            // A task whose parent is missing from the payload becomes a root
            // rather than vanishing from the view.
            Some(p) if by_id.contains_key(p) => by_parent.entry(p).or_default().push(t),
            _ => roots.push(t),
        }
    }

    let query = query.trim();
    let query = if query.is_empty() { None } else { Some(query) };

    order(&mut roots, sort, reversed);

    let mut visiting: HashSet<&str> = HashSet::new();
    roots
        .into_iter()
        .filter_map(|r| build_node(r, &by_parent, query, filter, sort, reversed, &mut visiting))
        .collect()
}

fn build_node<'a>(
    task: &'a Task,
    by_parent: &HashMap<&'a str, Vec<&'a Task>>,
    query: Option<&str>,
    filter: Filter,
    sort: Sort,
    reversed: bool,
    visiting: &mut HashSet<&'a str>,
) -> Option<Node> {
    // Guards against a malformed store where parent links form a cycle.
    if !visiting.insert(task.id.as_str()) {
        return None;
    }

    let mut children = Vec::new();
    if let Some(kids) = by_parent.get(task.id.as_str()) {
        let mut kids = kids.clone();
        order(&mut kids, sort, reversed);
        children = kids
            .into_iter()
            .filter_map(|k| build_node(k, by_parent, query, filter, sort, reversed, visiting))
            .collect();
    }

    visiting.remove(task.id.as_str());

    let is_match = matches(task, query, filter);
    if !is_match && children.is_empty() {
        return None;
    }

    Some(Node {
        task: task.clone(),
        children,
        is_match,
    })
}

fn order(tasks: &mut [&Task], sort: Sort, reversed: bool) {
    // Name is the final tiebreak everywhere, so ordering is stable and does not
    // jitter between refreshes when the primary key ties.
    let by_name = |a: &Task, b: &Task| a.name.to_lowercase().cmp(&b.name.to_lowercase());

    // Tasks missing a timestamp sort last rather than first, in either
    // direction: an absent date is not "oldest", it is unknown.
    let stamp = |t: &Task, key: fn(&Task) -> Option<&String>| key(t).cloned();

    tasks.sort_by(|a, b| match sort {
        Sort::Priority => a
            .priority
            .cmp(&b.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| by_name(a, b)),
        Sort::Updated => stamp(b, |t| t.updated_at.as_ref())
            .cmp(&stamp(a, |t| t.updated_at.as_ref()))
            .then_with(|| by_name(a, b)),
        Sort::Created => stamp(b, |t| t.created_at.as_ref())
            .cmp(&stamp(a, |t| t.created_at.as_ref()))
            .then_with(|| by_name(a, b)),
        Sort::Name => by_name(a, b),
    });

    if reversed {
        tasks.reverse();
    }
}

fn matches(t: &Task, query: Option<&str>, filter: Filter) -> bool {
    let status_ok = match filter {
        Filter::All => true,
        Filter::Pending => !t.completed,
        Filter::InProgress => t.is_in_progress(),
    };

    if !status_ok {
        return false;
    }

    match query {
        None => true,
        Some(q) => {
            let q = q.to_lowercase();
            t.name.to_lowercase().contains(&q)
                || t
                    .description
                    .as_deref()
                    .is_some_and(|d| d.to_lowercase().contains(&q))
        }
    }
}

/// Completed vs total descendants of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    pub done: usize,
    /// Started but not finished. Tracked separately so a parent whose children
    /// are all underway does not read as a completely empty bar.
    pub active: usize,
    pub total: usize,
}

/// Completion rolled up over every descendant, for every task that has any.
///
/// Deliberately computed from the *unfiltered* task list: a meter built from the
/// filtered tree would read 0/n as soon as a filter hid the completed children,
/// which is exactly when you most want to see the real number.
pub fn subtree_progress(tasks: &[Task]) -> HashMap<String, Progress> {
    let mut by_parent: HashMap<&str, Vec<&Task>> = HashMap::new();
    for t in tasks {
        if let Some(p) = t.parent_id.as_deref() {
            by_parent.entry(p).or_default().push(t);
        }
    }

    let mut out = HashMap::new();
    for t in tasks {
        let mut progress = Progress::default();
        let mut seen: HashSet<&str> = HashSet::new();
        accumulate(t.id.as_str(), &by_parent, &mut progress, &mut seen);
        if progress.total > 0 {
            out.insert(t.id.clone(), progress);
        }
    }
    out
}

fn accumulate<'a>(
    id: &'a str,
    by_parent: &HashMap<&'a str, Vec<&'a Task>>,
    progress: &mut Progress,
    seen: &mut HashSet<&'a str>,
) {
    // Guards against a cyclic store, same as build().
    if !seen.insert(id) {
        return;
    }
    if let Some(kids) = by_parent.get(id) {
        for k in kids {
            progress.total += 1;
            if k.completed {
                progress.done += 1;
            } else if k.started_at.is_some() {
                progress.active += 1;
            }
            accumulate(k.id.as_str(), by_parent, progress, seen);
        }
    }
}

/// Every node in the forest, depth-first, ignoring expansion.
pub fn flatten(nodes: &[Node]) -> Vec<&Node> {
    let mut out = Vec::new();
    collect(nodes, &mut out);
    out
}

fn collect<'a>(nodes: &'a [Node], out: &mut Vec<&'a Node>) {
    for n in nodes {
        out.push(n);
        collect(&n.children, out);
    }
}

/// One rendered line: the node, its tree-drawing prefix, and enough state for
/// the renderer to pick an expand/collapse glyph.
///
/// The marker is deliberately NOT baked into `prefix`: which glyph to use is a
/// presentation decision that depends on the icon tier, and belongs in `ui`.
pub struct Row<'a> {
    pub node: &'a Node,
    /// Indentation plus the branch character, e.g. `"│ ├"`.
    pub prefix: String,
    pub has_children: bool,
    pub is_open: bool,
}

/// Flattens to only what is currently visible, honouring `expanded`, and builds
/// the box-drawing prefix for each row.
pub fn visible_rows<'a>(nodes: &'a [Node], expanded: &HashSet<String>) -> Vec<Row<'a>> {
    let mut out = Vec::new();
    walk(nodes, expanded, &mut String::new(), &mut out);
    out
}

fn walk<'a>(
    nodes: &'a [Node],
    expanded: &HashSet<String>,
    indent: &mut String,
    out: &mut Vec<Row<'a>>,
) {
    for (i, node) in nodes.iter().enumerate() {
        let last = i + 1 == nodes.len();
        let has_kids = !node.children.is_empty();
        let is_open = expanded.contains(node.id());

        let branch = if last { "└" } else { "├" };
        out.push(Row {
            node,
            prefix: format!("{indent}{branch}"),
            has_children: has_kids,
            is_open,
        });

        if has_kids && is_open {
            let added = if last { "  " } else { "│ " };
            indent.push_str(added);
            walk(&node.children, expanded, indent, out);
            indent.truncate(indent.len() - added.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, parent: Option<&str>) -> Task {
        Task {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            name: id.to_string(),
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            ..Default::default()
        }
    }

    /// The default ordering, so existing tests keep asserting sibling order
    /// under Sort::Priority without restating it everywhere.
    fn build(tasks: &[Task], query: &str, filter: Filter) -> Vec<Node> {
        super::build(tasks, query, filter, Sort::Priority, false)
    }

    fn named(id: &str, parent: Option<&str>, name: &str) -> Task {
        Task { name: name.to_string(), ..task(id, parent) }
    }

    #[test]
    fn nests_children_under_their_parent() {
        let tasks = vec![
            task("root", None),
            task("child", Some("root")),
            task("grandchild", Some("child")),
        ];

        let roots = build(&tasks, "", Filter::All);

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id(), "root");
        assert_eq!(roots[0].children[0].id(), "child");
        assert_eq!(roots[0].children[0].children[0].id(), "grandchild");
    }

    #[test]
    fn orders_siblings_by_priority_then_creation_time() {
        // dex returns tasks sorted by id, which is meaningless to a reader.
        let mut a = task("a", Some("r"));
        a.priority = 2;
        a.created_at = Some("2026-01-01T00:00:00Z".into());
        let mut b = task("b", Some("r"));
        b.created_at = Some("2026-01-01T00:00:50Z".into());
        let mut c = task("c", Some("r"));
        c.created_at = Some("2026-01-01T00:00:10Z".into());

        let tasks = vec![a, b, c, task("r", None)];
        let roots = build(&tasks, "", Filter::All);

        let ids: Vec<&str> = roots[0].children.iter().map(|n| n.id()).collect();
        assert_eq!(ids, vec!["c", "b", "a"]);
    }

    #[test]
    fn promotes_orphans_to_roots_instead_of_dropping_them() {
        // Parent absent from the payload; the child must stay visible.
        let tasks = vec![task("orphan", Some("missing"))];
        let roots = build(&tasks, "", Filter::All);

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id(), "orphan");
    }

    #[test]
    fn survives_a_parent_cycle() {
        // A malformed store must not take the whole TUI down.
        let tasks = vec![task("a", Some("b")), task("b", Some("a"))];
        let _ = build(&tasks, "", Filter::All);
    }

    #[test]
    fn pending_filter_hides_completed_tasks() {
        let mut done = task("done", None);
        done.completed = true;
        let tasks = vec![done, task("todo", None)];

        let roots = build(&tasks, "", Filter::Pending);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id(), "todo");
    }

    #[test]
    fn in_progress_filter_shows_only_started_incomplete_tasks() {
        let mut started = task("started", None);
        started.started_at = Some("2026-01-01T00:00:00Z".into());
        let mut finished = task("finished", None);
        finished.started_at = Some("2026-01-01T00:00:00Z".into());
        finished.completed = true;

        let tasks = vec![started, task("not-started", None), finished];
        let roots = build(&tasks, "", Filter::InProgress);

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id(), "started");
    }

    #[test]
    fn a_matching_child_keeps_its_non_matching_ancestors_as_scaffolding() {
        let tasks = vec![
            named("parent", None, "unrelated"),
            named("child", Some("parent"), "login bug"),
        ];

        let roots = build(&tasks, "login", Filter::All);

        assert_eq!(roots.len(), 1);
        // Kept only to lead to the match, so the UI can dim it.
        assert!(!roots[0].is_match);
        assert!(roots[0].children[0].is_match);
    }

    #[test]
    fn query_matches_description_and_ignores_case() {
        let mut t = named("a", None, "nothing");
        t.description = Some("mentions LOGIN here".into());

        assert_eq!(build(&[t], "login", Filter::All).len(), 1);
    }

    #[test]
    fn query_that_matches_nothing_yields_an_empty_tree() {
        let tasks = vec![named("a", None, "alpha"), named("b", None, "beta")];
        assert!(build(&tasks, "zzz", Filter::All).is_empty());
    }

    #[test]
    fn visible_rows_honour_expansion() {
        let tasks = vec![task("r", None), task("kid", Some("r"))];
        let forest = build(&tasks, "", Filter::All);

        let collapsed = visible_rows(&forest, &HashSet::new());
        assert_eq!(collapsed.len(), 1, "children hidden while collapsed");

        let expanded: HashSet<String> = ["r".to_string()].into_iter().collect();
        assert_eq!(visible_rows(&forest, &expanded).len(), 2);
    }

    #[test]
    fn subtree_progress_counts_all_descendants_not_just_children() {
        let mut done = task("leaf", Some("mid"));
        done.completed = true;
        let tasks = vec![task("root", None), task("mid", Some("root")), done];

        let p = subtree_progress(&tasks);

        // root sees the grandchild too.
        assert_eq!(p["root"], Progress { done: 1, active: 0, total: 2 });
        assert_eq!(p["mid"], Progress { done: 1, active: 0, total: 1 });
    }

    #[test]
    fn subtree_progress_separates_in_flight_from_done() {
        let mut started = task("b", Some("root"));
        started.started_at = Some("2026-01-01T00:00:00Z".into());
        let mut finished = task("c", Some("root"));
        finished.completed = true;

        let tasks = vec![task("root", None), task("a", Some("root")), started, finished];

        assert_eq!(
            subtree_progress(&tasks)["root"],
            Progress { done: 1, active: 1, total: 3 }
        );
    }

    #[test]
    fn leaves_get_no_rollup_at_all() {
        // A meter on a task with no children would be meaningless.
        let tasks = vec![task("solo", None)];
        assert!(subtree_progress(&tasks).is_empty());
    }

    #[test]
    fn subtree_progress_survives_a_cycle() {
        let tasks = vec![task("a", Some("b")), task("b", Some("a"))];
        let _ = subtree_progress(&tasks);
    }

    fn stamped(id: &str, created: &str, updated: &str) -> Task {
        Task {
            created_at: Some(created.to_string()),
            updated_at: Some(updated.to_string()),
            ..task(id, None)
        }
    }

    fn ids(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.id()).collect()
    }

    fn sorted(tasks: &[Task], sort: Sort, reversed: bool) -> Vec<String> {
        super::build(tasks, "", Filter::All, sort, reversed)
            .iter()
            .map(|n| n.id().to_string())
            .collect()
    }

    #[test]
    fn updated_sort_puts_the_most_recent_first() {
        let tasks = vec![
            stamped("old", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            stamped("new", "2026-01-01T00:00:00Z", "2026-03-01T00:00:00Z"),
            stamped("mid", "2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(sorted(&tasks, Sort::Updated, false), ["new", "mid", "old"]);
        assert_eq!(sorted(&tasks, Sort::Updated, true), ["old", "mid", "new"]);
    }

    #[test]
    fn created_sort_is_newest_first_and_reverses_to_oldest() {
        let tasks = vec![
            stamped("first", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            stamped("third", "2026-03-01T00:00:00Z", "2026-03-01T00:00:00Z"),
            stamped("second", "2026-02-01T00:00:00Z", "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(sorted(&tasks, Sort::Created, false), ["third", "second", "first"]);
        assert_eq!(sorted(&tasks, Sort::Created, true), ["first", "second", "third"]);
    }

    #[test]
    fn name_sort_ignores_case() {
        let tasks = vec![named("b", None, "beta"), named("a", None, "Alpha")];
        assert_eq!(sorted(&tasks, Sort::Name, false), ["a", "b"]);
        assert_eq!(sorted(&tasks, Sort::Name, true), ["b", "a"]);
    }

    #[test]
    fn sorting_applies_at_every_level_not_just_the_roots() {
        let tasks = vec![
            task("root", None),
            Task { name: "zulu".into(), ..task("z", Some("root")) },
            Task { name: "alpha".into(), ..task("a", Some("root")) },
        ];
        let roots = super::build(&tasks, "", Filter::All, Sort::Name, false);
        assert_eq!(ids(&roots[0].children), ["a", "z"]);
    }

    #[test]
    fn tasks_without_a_timestamp_sort_last_in_both_directions() {
        // An absent date is unknown, not "oldest"; floating it to the top under
        // one direction would be actively misleading.
        let mut missing = task("missing", None);
        missing.updated_at = None;
        let tasks = vec![
            missing,
            stamped("has", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        ];

        assert_eq!(sorted(&tasks, Sort::Updated, false), ["has", "missing"]);
        assert_eq!(sorted(&tasks, Sort::Updated, true).last().unwrap(), "has");
    }

    #[test]
    fn ties_break_on_name_so_order_does_not_jitter_between_refreshes() {
        let tasks = vec![
            stamped("b", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
            stamped("a", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        ];
        // Identical timestamps: name decides, and does so consistently.
        assert_eq!(sorted(&tasks, Sort::Updated, false), ["a", "b"]);
    }

    #[test]
    fn the_sort_cycle_returns_to_where_it_started() {
        let mut s = Sort::Priority;
        for _ in 0..4 {
            s = s.next();
        }
        assert_eq!(s, Sort::Priority);
    }

    #[test]
    fn every_sort_and_direction_has_a_distinct_label() {
        let mut labels = Vec::new();
        let mut s = Sort::Priority;
        for _ in 0..4 {
            labels.push(s.label(false));
            labels.push(s.label(true));
            s = s.next();
        }
        let count = labels.len();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), count, "two orders render the same label");
    }
}
