# dex-tui

A terminal UI for browsing and triaging [dex](https://dex.rip/) tasks. Rust,
with [ratatui](https://ratatui.rs) + crossterm. Two panes: task tree on the left,
full task detail on the right, with a search/filter bar on top.

> There was previously a .NET + Terminal.Gui implementation. It was replaced by
> this one and remains at the `v0.1.0-dotnet` tag. See
> [docs/rust-vs-dotnet.md](docs/rust-vs-dotnet.md) for the measured comparison
> and what the rewrite actually bought.

## Build, test, run

```bash
cargo build
cargo test
cargo clippy --all-targets

# Run against the dex store for whatever directory you are in.
# run.sh builds first, then execs the app; it deliberately does NOT cd into the
# repo, because dex resolves its store from the working directory.
cd ~/some/project && ~/Developer/DanielCarmingham/dex-tui/run.sh

./run.sh -n           # skip the build, run the last one
./run.sh -r           # release build
./run.sh --selftest   # print the data pipeline as text, no TUI
```

`--selftest` resolves the store, lists tasks, builds the tree under every filter,
and renders the detail pane as text. Use it whenever you change the data path,
and to check behaviour where no interactive terminal exists.

## Layout

| File | Purpose |
| --- | --- |
| `src/dex.rs` | The only module that knows dex exists: model, argv, JSON. |
| `src/tree.rs` | Flat list → hierarchy, search and status filtering, row prefixes. |
| `src/app.rs` | All view state, plus the refresh-survival rules. |
| `src/ui.rs` | Immediate-mode rendering, and `--selftest`. |
| `src/watch.rs` | Debounced FS events plus the safety poll. |
| `src/main.rs` | Event loop and key handling. |

Tests live in `#[cfg(test)]` modules beside the code they cover.

## Things about dex that will bite you

These were all discovered the hard way; do not re-derive them.

- **The store is not always `./.dex`.** Inside a git repo dex uses a repo-local
  `.dex` directory; outside one it silently falls back to a *global* store at
  `~/.config/dex/local`. Always resolve it with `dex dir` and watch whatever that
  reports. Writing to the wrong store pollutes the user's global task list.
- **The JSON mixes naming conventions.** `parent_id` and `created_at` are
  snake_case, but `blockedBy` and `blocks` are camelCase — in the same object.
  serde matches the snake_case ones on field names; the others need an explicit
  `#[serde(rename)]`.
- **There is no status field.** `Status` is derived: `completed` true means
  completed, otherwise a non-null `started_at` means in progress.
- **`dex delete` prompts interactively** when a task has subtasks, which would
  hang a TUI with no way to answer. `Dex::delete` always passes `--force`, behind
  our own confirmation dialog.
- **`dex complete` requires `--commit` or `--no-commit`** for tasks synced to
  GitHub/Shortcut, and refuses outright when subtasks are incomplete. We always
  send `--no-commit`, and offer a force retry when the error mentions subtasks.
- **Task ids are short slugs** (`b4d5gfpl`), not integers. Never assume ordering
  from them — `dex list --json` is sorted by id, which is meaningless to a reader,
  so `tree::build` re-sorts by priority, then creation time, then name.
- **A `dex` call costs ~180ms** (Node startup). Cheap on a change, unaffordable
  per keystroke — hence no `dex show` call when the selection moves.

## Architecture: reads vs writes

**Reads** are triggered by a `notify` watcher on the store directory, but the
watcher only reports *that* something changed. The actual data always comes from
`dex list --json --all`. This keeps us off dex's private on-disk format while
costing nothing at all when the store is idle. A 10s safety poll backstops it,
because macOS can drop events for atomic-rename writes.

**Writes** always shell out to the dex CLI — never to `tasks.jsonl` directly — so
dex's validation and its GitHub/Shortcut sync hooks always run.

Arguments go through `Command::args`, never a shell and never a concatenated
string. Task names and result text routinely contain quotes, apostrophes,
ampersands and newlines.

Filtering is client-side: we always fetch `--all` and filter in memory, so
changing the filter is instant and costs no process spawn.

## The invariant: refresh must never disturb the user

This is the core product requirement, not a nicety. A refresh may never move the
selection, collapse an expanded node, or interrupt typing.

- Selection and expansion are keyed by **task id**, held in `App`. Immediate-mode
  rendering means there is no widget state to restore — the tree is rebuilt from
  scratch every frame.
- A deleted selection falls back to its nearest surviving **sibling**, then an
  **ancestor**, then the first root. That is genuine product logic and is tested.
- New tasks always arrive **collapsed**, so an agent creating subtasks in the
  background cannot explode the tree under the cursor.
- **First load is the exception**: everything is new then, so `App::new` expands
  once up front. Skipping this opens the app onto a single collapsed root — it
  was a real bug in the .NET version.
- While a dialog is open, refreshes set `pending_refresh` and are applied on
  close. Never let one land mid-dialog.

## Verifying the UI

`cargo test` covers the data path. The UI needs a real terminal emulator: under a
bare pty (`script`, a pipe) capability queries go unanswered and you get no
usable frames.

**tmux works.** `scripts/render-check.sh` starts the app in a detached tmux
session on a private socket, optionally sends keys, and prints the pane:

```bash
scripts/render-check.sh                    # just render
scripts/render-check.sh "Down Down"        # navigate, then render
scripts/render-check.sh "f"                # cycle the filter
scripts/render-check.sh "?"                # open the help dialog
```

Every UI bug found in this project — a tree loading collapsed, shortcuts being
swallowed, a truncated filter label, centred help text — was invisible to the
compiler and to the tests, and obvious the moment a pane was captured. **Use it
after any change to `ui.rs` or the key handling.**

## Scope

In: browse, search, filter, start, complete, edit, create, subtask, delete.
Out (run these from the shell): `sync`, `import`, `export`, `plan`,
`archive`, and multi-project views. dex-tui shows the current directory's store
only.
