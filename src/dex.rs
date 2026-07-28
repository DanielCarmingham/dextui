//! Everything that knows the `dex` CLI exists.
//!
//! Reads and writes both go through the CLI rather than touching `tasks.jsonl`
//! directly, so dex's own validation and its GitHub/Shortcut sync hooks always run.

use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Pending,
    InProgress,
    Completed,
}

impl Status {
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Completed => "✓",
            Status::InProgress => "◐",
            Status::Pending => "○",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Completed => "completed",
            Status::InProgress => "in progress",
            Status::Pending => "pending",
        }
    }
}

fn default_priority() -> i64 {
    1
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
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default, rename = "blockedBy")]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub children: Vec<String>,
}

impl Task {
    /// dex has no status field; it is implied by `completed` and `started_at`.
    pub fn status(&self) -> Status {
        if self.completed {
            Status::Completed
        } else if self.started_at.is_some() {
            Status::InProgress
        } else {
            Status::Pending
        }
    }

    pub fn is_blocked(&self) -> bool {
        !self.blocked_by.is_empty()
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
        let out = Command::new("dex")
            .args(args)
            .output()
            .map_err(|e| format!("could not run `dex`: {e}"))?;

        Ok(Output {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

pub struct Dex {
    runner: Box<dyn Runner>,
}

fn s(v: &str) -> String {
    v.to_string()
}

impl Dex {
    pub fn new(runner: Box<dyn Runner>) -> Self {
        Self { runner }
    }

    pub fn real() -> Self {
        Self::new(Box::new(ProcessRunner))
    }

    /// Resolves the store dex is actually using.
    ///
    /// This is NOT always `./.dex`: outside a git repo dex falls back to a global
    /// store under `~/.config/dex`, so the watcher must follow whatever this says.
    pub fn store_dir(&self) -> Result<String, String> {
        let out = self.runner.run(&[s("dir")])?;
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
            .run(&[s("list"), s("--json"), s("--all")])?;

        if !out.ok() {
            return Err(out.message("dex list"));
        }

        serde_json::from_str(&out.stdout).map_err(|e| format!("could not parse dex output: {e}"))
    }

    pub fn start(&self, id: &str) -> Result<(), String> {
        self.void(&[s("start"), s(id)], "dex start")
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
        self.void(&args, "dex complete")
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
        self.void(&args, "dex create")
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
        self.void(&args, "dex edit")
    }

    /// Always forced: dex prompts interactively when subtasks exist, which would
    /// hang a TUI that has no way to answer.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.void(&[s("delete"), s(id), s("--force")], "dex delete")
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

    if trimmed.contains(".config/dex") {
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
        assert!(tasks[1].is_blocked());
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

        assert_eq!(tasks[0].status(), Status::Pending);
        assert_eq!(tasks[1].status(), Status::Completed);
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

    #[test]
    fn store_label_names_the_project_or_global() {
        assert_eq!(store_label("/Users/x/Developer/myproj/.dex"), "myproj");
        assert_eq!(store_label("/Users/x/.config/dex/local"), "global");
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
}
