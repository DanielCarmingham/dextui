//! Everything that knows the `dex` CLI exists.
//!
//! Reads and writes both go through the CLI rather than touching `tasks.jsonl`
//! directly, so dex's own validation and its GitHub/Shortcut sync hooks always run.

use std::collections::HashMap;
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    Blocked,
    InProgress,
    Completed,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Completed => "completed",
            Status::InProgress => "in progress",
            Status::Blocked => "blocked",
            Status::Pending => "pending",
        }
    }
}

/// Whether anything is still holding this task up.
///
/// dex never clears `blockedBy` when a blocker finishes, so the list alone says
/// nothing -- the blockers have to be resolved against the rest of the set and
/// checked. This mirrors dex's own `isBlocked` in `core/task-relationships.js`:
/// ids absent from the set are skipped (`t !== undefined` there), as are
/// completed blockers.
///
/// Only *direct* blockers count, exactly as in dex. That also means a blocking
/// cycle -- which dex refuses to create, but a hand-edited store could hold --
/// cannot recurse.
pub fn is_blocked(task: &Task, by_id: &HashMap<String, Task>) -> bool {
    task.blocked_by
        .iter()
        .filter_map(|id| by_id.get(id))
        .any(|blocker| !blocker.completed)
}

/// dex has no status field; it is implied by `completed`, `started_at` and the
/// state of whatever `blockedBy` points at.
///
/// The order is dex's own, from `cli/status.js`: in progress is tested *before*
/// blocked, so a started-but-blocked task reads as in progress. Work is
/// actually happening on it, which is the more useful signal than the fact that
/// something else nominally holds it up.
pub fn status(task: &Task, by_id: &HashMap<String, Task>) -> Status {
    if task.completed {
        Status::Completed
    } else if task.started_at.is_some() {
        Status::InProgress
    } else if is_blocked(task, by_id) {
        Status::Blocked
    } else {
        Status::Pending
    }
}

fn default_priority() -> i64 {
    1
}

/// A git commit linked to a task via `dex complete --commit <sha>`.
///
/// Purely local: this comes from your git repo and needs no sync configured.
#[derive(Debug, Clone, Deserialize)]
pub struct CommitMeta {
    pub sha: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    // dex also sends `url` when the repo has a remote. Not modelled because
    // nothing renders it; serde ignores unknown keys, so adding it is trivial.
}

impl CommitMeta {
    /// The usual 7-character abbreviation.
    pub fn short_sha(&self) -> &str {
        let n = self.sha.len().min(7);
        &self.sha[..n]
    }
}

/// dex also stores `github`, `shortcut` and `beads` blocks here. They are
/// deliberately not modelled: they only appear once sync is configured, and
/// serde ignores unknown keys, so adding them later is additive.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Metadata {
    #[serde(default)]
    pub commit: Option<CommitMeta>,
}

/// One dex task, as emitted by `dex list --json`.
///
/// The wire format mixes conventions: most keys are snake_case, which serde
/// matches on field names directly, but `blockedBy` and `blocks` are camelCase
/// and need an explicit rename.
#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i64,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub result: Option<String>,
    /// Null for most tasks; carries the linked commit when there is one.
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default, rename = "blockedBy")]
    pub blocked_by: Vec<String>,
    /// Tasks this one is blocking -- the reverse of `blocked_by`.
    #[serde(default)]
    pub blocks: Vec<String>,
    #[serde(default)]
    pub children: Vec<String>,
}

impl Default for Task {
    fn default() -> Self {
        Self {
            id: String::new(),
            parent_id: None,
            name: String::new(),
            description: None,
            // Matches serde's default, not i64::default(), so fixtures and real
            // payloads agree on sibling ordering.
            priority: default_priority(),
            completed: false,
            result: None,
            metadata: None,
            created_at: None,
            updated_at: None,
            started_at: None,
            completed_at: None,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            children: Vec::new(),
        }
    }
}

impl Task {
    /// Started and not yet finished. Self-contained, exactly like dex's own
    /// `isInProgress` -- unlike blocked-ness, this needs no view of the set.
    pub fn is_in_progress(&self) -> bool {
        self.started_at.is_some() && !self.completed
    }

    pub fn commit(&self) -> Option<&CommitMeta> {
        self.metadata.as_ref()?.commit.as_ref()
    }

    /// How long the task was actually in flight, if it ran to completion.
    pub fn worked_duration(&self) -> Option<String> {
        span_between(self.started_at.as_deref()?, self.completed_at.as_deref()?)
    }

    /// True only when `updated_at` tells you something the other timestamps do
    /// not. Creating, starting and completing all bump `updated_at`, so without
    /// this the row would simply repeat whichever of those happened last.
    pub fn has_distinct_update(&self) -> bool {
        let Some(updated) = self.updated_at.as_deref() else {
            return false;
        };
        [&self.created_at, &self.started_at, &self.completed_at]
            .iter()
            .all(|other| other.as_deref() != Some(updated))
    }
}

