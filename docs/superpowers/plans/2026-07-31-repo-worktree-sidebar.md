# Repo and Worktree Sidebar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a third pane listing registered repositories with their git worktrees nested underneath, where selecting a worktree decides which dex store the task tree and detail pane read.

**Architecture:** Three new modules with one job each — `worktree` parses `git worktree list --porcelain`, `registry` owns a writable list of repo paths, and `repos` combines them into renderable rows. Every dex call is routed through a new `Dex::for_store(path)` so exactly one place can target the wrong store. The pane ladder extends the existing zoom mechanism rather than adding a second one.

**Tech Stack:** Rust edition 2024, ratatui 0.30, crossterm 0.29, serde + toml (all already present). **No new dependencies.**

## Global Constraints

- **No new crates.** Everything here uses dependencies already in `Cargo.toml`.
- **Colour lives in `src/theme.rs`.** Use only `Color::Reset` and ANSI-16 names; never `Indexed` or `Rgb`, never `Color::White`/`Color::Black` for text. A test walks `theme::ALL` and enforces this.
- **Writes go through the dex CLI**, never to `tasks.jsonl` directly. Arguments go through `Command::args`, never a shell.
- **A refresh must never disturb the user** — it may not move the task selection, the worktree selection, collapse a node, or interrupt a dialog.
- **Idle cost stays at zero.** No new timer and no new poll cadence; `pulse::poll_timeout` is not to be touched.
- **Tests live in `#[cfg(test)]` modules beside the code they cover.**
- Run `cargo test` and `cargo clippy --all-targets` before every commit. Both must be clean.
- `cargo build` has twice left `target/debug/dextui` stale while reporting `Finished`. Before believing any tmux capture, check `ls -la target/debug/dextui`; `cargo clean -p dextui` is the fix.

---

## File Structure

**Created:**
- `src/worktree.rs` — parse and list git worktrees. No dex, no UI.
- `src/registry.rs` — the writable repo list at `~/.config/dextui/repos.toml`.
- `src/repos.rs` — registry + worktrees, flattened into rows for rendering.

**Modified:**
- `src/dex.rs` — `Dex::for_store`, and every verb routed through one argv builder.
- `src/config.rs` — add `repos_pane_above`.
- `src/app.rs` — `Focus::Repos`, worktree selection, per-worktree task memory.
- `src/ui.rs` — the three-pane ladder and the sidebar renderer.
- `src/main.rs` — keys, store switching, multi-store watching.
- `CLAUDE.md`, `README.md` — documentation.

---

### Task 1: Every dex call targets a chosen store

**Files:**
- Modify: `src/dex.rs`
- Test: `src/dex.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `Dex::for_store(store_dir: &str) -> Result<Dex, String>`, `Dex::for_store_with(store_dir: &str, runner: Box<dyn Runner>) -> Result<Dex, String>`. `Dex::real()` and `Dex::new()` keep their signatures and target the cwd's store.

This is the highest-risk change in the plan. Writing to the wrong store is silent and destroys someone else's task list, so every verb goes through one argv builder rather than each call site remembering a flag.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/dex.rs`:

```rust
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --bin dextui dex::tests 2>&1 | tail -20`
Expected: FAIL — `no function or associated item named 'for_store_with'`.

- [ ] **Step 3: Implement**

In `src/dex.rs`, change the `Dex` struct:

```rust
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
```

Update `new` and `real` to set `store: None`, then add:

```rust
impl Dex {
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
}
```

Then route every verb through it. Each currently reads `self.runner.run(&[...])`; change to `self.runner.run(&self.argv(&[...]))`. Find them all with the grep in Step 5.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --bin dextui 2>&1 | tail -3`
Expected: PASS, with no existing test broken.

- [ ] **Step 5: Verify no verb was missed**

Run: `grep -n "runner.run(&\[" src/dex.rs`
Expected: **no output.** Every call site must now read `self.argv(...)`. A hit here is a verb that still targets the wrong store.

- [ ] **Step 6: Commit**

```bash
git add src/dex.rs
git commit -m "feat: target a chosen dex store through one argv builder"
```

---

### Task 2: Parse git worktrees

**Files:**
- Create: `src/worktree.rs`
- Modify: `src/main.rs` (add `mod worktree;` to the module list at the top)
- Test: `src/worktree.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `struct Worktree { path: String, branch: String, is_main: bool, is_locked: bool, is_detached: bool }`, `fn parse(porcelain: &str) -> Vec<Worktree>`, `fn list(repo_path: &str) -> Result<Vec<Worktree>, String>`.

- [ ] **Step 1: Write the failing tests**

Create `src/worktree.rs`:

