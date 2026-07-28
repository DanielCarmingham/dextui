//! Turns dex's flat task array into a hierarchy, with search and status filtering.

use std::collections::{HashMap, HashSet};

use crate::dex::{Status, Task};

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
pub fn build(tasks: &[Task], query: &str, filter: Filter) -> Vec<Node> {
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

    sort(&mut roots);

    let mut visiting: HashSet<&str> = HashSet::new();
    roots
        .into_iter()
        .filter_map(|r| build_node(r, &by_parent, query, filter, &mut visiting))
        .collect()
}

fn build_node<'a>(
    task: &'a Task,
    by_parent: &HashMap<&'a str, Vec<&'a Task>>,
    query: Option<&str>,
    filter: Filter,
    visiting: &mut HashSet<&'a str>,
) -> Option<Node> {
    // Guards against a malformed store where parent links form a cycle.
    if !visiting.insert(task.id.as_str()) {
        return None;
    }

    let mut children = Vec::new();
    if let Some(kids) = by_parent.get(task.id.as_str()) {
        let mut kids = kids.clone();
        sort(&mut kids);
        children = kids
            .into_iter()
            .filter_map(|k| build_node(k, by_parent, query, filter, visiting))
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

fn sort(tasks: &mut [&Task]) {
    tasks.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

fn matches(t: &Task, query: Option<&str>, filter: Filter) -> bool {
    let status_ok = match filter {
        Filter::All => true,
        Filter::Pending => !t.completed,
        Filter::InProgress => t.status() == Status::InProgress,
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

/// One rendered line: the node plus the tree-drawing prefix for its position.
/// Indentation is baked into `prefix`, so no separate depth is needed.
pub struct Row<'a> {
    pub node: &'a Node,
    pub prefix: String,
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

        let marker = if !has_kids {
            "─"
        } else if is_open {
            "▾"
        } else {
            "▸"
        };

        let branch = if last { "└" } else { "├" };
        out.push(Row {
            node,
            prefix: format!("{indent}{branch}{marker} "),
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
            description: None,
            priority: 1,
            completed: false,
            result: None,
            created_at: Some("2026-01-01T00:00:00Z".to_string()),
            started_at: None,
            completed_at: None,
            blocked_by: vec![],
            children: vec![],
        }
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
}