#[derive(Debug, Clone)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// dex writes real diagnostics to stderr; prefer those over an exit code.
    pub fn message(&self, label: &str) -> String {
        let err = self.stderr.trim();
        if !err.is_empty() {
            return err.to_string();
        }
        let out = self.stdout.trim();
        if !out.is_empty() {
            return out.to_string();
        }
        format!("{label} failed (exit {})", self.code)
    }
}

/// Seam so the argv built for every verb can be asserted without running dex.
pub trait Runner: Send + Sync {
    fn run(&self, args: &[String]) -> Result<Output, String>;
}

pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, args: &[String]) -> Result<Output, String> {
        // `.args()` passes argv straight through with no shell involved, so task
        // names containing quotes, ampersands or newlines survive verbatim.
        let out = Command::new("dex").args(args).output().map_err(|e| {
            // Distinguished because the remedies differ: one is "install it",
            // the other is "it is installed and cannot start".
            if e.kind() == std::io::ErrorKind::NotFound {
                why_not_found(&std::env::var("PATH").unwrap_or_default())
            } else {
                format!("could not run `dex`: {e}")
            }
        })?;

        Ok(Output {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Where to get dex, for the one message that has to explain the dependency.
pub const HOME: &str = "https://dex.rip/";

/// Where `name` resolves on `path`, if anywhere. `path` is a `PATH`-style
/// colon-separated list.
///
/// Needed because `ErrorKind::NotFound` from an exec is **ambiguous**: it means
/// "nothing to run" whether the binary is absent *or* present with a `#!` line
/// pointing at an interpreter that is absent. dex is a Node script, so the
/// second is a real case -- a node upgrade moving the runtime out from under it
/// produces a `dex` sitting on the PATH that cannot start. Telling someone to
/// install a thing they can see with `which` sends them the wrong way.
fn lookup(name: &str, path: &str) -> Option<std::path::PathBuf> {
    path.split(':')
        .filter(|dir| !dir.is_empty())
        .map(|dir| std::path::Path::new(dir).join(name))
        .find(|p| p.is_file())
}

/// Why `dex` could not be started, said as precisely as the evidence allows.
fn why_not_found(path: &str) -> String {
    match lookup("dex", path) {
        Some(p) => format!(
            "`dex` is at {} but could not be started -- its interpreter is \
             probably missing (check the `#!` line)",
            p.display()
        ),
        None => "`dex` was not found on your PATH".to_string(),
    }
}

/// What to print when dex cannot be run at all.
///
/// dextui is a front end and nothing else -- every read and every write is a
/// `dex` call -- so this is a hard stop rather than a degraded mode, and the
/// message has to say what to install and where from.
///
/// It deliberately does **not** guess at PATH. `dex` being absent and `dex`
/// being present but failing produce very different errors, and telling someone
/// to check their PATH when the binary is sitting on it -- because, say, a node
/// upgrade moved the runtime out from under it -- sends them the wrong way. The
/// underlying error is shown instead, and it is left to say which happened.
pub fn requires_dex(err: &str) -> String {
    // Node stack traces run to dozens of lines, and the *first* is the least
    // useful of them -- `node:internal/modules/cjs/loader:1520` says nothing a
    // reader can act on, while the `Error:` line below it names the missing
    // module. Prefer that line, and fall back to the first only if there is none.
    let cause = err
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Error:") || l.starts_with("error:"))
        .or_else(|| err.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or(err);
    format!(
        "dextui: dex is required, but could not be run.\n\n    {cause}\n\n\
         dextui is a viewer for dex -- it reads and writes tasks only through\n\
         the dex CLI, so there is nothing it can show without it.\n\n\
         Install it from {HOME} and check that `dex --version` works."
    )
}

pub struct Dex {
    runner: Box<dyn Runner>,
    /// The store every call targets, or `None` to let dex resolve it from the
    /// working directory.
    ///
    /// Held here rather than passed per call because a verb that forgets the
    /// flag writes to a different project's task list, silently. One field and
    /// one argv builder means there is exactly one place that can be wrong.
    store: Option<String>,
}

impl std::fmt::Debug for Dex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dex")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

fn s(v: &str) -> String {
    v.to_string()
}

impl Dex {
    pub fn new(runner: Box<dyn Runner>) -> Self {
        Self { runner, store: None }
    }

    pub fn real() -> Self {
        Self::new(Box::new(ProcessRunner))
    }

    /// Targets a specific store directory.
    ///
    /// Rejects a path ending in `.jsonl` because dex accepts it, finds no tasks,
    /// and returns an empty list rather than an error -- a wrong store would
    /// look like an empty project.
    pub fn for_store(store_dir: &str) -> Result<Self, String> {
        Self::for_store_with(store_dir, Box::new(ProcessRunner))
    }

    pub fn for_store_with(store_dir: &str, runner: Box<dyn Runner>) -> Result<Self, String> {
        if store_dir.ends_with(".jsonl") {
            return Err(format!(
                "store path must be the .dex directory, not a file: {store_dir}"
            ));
        }
        Ok(Self {
            runner,
            store: Some(store_dir.to_string()),
        })
    }

    /// The full argv for a call, with the global option ahead of the verb.
    fn argv(&self, rest: &[String]) -> Vec<String> {
        match &self.store {
            Some(dir) => {
                let mut v = vec![s("--storage-path"), dir.clone()];
                v.extend_from_slice(rest);
                v
            }
            None => rest.to_vec(),
        }
    }

    /// Resolves the store dex is actually using.
    ///
    /// This is NOT always `./.dex`: outside a git repo dex falls back to a global
    /// store under `~/.config/dex`, so the watcher must follow whatever this says.
    pub fn store_dir(&self) -> Result<String, String> {
        let out = self.runner.run(&self.argv(&[s("dir")]))?;
        if out.ok() {
            Ok(out.stdout.trim().to_string())
        } else {
            Err(out.message("dex dir"))
        }
    }

    /// Always fetches `--all`; status filtering happens in memory so that
    /// changing the filter is instant and costs no process spawn.
    pub fn list(&self) -> Result<Vec<Task>, String> {
        let out = self
            .runner
            .run(&self.argv(&[s("list"), s("--json"), s("--all")]))?;

        if !out.ok() {
            return Err(out.message("dex list"));
        }

        serde_json::from_str(&out.stdout).map_err(|e| format!("could not parse dex output: {e}"))
    }

    pub fn start(&self, id: &str) -> Result<(), String> {
        self.void(&self.argv(&[s("start"), s(id)]), "dex start")
    }

    /// `--no-commit` is always sent: for tasks synced to GitHub, dex refuses to
    /// complete without either `--commit` or `--no-commit`, and a TUI has no way
    /// to answer that prompt. `force` bypasses the incomplete-subtask check.
    pub fn complete(&self, id: &str, result: &str, force: bool) -> Result<(), String> {
        let mut args = vec![
            s("complete"),
            s(id),
            s("--result"),
            s(result),
            s("--no-commit"),
        ];
        if force {
            args.push(s("--force"));
        }
        self.void(&self.argv(&args), "dex complete")
    }

    pub fn create(&self, name: &str, description: &str, parent: Option<&str>) -> Result<(), String> {
        let mut args = vec![s("create"), s(name)];
        if !description.trim().is_empty() {
            args.push(s("--description"));
            args.push(s(description));
        }
        if let Some(p) = parent {
            args.push(s("--parent"));
            args.push(s(p));
        }
        self.void(&self.argv(&args), "dex create")
    }

    pub fn edit(&self, id: &str, name: Option<&str>, description: Option<&str>) -> Result<(), String> {
        let mut args = vec![s("edit"), s(id)];
        if let Some(n) = name {
            args.push(s("--name"));
            args.push(s(n));
        }
        if let Some(d) = description {
            args.push(s("--description"));
            args.push(s(d));
        }
        self.void(&self.argv(&args), "dex edit")
    }

    /// Always forced: dex prompts interactively when subtasks exist, which would
    /// hang a TUI that has no way to answer.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.void(&self.argv(&[s("delete"), s(id), s("--force")]), "dex delete")
    }

    fn void(&self, args: &[String], label: &str) -> Result<(), String> {
        let out = self.runner.run(args)?;
        if out.ok() {
            Ok(())
        } else {
            Err(out.message(label))
        }
    }
}

/// Whether this is dex's shared fallback store rather than a project's own.
///
/// Outside a git repo dex writes to `~/.config/dex/local`, so the two are told
/// apart by the path alone -- never by the label, which is only a directory
/// name and would call a project literally named `global` the global store.
pub fn is_global_store(store_dir: &str) -> bool {
    let trimmed = store_dir.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    name != ".dex" && trimmed.contains(".config/dex")
}

/// A human label for the store: either the project directory that owns the
/// `.dex` folder, or "global" for the shared fallback store.
pub fn store_label(store_dir: &str) -> String {
    let trimmed = store_dir.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);

    if name == ".dex" {
        let parent = trimmed
            .trim_end_matches(".dex")
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("");
        if parent.is_empty() {
            return ".dex".to_string();
        }
        return parent.to_string();
    }

    if is_global_store(trimmed) {
        return "global".to_string();
    }

    name.to_string()
}

/// Formats an ISO-8601 timestamp from dex as local `yyyy-MM-dd HH:mm`.
pub fn local_time(iso: &Option<String>) -> String {
    let Some(raw) = iso else {
        return "-".to_string();
    };

    match chrono::DateTime::parse_from_rfc3339(raw) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        // Never lose the value just because the shape was unexpected.
        Err(_) => raw.clone(),
    }
}

/// Compresses a duration to a short label: `12m`, `4h`, `2d`, `3w`.
///
/// Split out from the clock so it can be tested directly.
pub fn humanize_secs(secs: i64) -> String {
    let secs = secs.max(0);
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;

    if secs < MIN {
        "now".to_string()
    } else if secs < HOUR {
        format!("{}m", secs / MIN)
    } else if secs < DAY {
        format!("{}h", secs / HOUR)
    } else if secs < WEEK {
        format!("{}d", secs / DAY)
    } else {
        format!("{}w", secs / WEEK)
    }
}

/// A duration as up to two units: `45s`, `5m 30s`, `4h 12m`, `3d 5h`, `2w 3d`.
///
/// Deliberately more precise than `humanize_secs`, which shows one unit because
/// it has to fit a narrow gutter. Here there is room, and "4h 12m" answers
/// "how long did this take" better than "4h".
pub fn humanize_span(secs: i64) -> String {
    let secs = secs.max(0);
    const MIN: i64 = 60;
    const HOUR: i64 = 60 * MIN;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;

    let (major, major_unit, minor_unit) = if secs < MIN {
        return format!("{secs}s");
    } else if secs < HOUR {
        (MIN, "m", "s")
    } else if secs < DAY {
        (HOUR, "h", "m")
    } else if secs < WEEK {
        (DAY, "d", "h")
    } else {
        (WEEK, "w", "d")
    };

    let whole = secs / major;
    let minor_size = match major_unit {
        "m" => 1,
        "h" => MIN,
        "d" => HOUR,
        _ => DAY,
    };
    let rest = (secs % major) / minor_size;

    if rest == 0 {
        format!("{whole}{major_unit}")
    } else {
        format!("{whole}{major_unit} {rest}{minor_unit}")
    }
}

/// Elapsed time between two ISO-8601 stamps, or None if either will not parse.
pub fn span_between(start: &str, end: &str) -> Option<String> {
    let a = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let b = chrono::DateTime::parse_from_rfc3339(end).ok()?;
    Some(humanize_span(b.signed_duration_since(a).num_seconds()))
}

/// How long ago an ISO-8601 timestamp was, or None if absent or unparseable.
pub fn age(iso: &Option<String>) -> Option<String> {
    let raw = iso.as_ref()?;
    let then = chrono::DateTime::parse_from_rfc3339(raw).ok()?;
    let secs = chrono::Utc::now()
        .signed_duration_since(then.with_timezone(&chrono::Utc))
        .num_seconds();
    Some(humanize_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Records the argv it was handed and replays canned output.
    #[derive(Clone)]
    struct Fake {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        out: Output,
    }

    impl Fake {
        fn new(stdout: &str, stderr: &str, code: i32) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                out: Output {
                    code,
                    stdout: stdout.to_string(),
                    stderr: stderr.to_string(),
                },
            }
        }

        fn last(&self) -> Vec<String> {
            self.calls.lock().unwrap().last().cloned().unwrap_or_default()
        }
    }

    impl Runner for Fake {
        fn run(&self, args: &[String]) -> Result<Output, String> {
            self.calls.lock().unwrap().push(args.to_vec());
            Ok(self.out.clone())
        }
    }

    fn dex_with(fake: &Fake) -> Dex {
        Dex::new(Box::new(fake.clone()))
    }

    /// Captured verbatim from a real `dex list --json --all`.
    const REAL_JSON: &str = r#"[
      {
        "id": "s7rngopd",
        "parent_id": "b4d5gfpl",
        "name": "Handle \"quoted\" names & $pecial chars",
        "description": "Line one\nLine two with a  double space",
        "priority": 1,
        "completed": false,
        "result": null,
        "metadata": null,
        "created_at": "2026-07-27T01:47:19.253Z",
        "started_at": null,
        "completed_at": null,
        "blockedBy": [],
        "blocks": [],
        "children": []
      },
      {
        "id": "uqolu0wq",
        "parent_id": "b4d5gfpl",
        "name": "Pick a toolkit",
        "description": "Evaluate options",
        "priority": 2,
        "completed": true,
        "result": "Chose ratatui",
        "metadata": null,
        "created_at": "2026-07-27T01:47:19.093Z",
        "started_at": "2026-07-27T01:47:26.225Z",
        "completed_at": "2026-07-27T01:47:26.371Z",
        "blockedBy": ["s7rngopd"],
        "blocks": [],
        "children": []
      }
    ]"#;

    #[test]
    fn parses_the_mixed_case_wire_format() {
        let fake = Fake::new(REAL_JSON, "", 0);
        let tasks = dex_with(&fake).list().unwrap();

        assert_eq!(tasks.len(), 2);
        // snake_case key...
        assert_eq!(tasks[0].parent_id.as_deref(), Some("b4d5gfpl"));
        // ...and a camelCase key in the very same payload.
        assert_eq!(tasks[1].blocked_by, vec!["s7rngopd"]);
    }

    #[test]
    fn preserves_quotes_and_newlines_in_task_text() {
        let fake = Fake::new(REAL_JSON, "", 0);
        let tasks = dex_with(&fake).list().unwrap();

        assert_eq!(tasks[0].name, "Handle \"quoted\" names & $pecial chars");
        assert_eq!(
            tasks[0].description.as_deref(),
            Some("Line one\nLine two with a  double space")
        );
    }

    #[test]
    fn status_is_derived_from_completed_and_started_at() {
        let fake = Fake::new(REAL_JSON, "", 0);
        let tasks = dex_with(&fake).list().unwrap();
        let all = set(tasks.clone());

        assert_eq!(status(&tasks[0], &all), Status::Pending);
        assert_eq!(status(&tasks[1], &all), Status::Completed);
    }

    /// Builds a set keyed by id, the way `App` holds it.
    fn set(tasks: Vec<Task>) -> HashMap<String, Task> {
        tasks.into_iter().map(|t| (t.id.clone(), t)).collect()
    }

    fn t(id: &str) -> Task {
        Task {
            id: id.into(),
            name: id.into(),
            ..Default::default()
        }
    }

    fn blocked_on(id: &str, blockers: &[&str]) -> Task {
        Task {
            blocked_by: blockers.iter().map(|s| s.to_string()).collect(),
            ..t(id)
        }
    }

    /// The rule is dex's own `isBlocked` (core/task-relationships.js): resolve
    /// `blockedBy`, drop ids that are absent from the set, drop blockers that
    /// are completed, and report blocked if any remain.
    #[test]
    fn an_incomplete_blocker_blocks() {
        let all = set(vec![t("blocker"), blocked_on("victim", &["blocker"])]);
        assert!(is_blocked(&all["victim"], &all));
    }

    /// The reason `blocked_by.is_empty()` was not good enough: dex never clears
    /// `blockedBy` when a blocker finishes, so a task would read as blocked for
    /// the rest of its life.
    #[test]
    fn a_completed_blocker_stops_blocking() {
        let mut blocker = t("blocker");
        blocker.completed = true;
        let all = set(vec![blocker, blocked_on("victim", &["blocker"])]);
        assert!(!is_blocked(&all["victim"], &all));
    }

    /// A dangling reference is not a blocker. dex filters these out with
    /// `t !== undefined` for the same reason.
    #[test]
    fn a_blocker_missing_from_the_set_does_not_block() {
        let all = set(vec![blocked_on("victim", &["ghost"])]);
        assert!(!is_blocked(&all["victim"], &all));
    }

    #[test]
    fn one_incomplete_blocker_is_enough_among_several() {
        let mut done = t("done");
        done.completed = true;
        let all = set(vec![done, t("open"), blocked_on("victim", &["done", "open"])]);
        assert!(is_blocked(&all["victim"], &all));
    }

    /// Precedence matches dex's own `status.js`, which tests in-progress first:
    /// work is actually happening on the task, which is the more useful signal
    /// than the fact that something else is nominally holding it up.
    #[test]
    fn a_started_task_reads_as_in_progress_even_when_blocked() {
        let mut victim = blocked_on("victim", &["blocker"]);
        victim.started_at = Some("2026-01-01T00:00:00Z".into());
        let all = set(vec![t("blocker"), victim]);
        assert_eq!(status(&all["victim"], &all), Status::InProgress);
    }

    #[test]
    fn completed_wins_over_every_other_signal() {
        let mut victim = blocked_on("victim", &["blocker"]);
        victim.started_at = Some("2026-01-01T00:00:00Z".into());
        victim.completed = true;
        let all = set(vec![t("blocker"), victim]);
        assert_eq!(status(&all["victim"], &all), Status::Completed);
    }

    #[test]
    fn an_unstarted_task_with_a_live_blocker_is_blocked() {
        let all = set(vec![t("blocker"), blocked_on("victim", &["blocker"])]);
        assert_eq!(status(&all["victim"], &all), Status::Blocked);
    }

    #[test]
    fn a_task_with_nothing_holding_it_up_is_pending() {
        let all = set(vec![t("lonely")]);
        assert_eq!(status(&all["lonely"], &all), Status::Pending);
    }

    /// dex refuses to create blocking cycles, but a hand-edited store could
    /// still contain one. The derivation looks only at direct blockers, so a
    /// cycle cannot recurse -- this pins that down.
    #[test]
    fn a_blocking_cycle_terminates() {
        let all = set(vec![
            blocked_on("a", &["b"]),
            blocked_on("b", &["a"]),
        ]);
        assert_eq!(status(&all["a"], &all), Status::Blocked);
        assert_eq!(status(&all["b"], &all), Status::Blocked);
    }

    #[test]
    fn list_always_requests_all_so_filtering_stays_client_side() {
        let fake = Fake::new("[]", "", 0);
        dex_with(&fake).list().unwrap();

        assert_eq!(fake.last(), vec!["list", "--json", "--all"]);
    }

    #[test]
    fn complete_passes_text_as_one_argv_entry() {
        let fake = Fake::new("", "", 0);
        let nasty = "done: \"quoted\" & $HOME\nsecond line";
        dex_with(&fake).complete("abc123", nasty, false).unwrap();

        // The dangerous text must arrive as a single unmangled argument.
        assert!(fake.last().contains(&nasty.to_string()));
        assert_eq!(
            fake.last(),
            vec!["complete", "abc123", "--result", nasty, "--no-commit"]
        );
    }

    #[test]
    fn complete_adds_force_only_when_asked() {
        let fake = Fake::new("", "", 0);
        let dex = dex_with(&fake);

        dex.complete("abc", "done", false).unwrap();
        assert!(!fake.last().contains(&"--force".to_string()));

        dex.complete("abc", "done", true).unwrap();
        assert!(fake.last().contains(&"--force".to_string()));
    }

    #[test]
    fn delete_always_forces_because_dex_would_otherwise_prompt() {
        let fake = Fake::new("", "", 0);
        dex_with(&fake).delete("abc123").unwrap();

        // An interactive prompt would hang a TUI with no way to answer it.
        assert_eq!(fake.last(), vec!["delete", "abc123", "--force"]);
    }

    #[test]
    fn create_omits_optional_flags_when_not_supplied() {
        let fake = Fake::new("", "", 0);
        dex_with(&fake).create("Just a name", "", None).unwrap();

        assert_eq!(fake.last(), vec!["create", "Just a name"]);
    }

    #[test]
    fn create_includes_parent_for_subtasks() {
        let fake = Fake::new("", "", 0);
        dex_with(&fake)
            .create("Child", "details", Some("parent1"))
            .unwrap();

        assert_eq!(
            fake.last(),
            vec!["create", "Child", "--description", "details", "--parent", "parent1"]
        );
    }

    #[test]
    fn edit_sends_only_the_fields_supplied() {
        let fake = Fake::new("", "", 0);
        dex_with(&fake).edit("abc123", Some("New name"), None).unwrap();

        assert_eq!(fake.last(), vec!["edit", "abc123", "--name", "New name"]);
    }

    #[test]
    fn failures_surface_stderr_rather_than_an_exit_code() {
        let fake = Fake::new("", "Task has 2 incomplete subtasks. Use --force.", 1);
        let err = dex_with(&fake).complete("abc", "done", false).unwrap_err();

        assert!(err.contains("incomplete subtasks"));
    }

    #[test]
    fn malformed_json_is_reported_rather_than_panicking() {
        let fake = Fake::new("this is not json", "", 0);
        let err = dex_with(&fake).list().unwrap_err();

        assert!(err.contains("could not parse"));
    }

    #[test]
    fn store_dir_is_trimmed() {
        // Must be honoured rather than assuming ./.dex -- outside a git repo dex
        // uses a global store under ~/.config/dex.
        let fake = Fake::new("/Users/x/.config/dex/local\n", "", 0);
        assert_eq!(dex_with(&fake).store_dir().unwrap(), "/Users/x/.config/dex/local");
    }

    /// The whole point is that someone who has never heard of dex can act on
    /// this, so the two facts that make it actionable are pinned.
    #[test]
    fn the_missing_dex_message_says_what_to_install_and_where_from() {
        let m = requires_dex("`dex` was not found on your PATH");
        assert!(m.contains(HOME), "no link to install from: {m}");
        assert!(m.contains("dex is required"), "does not say it is required: {m}");
        assert!(m.contains("not found on your PATH"), "loses the cause: {m}");
    }

    /// A dex that is installed but broken -- a node upgrade moving the runtime
    /// out from under it, say -- prints a stack trace dozens of lines long, and
    /// its **first** line is the least useful of them. This is the real stderr
    /// from exactly that failure.
    #[test]
    fn a_stack_trace_is_reduced_to_the_line_that_names_the_cause() {
        let trace = "node:internal/modules/cjs/loader:1520\n  throw err;\n  ^\n\n\
                     Error: Cannot find module '/x/fnm/aliases/default/bin/dex'\n\
                     \x20   at Module._resolveFilename (node:internal/modules/cjs/loader:1517:15)\n\
                     \x20   at wrapResolveFilename (node:internal/modules/cjs/loader:1071:27)";
        let m = requires_dex(trace);

        assert!(m.contains("Cannot find module"), "lost the cause: {m}");
        assert!(
            !m.contains("cjs/loader:1520"),
            "led with the least useful line: {m}"
        );
        assert!(!m.contains("at Module._resolveFilename"), "kept the noise: {m}");
        assert!(m.contains(HOME));
    }

    /// It must not tell someone to check their PATH when the binary is on it.
    /// That was the old message, and it sends a broken install the wrong way.
    #[test]
    fn the_message_does_not_guess_at_the_cause() {
        let m = requires_dex("Error: Cannot find module '/x/y/bin/dex'");
        assert!(
            !m.to_lowercase().contains("path"),
            "guessed at PATH when the cause says otherwise: {m}"
        );
    }

    /// `ErrorKind::NotFound` from an exec cannot tell "no such binary" from
    /// "binary present, interpreter missing" -- and dex is a Node script, so the
    /// second is real. Resolving the name ourselves is what separates them.
    #[test]
    fn a_present_but_unstartable_dex_is_not_reported_as_missing() {
        let dir = std::env::temp_dir().join("dextui-lookup-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dex = dir.join("dex");
        std::fs::write(&dex, "#!/nonexistent/node\n").unwrap();

        let msg = why_not_found(dir.to_str().unwrap());
        assert!(
            msg.contains("could not be started"),
            "did not recognise a present dex: {msg}"
        );
        assert!(msg.contains("interpreter"), "no actionable cause: {msg}");
        assert!(
            !msg.contains("not found on your PATH"),
            "still claims it is missing: {msg}"
        );
        assert!(msg.contains(dex.to_str().unwrap()), "does not say where: {msg}");

        std::fs::remove_file(&dex).ok();
    }

    /// The genuinely-absent case must still say so plainly.
    #[test]
    fn an_absent_dex_is_reported_as_missing() {
        let msg = why_not_found("/nonexistent/a:/nonexistent/b");
        assert_eq!(msg, "`dex` was not found on your PATH");
    }

    /// An empty or unset PATH must not panic or claim to have found something.
    #[test]
    fn an_empty_path_is_not_a_match() {
        assert_eq!(lookup("dex", ""), None);
        assert_eq!(lookup("dex", ":::"), None);
    }

    #[test]
    fn store_label_names_the_project_or_global() {
        assert_eq!(store_label("/Users/x/Developer/myproj/.dex"), "myproj");
        assert_eq!(store_label("/Users/x/.config/dex/local"), "global");
        assert!(is_global_store("/Users/x/.config/dex/local"));
        // A directory name is not evidence: this one is a project's own store.
        assert!(!is_global_store("/Users/x/Developer/global/.dex"));
        assert_eq!(store_label("/Users/x/Developer/global/.dex"), "global");
    }

    #[test]
    fn humanize_secs_picks_a_sensible_unit() {
        assert_eq!(humanize_secs(0), "now");
        assert_eq!(humanize_secs(59), "now");
        assert_eq!(humanize_secs(60), "1m");
        assert_eq!(humanize_secs(59 * 60), "59m");
        assert_eq!(humanize_secs(60 * 60), "1h");
        assert_eq!(humanize_secs(23 * 3600), "23h");
        assert_eq!(humanize_secs(24 * 3600), "1d");
        assert_eq!(humanize_secs(6 * 86400), "6d");
        assert_eq!(humanize_secs(7 * 86400), "1w");
        assert_eq!(humanize_secs(30 * 86400), "4w");
    }

    #[test]
    fn humanize_secs_never_renders_a_negative_age() {
        // Clock skew between machines writing the store must not print "-3h".
        assert_eq!(humanize_secs(-500), "now");
    }

    #[test]
    fn age_of_an_unparseable_or_absent_stamp_is_none() {
        assert_eq!(age(&None), None);
        assert_eq!(age(&Some("not a date".into())), None);
    }

    /// Captured verbatim from a task completed with `dex complete --commit`.
    const WITH_COMMIT: &str = r#"[{
        "id": "x62ncnp9",
        "parent_id": null,
        "name": "Task with a linked commit",
        "description": "probe",
        "priority": 1,
        "completed": true,
        "result": "Implemented it.",
        "metadata": {"commit": {
            "sha": "8f7c1015d414e18c5c071d3b1c0d856096e34f6c",
            "message": "Add the file",
            "branch": "main",
            "timestamp": "2026-07-28T14:28:16.044Z"
        }},
        "created_at": "2026-07-28T14:28:14.729Z",
        "updated_at": "2026-07-28T14:28:16.044Z",
        "started_at": "2026-07-28T14:28:14.858Z",
        "completed_at": "2026-07-28T14:28:16.044Z",
        "blockedBy": [],
        "blocks": ["abc123"],
        "children": []
    }]"#;

    #[test]
    fn commit_metadata_is_parsed() {
        let fake = Fake::new(WITH_COMMIT, "", 0);
        let tasks = dex_with(&fake).list().unwrap();
        let c = tasks[0].commit().expect("commit metadata");

        assert_eq!(c.sha, "8f7c1015d414e18c5c071d3b1c0d856096e34f6c");
        assert_eq!(c.short_sha(), "8f7c101");
        assert_eq!(c.message.as_deref(), Some("Add the file"));
        assert_eq!(c.branch.as_deref(), Some("main"));
    }

    #[test]
    fn unknown_metadata_blocks_are_ignored_not_fatal() {
        // github/shortcut/beads appear once sync is configured. We do not model
        // them yet, and their presence must never break parsing.
        let json = r#"[{"id":"a","name":"n","created_at":"2026-01-01T00:00:00Z",
            "metadata":{"github":{"issueNumber":42,"issueUrl":"https://x/1","repo":"o/r"},
                        "commit":{"sha":"deadbeefcafe"}}}]"#;
        let fake = Fake::new(json, "", 0);
        let tasks = dex_with(&fake).list().unwrap();

        assert_eq!(tasks[0].commit().unwrap().short_sha(), "deadbee");
    }

    #[test]
    fn blocks_is_parsed_separately_from_blocked_by() {
        let fake = Fake::new(WITH_COMMIT, "", 0);
        let tasks = dex_with(&fake).list().unwrap();

        assert_eq!(tasks[0].blocks, vec!["abc123"]);
        assert!(tasks[0].blocked_by.is_empty());
    }

    #[test]
    fn worked_duration_is_start_to_completion() {
        let fake = Fake::new(WITH_COMMIT, "", 0);
        let tasks = dex_with(&fake).list().unwrap();
        // 14:28:14.858 -> 14:28:16.044 is just over a second.
        assert_eq!(tasks[0].worked_duration().as_deref(), Some("1s"));
    }

    #[test]
    fn worked_duration_needs_both_ends() {
        let mut t = Task {
            started_at: Some("2026-01-01T00:00:00Z".into()),
            ..Default::default()
        };
        assert_eq!(t.worked_duration(), None, "never completed");

        t.started_at = None;
        t.completed_at = Some("2026-01-01T01:00:00Z".into());
        assert_eq!(t.worked_duration(), None, "never started");
    }

    #[test]
    fn updated_row_is_hidden_when_it_echoes_another_timestamp() {
        let untouched = Task {
            created_at: Some("2026-01-01T00:00:00Z".into()),
            updated_at: Some("2026-01-01T00:00:00Z".into()),
            ..Default::default()
        };
        assert!(!untouched.has_distinct_update(), "same as created");

        // Completing bumps updated_at, so the two match and the row is noise.
        let completed = Task {
            completed_at: Some("2026-01-02T00:00:00Z".into()),
            updated_at: Some("2026-01-02T00:00:00Z".into()),
            ..untouched.clone()
        };
        assert!(!completed.has_distinct_update(), "same as done");

        let started = Task {
            started_at: Some("2026-01-03T00:00:00Z".into()),
            updated_at: Some("2026-01-03T00:00:00Z".into()),
            ..untouched.clone()
        };
        assert!(!started.has_distinct_update(), "same as started");
    }

    #[test]
    fn updated_row_shows_for_a_genuine_edit() {
        let edited = Task {
            created_at: Some("2026-01-01T00:00:00Z".into()),
            started_at: Some("2026-01-02T00:00:00Z".into()),
            updated_at: Some("2026-01-05T00:00:00Z".into()),
            ..Default::default()
        };
        assert!(edited.has_distinct_update());
    }

    #[test]
    fn humanize_span_uses_up_to_two_units() {
        assert_eq!(humanize_span(0), "0s");
        assert_eq!(humanize_span(45), "45s");
        assert_eq!(humanize_span(60), "1m");
        assert_eq!(humanize_span(330), "5m 30s");
        assert_eq!(humanize_span(3600), "1h");
        assert_eq!(humanize_span(4 * 3600 + 12 * 60), "4h 12m");
        assert_eq!(humanize_span(86400), "1d");
        assert_eq!(humanize_span(3 * 86400 + 5 * 3600), "3d 5h");
        assert_eq!(humanize_span(7 * 86400), "1w");
        assert_eq!(humanize_span(17 * 86400), "2w 3d");
    }

    #[test]
    fn humanize_span_never_goes_negative() {
        // Clock skew between machines writing the store must not print "-3h".
        assert_eq!(humanize_span(-9999), "0s");
    }

    #[test]
    fn span_between_rejects_unparseable_stamps() {
        assert_eq!(span_between("nonsense", "2026-01-01T00:00:00Z"), None);
    }

    #[test]
    fn short_sha_handles_an_already_short_value() {
        let c = CommitMeta {
            sha: "abc".into(),
            message: None,
            branch: None,
        };
        assert_eq!(c.short_sha(), "abc");
    }

    /// The whole point of `for_store`: a verb cannot forget the flag, because no
    /// verb builds its own argv any more.
    #[test]
    fn every_verb_carries_the_storage_path() {
        let fake = Fake::new("[]", "", 0);
        let dex = Dex::for_store_with("/tmp/x/.dex", Box::new(fake.clone())).unwrap();

        let _ = dex.list();
        assert_eq!(
            &fake.last()[..2],
            &["--storage-path".to_string(), "/tmp/x/.dex".to_string()],
            "list lost the store: {:?}",
            fake.last()
        );

        let _ = dex.start("abc");
        assert_eq!(
            &fake.last()[..2],
            &["--storage-path".to_string(), "/tmp/x/.dex".to_string()],
            "start lost the store: {:?}",
            fake.last()
        );
    }

    /// dex puts global options before the command, so the flag has to lead.
    #[test]
    fn the_storage_path_precedes_the_verb() {
        let fake = Fake::new("[]", "", 0);
        let dex = Dex::for_store_with("/tmp/x/.dex", Box::new(fake.clone())).unwrap();
        let _ = dex.list();

        let argv = fake.last();
        assert_eq!(argv[0], "--storage-path");
        assert_eq!(argv[2], "list", "the verb must follow the global option");
    }

    /// `--storage-path` pointed at tasks.jsonl returns an empty list rather than an
    /// error, so a wrong path here is silent. Reject it where it can still be seen.
    #[test]
    fn a_store_path_must_be_the_directory_not_the_file() {
        let err = Dex::for_store_with("/tmp/x/.dex/tasks.jsonl", Box::new(Fake::new("", "", 0)))
            .unwrap_err();
        assert!(err.contains("directory"), "unhelpful message: {err}");
    }

    /// The cwd-resolving constructor must stay flagless, or every existing call
    /// would start targeting a store it did not ask for.
    #[test]
    fn the_default_dex_passes_no_storage_path() {
        let fake = Fake::new("[]", "", 0);
        let dex = Dex::new(Box::new(fake.clone()));
        let _ = dex.list();
        assert_eq!(fake.last()[0], "list", "unexpected flag: {:?}", fake.last());
    }
}