```rust
//! Git worktrees for a repository. No dex, no UI -- just `git worktree list`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: String,
    /// The branch name with `refs/heads/` stripped, or the short SHA when
    /// detached. Never empty, so a row always has something to show.
    pub branch: String,
    /// The main checkout, which porcelain always lists first.
    pub is_main: bool,
    pub is_locked: bool,
    pub is_detached: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `git worktree list --porcelain` on a real repo with linked
    /// worktrees, all locked -- which is the common case here and was nearly
    /// missed.
    const PORCELAIN: &str = "\
worktree /Users/x/Developer/TaxCommHub
HEAD edaad18c1111111111111111111111111111111
branch refs/heads/main

worktree /Users/x/Developer/TaxCommHub-561
HEAD 65416862222222222222222222222222222222b
branch refs/heads/561-enrollment-window-support
locked

worktree /Users/x/Developer/TaxCommHub-detached
HEAD 06dd22433333333333333333333333333333333
detached

worktree /Users/x/Developer/TaxCommHub-email
HEAD 5db559e64444444444444444444444444444444
branch refs/heads/email-project-prototype
locked
";

    #[test]
    fn the_first_worktree_is_the_main_checkout() {
        let w = parse(PORCELAIN);
        assert_eq!(w.len(), 4);
        assert!(w[0].is_main, "porcelain lists the main checkout first");
        assert!(!w[1].is_main);
    }

    #[test]
    fn branch_names_lose_their_refs_prefix() {
        let w = parse(PORCELAIN);
        assert_eq!(w[0].branch, "main");
        assert_eq!(w[1].branch, "561-enrollment-window-support");
    }

    /// All the real worktrees here are locked, so dropping this attribute would
    /// have looked fine on a toy repo and wrong on every real one.
    #[test]
    fn locked_worktrees_are_marked_not_skipped() {
        let w = parse(PORCELAIN);
        assert!(w[1].is_locked);
        assert!(!w[0].is_locked, "the main checkout is not locked");
        assert_eq!(w.len(), 4, "a locked worktree is still a worktree");
    }

    /// A detached worktree has no branch line at all. Left empty the row would
    /// render blank, so it falls back to the short SHA.
    #[test]
    fn a_detached_worktree_shows_its_sha_rather_than_nothing() {
        let w = parse(PORCELAIN);
        assert!(w[2].is_detached);
        assert_eq!(w[2].branch, "06dd224");
        assert!(!w[2].branch.is_empty());
    }

    #[test]
    fn empty_input_is_no_worktrees_not_a_panic() {
        assert_eq!(parse(""), vec![]);
        assert_eq!(parse("\n\n"), vec![]);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin dextui worktree 2>&1 | tail -10`
Expected: FAIL — `cannot find function 'parse'`.

- [ ] **Step 3: Implement**

Add above the test module in `src/worktree.rs`:

```rust
use std::process::Command;

/// Parses `git worktree list --porcelain`.
///
/// The format is stanzas separated by blank lines, each starting with a
/// `worktree <path>` line. Attributes that follow are one per line and may be
/// absent -- `branch` is missing entirely when detached, and `locked` appears
/// only when set.
pub fn parse(porcelain: &str) -> Vec<Worktree> {
    let mut out: Vec<Worktree> = Vec::new();
    let mut current: Option<Worktree> = None;
    let mut head = String::new();

    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(w) = current.take() {
                out.push(w);
            }
            head.clear();
            current = Some(Worktree {
                path: path.to_string(),
                branch: String::new(),
                is_main: out.is_empty(),
                is_locked: false,
                is_detached: false,
            });
        } else if let Some(sha) = line.strip_prefix("HEAD ") {
            head = sha.to_string();
        } else if let Some(b) = line.strip_prefix("branch ") {
            if let Some(w) = current.as_mut() {
                w.branch = b.trim_start_matches("refs/heads/").to_string();
            }
        } else if line == "detached" {
            if let Some(w) = current.as_mut() {
                w.is_detached = true;
                // No branch line follows, and a blank row is useless.
                w.branch = head.chars().take(7).collect();
            }
        } else if line == "locked" || line.starts_with("locked ") {
            if let Some(w) = current.as_mut() {
                w.is_locked = true;
            }
        }
    }
    if let Some(w) = current.take() {
        out.push(w);
    }
    out
}

/// Every worktree of `repo_path`, main checkout first.
pub fn list(repo_path: &str) -> Result<Vec<Worktree>, String> {
    let out = Command::new("git")
        .args(["-C", repo_path, "worktree", "list", "--porcelain"])
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;

    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(parse(&String::from_utf8_lossy(&out.stdout)))
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --bin dextui worktree 2>&1 | tail -3`
Expected: PASS, 5 tests.

- [ ] **Step 5: Check the fixture matches reality**

Run: `git worktree list --porcelain`
Expected: the same shape as `PORCELAIN` — a `worktree` line, a `HEAD` line, then `branch` or `detached`. If real output has a stanza shape the fixture lacks, add it and re-run.

- [ ] **Step 6: Commit**

```bash
git add src/worktree.rs src/main.rs
git commit -m "feat: parse git worktree list --porcelain"
```

---

### Task 3: The writable repo registry

**Files:**
- Create: `src/registry.rs`
- Modify: `src/main.rs` (add `mod registry;`)
- Test: `src/registry.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `struct Registry { pub repos: Vec<String> }`, `Registry::load() -> (Registry, Option<String>)`, `Registry::parse(text: &str) -> (Registry, Option<String>)`, `Registry::save(&self) -> Result<(), String>`, `Registry::add(&mut self, path: &str) -> bool`, `Registry::remove(&mut self, path: &str) -> bool`, `fn path() -> Option<PathBuf>`.

`add` and `remove` return whether anything changed, so the caller can report "already registered" rather than appearing to do nothing.

- [ ] **Step 1: Write the failing tests**

Create `src/registry.rs`:

```rust
//! The list of repositories the sidebar shows.
//!
//! **A separate file from `config.toml` on purpose.** That file is read-only to
//! the app -- persisting toggles would clobber a file someone hand-edited, and
//! silently change their defaults. This one is app-owned state that the app
//! writes. Two files, and neither rule needs qualifying.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Registry {
    #[serde(default)]
    pub repos: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_a_repo_reports_that_it_changed() {
        let mut r = Registry::default();
        assert!(r.add("/x/one"));
        assert_eq!(r.repos, vec!["/x/one".to_string()]);
    }

    /// Pressing `a` twice is not a mistake worth an error, but the caller needs
    /// to know so it can say "already registered" instead of nothing at all.
    #[test]
    fn adding_a_known_repo_changes_nothing_and_says_so() {
        let mut r = Registry::default();
        r.add("/x/one");
        assert!(!r.add("/x/one"));
        assert_eq!(r.repos.len(), 1, "duplicated: {:?}", r.repos);
    }

    #[test]
    fn removing_reports_whether_it_was_there() {
        let mut r = Registry::default();
        r.add("/x/one");
        assert!(r.remove("/x/one"));
        assert!(!r.remove("/x/one"));
        assert!(r.repos.is_empty());
    }

    #[test]
    fn a_registry_round_trips_through_toml() {
        let mut r = Registry::default();
        r.add("/x/one");
        r.add("/x/two");

        let text = toml::to_string(&r).unwrap();
        let back: Registry = toml::from_str(&text).unwrap();
        assert_eq!(back, r);
    }

    /// A bad file must not stop the app starting -- the same rule config already
    /// follows, for the same reason.
    #[test]
    fn a_malformed_file_is_reported_and_not_fatal() {
        let (reg, problem) = Registry::parse("repos = \"not a list\"");
        assert_eq!(reg, Registry::default());
        assert!(problem.is_some(), "a bad file must be reported");
    }

    #[test]
    fn an_empty_file_is_normal_and_silent() {
        let (reg, problem) = Registry::parse("");
        assert_eq!(reg, Registry::default());
        assert!(problem.is_none(), "an empty registry is not a problem");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin dextui registry 2>&1 | tail -10`
Expected: FAIL — `no function or associated item named 'add'`.

- [ ] **Step 3: Implement**

Add above the test module:

```rust
impl Registry {
    /// Returns whether the list changed, so a duplicate `a` can be reported
    /// rather than looking like nothing happened.
    pub fn add(&mut self, path: &str) -> bool {
        if self.repos.iter().any(|p| p == path) {
            return false;
        }
        self.repos.push(path.to_string());
        self.repos.sort();
        true
    }

    pub fn remove(&mut self, path: &str) -> bool {
        let before = self.repos.len();
        self.repos.retain(|p| p != path);
        self.repos.len() != before
    }

    /// Parses registry text, reporting a problem rather than failing.
    pub fn parse(text: &str) -> (Registry, Option<String>) {
        if text.trim().is_empty() {
            return (Registry::default(), None);
        }
        match toml::from_str::<Registry>(text) {
            Ok(r) => (r, None),
            Err(e) => (Registry::default(), Some(format!("repos.toml: {e}"))),
        }
    }

    pub fn load() -> (Registry, Option<String>) {
        let Some(p) = path() else {
            return (Registry::default(), None);
        };
        match std::fs::read_to_string(&p) {
            Ok(text) => Registry::parse(&text),
            // Missing is the normal first-run state, and silent.
            Err(_) => (Registry::default(), None),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let p = path().ok_or_else(|| "could not resolve a config directory".to_string())?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = toml::to_string(self).map_err(|e| format!("could not serialise: {e}"))?;
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// Beside `config.toml`, resolved the same way so both follow XDG.
pub fn path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("dextui").join("repos.toml"))
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --bin dextui registry 2>&1 | tail -3`
Expected: PASS, 6 tests.

- [ ] **Step 5: Commit**

```bash
git add src/registry.rs src/main.rs
git commit -m "feat: a writable repo registry, kept out of the read-only config"
```

---

### Task 4: Rows for the sidebar

**Files:**
- Create: `src/repos.rs`
- Modify: `src/main.rs` (add `mod repos;`)
- Test: `src/repos.rs`

**Interfaces:**
- Consumes: `worktree::Worktree`.
- Produces: `struct Repo { name: String, path: String, worktrees: Vec<Worktree>, open: bool }`, `enum Row { Repo { index: usize }, Worktree { repo: usize, index: usize } }`, `fn rows(repos: &[Repo]) -> Vec<Row>`, `fn store_dir(worktree_path: &str) -> String`, `fn has_store(worktree_path: &str) -> bool`.

Flattening to rows mirrors `tree::visible_rows`, so the sidebar's selection and click handling reuse the shape the task tree already uses.

- [ ] **Step 1: Write the failing tests**

Create `src/repos.rs`:

```rust
//! Registered repositories and their worktrees, flattened for rendering.
//!
//! Mirrors `tree::visible_rows`: a flat list of rows with enough identity to
//! address the thing each one draws, so selection and clicking work the same way
//! they already do in the task tree.

use crate::worktree::Worktree;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub name: String,
    pub path: String,
    pub worktrees: Vec<Worktree>,
    pub open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Repo { index: usize },
    Worktree { repo: usize, index: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: &str, main: bool) -> Worktree {
        Worktree {
            path: path.to_string(),
            branch: branch.to_string(),
            is_main: main,
            is_locked: false,
            is_detached: false,
        }
    }

    fn repo(name: &str, open: bool) -> Repo {
        Repo {
            name: name.to_string(),
            path: format!("/x/{name}"),
            worktrees: vec![
                wt(&format!("/x/{name}"), "main", true),
                wt(&format!("/x/{name}-feat"), "feat", false),
            ],
            open,
        }
    }

    #[test]
    fn an_open_repo_lists_its_worktrees_beneath_it() {
        let rs = vec![repo("one", true)];
        assert_eq!(
            rows(&rs),
            vec![
                Row::Repo { index: 0 },
                Row::Worktree { repo: 0, index: 0 },
                Row::Worktree { repo: 0, index: 1 },
            ]
        );
    }

    #[test]
    fn a_closed_repo_hides_its_worktrees() {
        let rs = vec![repo("one", false)];
        assert_eq!(rows(&rs), vec![Row::Repo { index: 0 }]);
    }

    #[test]
    fn repos_keep_their_order_and_do_not_interleave() {
        let rs = vec![repo("a", true), repo("b", false), repo("c", true)];
        let r = rows(&rs);
        assert_eq!(r[0], Row::Repo { index: 0 });
        assert_eq!(r[3], Row::Repo { index: 1 });
        assert_eq!(r[4], Row::Repo { index: 2 });
    }

    /// dex stores live in `.dex` under the worktree, and this is the one place
    /// that knows it -- `Dex::for_store` rejects anything else.
    #[test]
    fn a_store_is_the_dex_directory_under_the_worktree() {
        assert_eq!(store_dir("/x/one"), "/x/one/.dex");
        assert_eq!(store_dir("/x/one/"), "/x/one/.dex");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin dextui repos 2>&1 | tail -10`
Expected: FAIL — `cannot find function 'rows'`.

- [ ] **Step 3: Implement**

Add above the test module:

```rust
/// Every visible row, top to bottom.
pub fn rows(repos: &[Repo]) -> Vec<Row> {
    let mut out = Vec::new();
    for (i, r) in repos.iter().enumerate() {
        out.push(Row::Repo { index: i });
        if r.open {
            for (j, _) in r.worktrees.iter().enumerate() {
                out.push(Row::Worktree { repo: i, index: j });
            }
        }
    }
    out
}

/// The dex store for a worktree.
pub fn store_dir(worktree_path: &str) -> String {
    format!("{}/.dex", worktree_path.trim_end_matches('/'))
}

/// Whether a worktree has a store yet. A plain on-disk check, deliberately not a
/// dex call: this runs for every row, and a worktree without tasks is an
/// ordinary row rather than an error.
pub fn has_store(worktree_path: &str) -> bool {
    std::path::Path::new(&store_dir(worktree_path)).is_dir()
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --bin dextui repos 2>&1 | tail -3`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add src/repos.rs src/main.rs
git commit -m "feat: flatten repos and worktrees into sidebar rows"
```

---

### Task 5: The pane ladder

**Files:**
- Modify: `src/config.rs`, `src/app.rs`
- Test: `src/app.rs`

**Interfaces:**
- Consumes: `Config`.
- Produces: `enum Panes { One, Two, Three }`, `App::panes(&self) -> Panes`, `Config::repos_pane_above: u16`, `App::repos_pane_above: u16`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/app.rs`:

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin dextui pane_ladder 2>&1 | tail -10`
Expected: FAIL — `cannot find type 'Panes'`.

- [ ] **Step 3: Implement**

In `src/config.rs`, add to `Config`:

```rust
    /// Terminal width at or above which the repo pane is drawn as a third pane.
    ///
    /// Three panes need roughly this much before each is worth having. `0` never
    /// shows it, matching what `single_pane_below = 0` means for the split.
    pub repos_pane_above: u16,
```

Set `repos_pane_above: 110` in the `Default` impl. `Config` uses `deny_unknown_fields`, so also add the key wherever fields are applied from a parsed file, and to `EXAMPLE`:

```toml
repos_pane_above = 110  # show the repo pane at or above this width; 0 never
```

In `src/app.rs`:

```rust
/// How many panes are drawn. A single ordered ladder, so first-fit can only
/// ever shed -- see `the_pane_ladder_is_monotone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panes {
    One,
    Two,
    Three,
}

impl App {
    pub fn panes(&self) -> Panes {
        if self.zoomed() {
            return Panes::One;
        }
        if self.repos_pane_above > 0 && self.terminal_width >= self.repos_pane_above {
            return Panes::Three;
        }
        Panes::Two
    }
}
```

Add `pub repos_pane_above: u16` to `App`, set from `cfg.repos_pane_above` in `App::new` and in `apply_config`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --bin dextui 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/app.rs
git commit -m "feat: a monotone three-rung pane ladder"
```

---

### Task 6: Worktree selection and per-worktree memory

**Files:**
- Modify: `src/app.rs`
- Test: `src/app.rs`

**Interfaces:**
- Consumes: `repos::{Repo, Row, rows}`, `registry::Registry`.
- Produces: `Focus::Repos`, `App::selected_worktree: Option<String>`, `App::task_memory: HashMap<String, String>`, `App::repos: Vec<repos::Repo>`, `App::selected_repo_row: usize`, `App::registry: registry::Registry`, `App::select_worktree(&mut self, path: &str)`, `App::repo_rows(&self) -> Vec<repos::Row>`.

- [ ] **Step 1: Write the failing tests**

```rust
/// Switching away and back must return the cursor to where it was, or the pane
/// is tedious for exactly the comparison it exists to serve.
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin dextui remembers 2>&1 | tail -10`
Expected: FAIL — `no field 'selected_worktree'`.

- [ ] **Step 3: Implement**

Add `Repos` to the `Focus` enum. Add to `App`:

```rust
    /// Which worktree's store the task tree is showing.
    pub selected_worktree: Option<String>,
    /// Task selection per worktree path, so switching back returns the cursor.
    /// Session-only: this is view state, not something to persist.
    pub task_memory: HashMap<String, String>,
    /// Registered repos with their worktrees, and whether each is expanded.
    pub repos: Vec<crate::repos::Repo>,
    pub selected_repo_row: usize,
    pub registry: crate::registry::Registry,
```

Initialise in `App::new`: `selected_worktree: None`, `task_memory: HashMap::new()`, `repos: Vec::new()`, `selected_repo_row: 0`, `registry: crate::registry::Registry::default()`.

```rust
impl App {
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
        self.selected = self.task_memory.get(path).cloned();
    }

    pub fn repo_rows(&self) -> Vec<crate::repos::Row> {
        crate::repos::rows(&self.repos)
    }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --bin dextui 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat: select a worktree, and remember the cursor in each"
```

---

### Task 7: Draw the sidebar

**Files:**
- Modify: `src/ui.rs`
- Test: `src/ui.rs`

**Interfaces:**
- Consumes: `App::repo_rows`, `App::panes`, `repos::has_store`, `Panes`.
- Produces: `fn draw_repos(frame: &mut Frame, app: &mut App, ic: &Icons, area: Rect)`.

- [ ] **Step 1: Write the failing tests**

```rust
/// The sidebar draws, and shows both levels.
#[test]
fn the_sidebar_shows_repos_with_their_worktrees() {
    let mut app = App::new(
        vec![task("a", None, "A task")],
        "demo".into(),
        crate::config::Config::default(),
    );
    app.terminal_width = 140;
    app.repos_pane_above = 110;
    app.repos = vec![crate::repos::Repo {
        name: "dextui".into(),
        path: "/x/dextui".into(),
        worktrees: vec![crate::worktree::Worktree {
            path: "/x/dextui".into(),
            branch: "main".into(),
            is_main: true,
            is_locked: false,
            is_detached: false,
        }],
        open: true,
    }];
    app.rebuild();

    let mut terminal = Terminal::new(TestBackend::new(140, 14)).unwrap();
    terminal
        .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
        .unwrap();

    let buf = terminal.backend().buffer();
    let text: String = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("dextui"), "no repo row: {text}");
    assert!(text.contains("main"), "no worktree row: {text}");
}

/// Every rung of the ladder draws without panicking, including the boundaries.
#[test]
fn every_width_draws_without_panicking() {
    for w in [40u16, 79, 80, 109, 110, 160] {
        let mut app = App::new(
            vec![task("a", None, "A task")],
            "demo".into(),
            crate::config::Config::default(),
        );
        app.terminal_width = w;
        app.rebuild();
        let mut terminal = Terminal::new(TestBackend::new(w, 14)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin dextui sidebar 2>&1 | tail -10`
Expected: FAIL — the repo name is not drawn.

- [ ] **Step 3: Implement**

Add to `src/ui.rs`:

```rust
/// The repo pane: registered repositories with their worktrees beneath.
///
/// Colour carries only what the task tree's already does -- a worktree with a
/// store is `PLAIN`, one without is `DIM` -- so the sidebar introduces no new
/// palette and `theme::ALL` stays the whole story.
fn draw_repos(frame: &mut Frame, app: &mut App, ic: &Icons, area: Rect) {
    let focused = app.focus == Focus::Repos;
    let block = Block::bordered()
        .title(" repos ")
        .title_style(Style::default().fg(DIM))
        .border_style(Style::default().fg(if focused { PLAIN } else { DIM }));

    let rows = app.repo_rows();
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let selected = i == app.selected_repo_row;
            let gutter = if selected {
                Span::styled(format!("{} ", ic.gutter), Style::default().fg(ACCENT))
            } else {
                Span::raw("  ")
            };
            let body = match row {
                crate::repos::Row::Repo { index } => {
                    let r = &app.repos[*index];
                    Span::styled(
                        format!("{} {}", ic.marker(true, r.open), r.name),
                        Style::default().fg(PLAIN).add_modifier(Modifier::BOLD),
                    )
                }
                crate::repos::Row::Worktree { repo, index } => {
                    let w = &app.repos[*repo].worktrees[*index];
                    let has = crate::repos::has_store(&w.path);
                    Span::styled(
                        format!("   {}", w.branch),
                        Style::default().fg(if has { PLAIN } else { DIM }),
                    )
                }
            };
            ListItem::new(Line::from(vec![gutter, body]))
        })
        .collect();

    frame.render_widget(List::new(items).block(block), area);
}
```

In `draw`, split the body by `app.panes()`. For `Panes::Three`, produce three areas — repos, tree, detail — and call `draw_repos` on the first. `Two` and `One` keep today's behaviour unchanged.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --bin dextui 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Look at it**

```bash
cargo build && ls -la target/debug/dextui
scripts/render-check.sh
```

Expected: a fresh timestamp on the binary, and three panes with `repos` on the left.

- [ ] **Step 6: Commit**

```bash
git add src/ui.rs
git commit -m "feat: draw the repo and worktree sidebar"
```

---

### Task 8: Keys, registration and store switching

**Files:**
- Modify: `src/main.rs`, `src/app.rs`
- Test: `src/app.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `App::register_repo_path(&mut self, repo_path: &str) -> Result<bool, String>`, `App::unregister_repo_path(&mut self, repo_path: &str)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn registering_adds_the_repo_and_reports_the_change() {
    let mut app = counted(vec![task("a", None, &[])]);
    app.registry = crate::registry::Registry::default();

    assert!(app.register_repo_path("/x/dextui").unwrap());
    assert_eq!(app.registry.repos, vec!["/x/dextui".to_string()]);
}

#[test]
fn registering_a_known_repo_is_reported_not_duplicated() {
    let mut app = counted(vec![task("a", None, &[])]);
    app.registry = crate::registry::Registry::default();
    app.register_repo_path("/x/dextui").unwrap();

    assert!(
        !app.register_repo_path("/x/dextui").unwrap(),
        "a duplicate must report that nothing changed"
    );
    assert_eq!(app.registry.repos.len(), 1);
}

/// Unregistering is a view operation. It must never touch the worktree, the
/// branch or the store -- only the entry and the row.
#[test]
fn unregistering_removes_only_the_entry() {
    let mut app = counted(vec![task("a", None, &[])]);
    app.registry = crate::registry::Registry::default();
    app.register_repo_path("/x/one").unwrap();
    app.register_repo_path("/x/two").unwrap();

    app.unregister_repo_path("/x/one");
    assert_eq!(app.registry.repos, vec!["/x/two".to_string()]);
}
```

**Note for the implementer:** these tests call `save()`, which writes the real `repos.toml`. Point `XDG_CONFIG_HOME` at a temp directory for the test process before running them. Do not skip this — a suite that rewrites the user's registry is worse than no suite.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin dextui register 2>&1 | tail -10`
Expected: FAIL — `no method named 'register_repo_path'`.

- [ ] **Step 3: Implement**

```rust
impl App {
    /// Registers a repo. Returns whether the registry changed, so a duplicate
    /// can be reported rather than looking inert.
    pub fn register_repo_path(&mut self, repo_path: &str) -> Result<bool, String> {
        let changed = self.registry.add(repo_path);
        if changed {
            self.registry.save()?;
        }
        Ok(changed)
    }

    pub fn unregister_repo_path(&mut self, repo_path: &str) {
        if self.registry.remove(repo_path) {
            let _ = self.registry.save();
            self.repos.retain(|r| r.path != repo_path);
            self.selected_repo_row = self
                .selected_repo_row
                .min(self.repo_rows().len().saturating_sub(1));
        }
    }
}
```

In `src/main.rs`, add key arms. **The `Focus::Repos` guards must come before the existing unguarded arms**, or `a` will create a subtask instead of registering:

```rust
KeyCode::Char('3') => app.focus = Focus::Repos,
KeyCode::Char('a') if app.focus == Focus::Repos => {
    // Register the main repo -- `git worktree list` reports it first, so the
    // repo that *has* the worktrees is what gets registered, not the worktree
    // the cursor happens to be in. Report the returned bool via app.status.
}
KeyCode::Char('D') if app.focus == Focus::Repos => {
    // Confirm via the existing Mode::Confirm dialog, then unregister. This
    // takes a row and its whole subtree off screen, so it is not silent.
}
```

`enter` and `l` in `Focus::Repos` call `app.select_worktree(path)` for the row under the cursor, set `app.focus = Focus::Tree`, and load that store via `Dex::for_store(&repos::store_dir(path))`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --bin dextui 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Check for a key collision**

Run: `grep -n "Char('a')" src/main.rs`
Expected: two arms, with the `Focus::Repos` one **above** the subtask one. Then by hand:

```bash
scripts/render-check.sh "3 a"
```

Expected: a status message about registration, not a "new subtask" prompt.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/app.rs
git commit -m "feat: register repos and switch stores from the sidebar"
```

---

### Task 9: Load and watch every registered store

**Files:**
- Modify: `src/main.rs`, `src/watch.rs`
- Test: `src/watch.rs`

**Interfaces:**
- Consumes: `repos::store_dir`, `Dex::for_store`, `watch::spawn`.
- Produces: `watch::spawn_many(dirs: &[String], out: Sender<String>) -> Vec<StoreWatcher>` — sends the path that changed, so the caller re-reads only that store.

- [ ] **Step 1: Write the failing test**

```rust
/// The multi-store watcher has to say *which* store changed, or the caller must
/// re-read all of them and watching separately buys nothing.
#[test]
fn a_change_reports_which_store_it_came_from() {
    let dir = std::env::temp_dir().join("dextui-watch-many");
    let a = dir.join("a");
    let b = dir.join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let _guards = spawn_many(
        &[
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ],
        tx,
    );

    std::fs::write(b.join("tasks.jsonl"), "{}").unwrap();

    let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(got.ends_with("/b"), "reported the wrong store: {got}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --bin dextui watch 2>&1 | tail -10`
Expected: FAIL — `cannot find function 'spawn_many'`.

- [ ] **Step 3: Implement**

In `src/watch.rs`, alongside `spawn`:

```rust
/// One watcher per store, each tagging its events with the directory they came
/// from so only that store is re-read.
///
/// The returned guards must be kept alive for the whole run; dropping one stops
/// its notifications.
pub fn spawn_many(dirs: &[String], out: Sender<String>) -> Vec<StoreWatcher> {
    dirs.iter()
        .map(|dir| {
            let (tx, rx) = channel::<()>();
            let guard = spawn(dir, tx);
            let out = out.clone();
            let dir = dir.clone();
            std::thread::spawn(move || {
                while rx.recv().is_ok() {
                    if out.send(dir.clone()).is_err() {
                        return;
                    }
                }
            });
            guard
        })
        .collect()
}
```

In `src/main.rs`: at startup, read counts for every registered worktree **concurrently** — one `std::thread::spawn` per store calling `Dex::for_store(&store_dir)` then `.list()`, joined before the first draw, so ten stores cost ~180ms rather than 1.8s. Keep the `spawn_many` guards alive for the run. The selected store keeps today's `watch::spawn` plus the existing 10s poll; the others get **no poll**, which is the documented staleness gap.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --bin dextui watch 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 5: Confirm the idle guarantee is intact**

Run: `grep -n "poll_timeout" src/main.rs`
Expected: exactly one call site, unchanged. A second timer here would break the guarantee `pulse.rs` exists to make testable.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/watch.rs
git commit -m "feat: watch every registered store, re-reading only what changed"
```

---

### Task 10: Documentation

**Files:**
- Modify: `CLAUDE.md`, `README.md`

- [ ] **Step 1: Rewrite the Scope section**

`CLAUDE.md`'s Scope section ends *"and multi-project views. dextui shows the current directory's store only."* That is now false. Replace it, and add a section covering:

- why the registry is a second file rather than a key in `config.toml`;
- that `Dex::for_store` is the single choke point for `--storage-path`, and that writing to the wrong store is silent;
- that `--storage-path` wants the `.dex` **directory** and returns an empty list for a file;
- the deliberate staleness gap for unselected worktrees, and why polling them all was rejected.

- [ ] **Step 2: Update the README**

Add the repo pane to the layout description, the keys table (`3`, `a`, `D`), and `repos_pane_above` to the config block.

- [ ] **Step 3: Regenerate the screenshot**

```bash
cargo build && ls -la target/debug/dextui
COLS=140 scripts/screenshot.sh
```

Expected: three panes. Check the binary's timestamp first — a stale one has produced a wrong screenshot twice.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md README.md docs/img/
git commit -m "docs: the repo sidebar, and what it changes about scope"
```

---

## Self-Review

**Spec coverage:** registry → Tasks 3, 8. Worktree discovery → Task 2. Store paths and counts → Tasks 4, 9. Registration keys → Task 8. Layout ladder → Tasks 5, 7. Reads and writes via `--storage-path` → Task 1. Refresh model → Task 9. Invariants → Task 6 (selection survival), Task 5 (monotone ladder), Task 9 (idle cost). Docs → Task 10.

**Two things deliberately left to the implementer, flagged rather than hidden:**

1. The `D` confirmation dialog is described in Task 8 as a comment, not code, because it reuses `Mode::Confirm` whose exact shape depends on how the key handling lands. Wire it to the existing dialog rather than inventing a second one.
2. Task 8's tests call `save()`, which writes the real `repos.toml`. The implementer must isolate that — `XDG_CONFIG_HOME` pointed at a temp directory is the least invasive fix. This is called out in the task itself.

**Type consistency:** `store_dir` is defined once in `repos.rs` (Task 4) and consumed by Tasks 7, 8 and 9. `Row` and `Repo` are defined in Task 4 and consumed in Tasks 6 and 7. `Panes` is defined in Task 5 and consumed in Task 7. `Worktree` is defined in Task 2 and consumed in Tasks 4 and 7. `Registry` is defined in Task 3 and consumed in Tasks 6 and 8.

**Ordering:** Tasks 1–4 are independent. Task 5 needs nothing. Task 6 needs 3 and 4. Task 7 needs 4, 5, 6. Task 8 needs 6 and 7. Task 9 needs 1 and 4. Task 10 last.

---

### Task 11: A sync log

**Added mid-plan.** Troubleshooting the watcher/refresh path has no evidence to work
from: the app's only feedback is `app.status`, one line, overwritten, no history —
and once the alternate screen is up, stdout and stderr belong to the TUI, so nothing
can be printed. The stat-gated safety net from Task 9 makes this sharper, because it
introduces a "decided **not** to refresh" branch that is invisible by design.

**Files:**
- Create: `src/log.rs`
- Modify: `src/main.rs`, `src/watch.rs`
- Modify: `CLAUDE.md`, `README.md`

**Interfaces:**
- Produces: `log::init()`, `log::line(area: &str, msg: &str)`, `log::path() -> Option<PathBuf>`.

**Design, decided:**
- Always on. An opt-in log is off precisely when the bug you did not expect happens,
  and sync faults are the kind that will not reproduce on demand.
- File only, at `$XDG_STATE_HOME/dextui/log`, falling back to `~/.local/state/dextui/log`.
  State, not config: it is machine-local, disposable, and must never sit beside
  `config.toml`, which is the user's hand-edited text.
- **Size-capped by truncation at startup**, not rotation. A log you `tail -f` while
  reproducing does not need history, and rotation is machinery for a problem this
  does not have.
- **Failure is silent and total.** If the file cannot be opened or written, the app
  behaves exactly as if logging were off. A logger that can break the program it
  exists to diagnose is worse than no logger.
- Never `stdout`/`stderr` once the TUI owns the terminal.

**Format:** `HH:MM:SS  area  message`, area padded so the column reads straight.
Areas: `watch`, `dex`, `store`, `registry`.

**What must be logged**, chosen so a sync fault is diagnosable from the file alone:
- `watch` — a watcher registered for a store, an FS event received, and **every
  safety tick with its outcome**, including `unchanged` (the invisible branch).
- `dex` — each `list` issued, which store, how many tasks came back, and how long it
  took. The duration is what makes a slow store obvious.
- `store` — switching the selected worktree, from and to.
- `registry` — loaded, saved, and any failure of either.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The whole value is that it is already on when something goes wrong, so a
    /// missing directory must be created rather than silently dropping the log.
    #[test]
    fn a_line_is_appended_and_the_directory_is_created() {
        let dir = std::env::temp_dir().join(format!("dextui-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("nested").join("log");

        write_line(&file, "watch", "registered /x/.dex");
        write_line(&file, "dex", "list /x/.dex - 14 tasks 173ms");

        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("registered /x/.dex"), "{text}");
        assert!(text.contains("14 tasks"), "{text}");
        assert_eq!(text.lines().count(), 2, "appended, not overwritten: {text}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A logger that can break the program it exists to diagnose is worse than no
    /// logger. An unwritable path must be a no-op, not a panic and not an error
    /// the caller has to handle.
    #[test]
    fn an_unwritable_path_is_silently_ignored() {
        // /dev/null/x cannot be created: /dev/null is not a directory.
        write_line(std::path::Path::new("/dev/null/x/log"), "watch", "ignored");
    }

    /// Truncation, not rotation. Reproducing a fault does not need history, and a
    /// log that grows without bound is its own problem.
    #[test]
    fn an_oversized_log_is_truncated_at_startup() {
        let dir = std::env::temp_dir().join(format!("dextui-log-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("log");
        std::fs::write(&file, "x".repeat(CAP + 1)).unwrap();

        truncate_if_oversized(&file);

        assert_eq!(std::fs::metadata(&file).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_log_within_the_cap_survives_startup() {
        let dir = std::env::temp_dir().join(format!("dextui-log-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("log");
        std::fs::write(&file, "kept").unwrap();

        truncate_if_oversized(&file);

        assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The area column is what makes the file skimmable; a ragged one is not.
    #[test]
    fn the_area_column_is_aligned() {
        let a = format_line("watch", "one");
        let b = format_line("registry", "two");
        let col = |s: &str| s.find("one").or_else(|| s.find("two")).unwrap();
        assert_eq!(col(&a), col(&b), "ragged columns:\n{a}\n{b}");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --bin dextui log 2>&1 | tail -10`
Expected: FAIL — `cannot find function 'write_line'`.

- [ ] **Step 3: Implement**

`src/log.rs`, with `CAP`, `format_line`, `write_line`, `truncate_if_oversized`, a
process-wide resolved path set by `init()`, and a `line(area, msg)` front door that
does nothing when `init()` found no usable path. Resolve the path like
`config::path()` does, but from `XDG_STATE_HOME` then `HOME/.local/state`.

Every write opens with `OpenOptions::append(true).create(true)` and discards any
error. No buffering held across calls — an append per event is cheap at human tempo,
and a buffer loses exactly the lines you need when the app dies.

- [ ] **Step 4: Wire the call sites**

`log::init()` early in `main`, before the TUI. Then the events listed above, in
`src/watch.rs` (registered / event / tick outcome) and `src/main.rs` (list with
duration and count, store switch, registry load and save).

- [ ] **Step 5: Run to verify they pass, and read the real file**

```bash
cargo test --bin dextui 2>&1 | tail -3
cargo clippy --all-targets 2>&1 | grep -c warning
cargo build && ls -la target/debug/dextui
scripts/render-check.sh
cat ~/.local/state/dextui/log
```

Expected: tests pass, zero clippy warnings, and a log containing a watcher
registration, at least one `list` with a duration, and — after ~20s idle — safety
ticks recorded as `unchanged`.

- [ ] **Step 6: Document and commit**

Add the log's path and purpose to `README.md`'s troubleshooting section, and a note
in `CLAUDE.md` on why it is always-on, file-only, truncating, and silent on failure.

```bash
git add src/log.rs src/main.rs src/watch.rs CLAUDE.md README.md
git commit -m "feat: an always-on sync log, so the watcher can be debugged"
```
