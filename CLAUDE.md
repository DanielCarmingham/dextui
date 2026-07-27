# dex-tui

A terminal UI for browsing and triaging [dex](https://dex.rip/) tasks. .NET 10 +
Terminal.Gui v2. Two panes: task tree on the left, full task detail on the right,
with a search/filter bar on top.

## Build, test, run

```bash
dotnet build DexTui.slnx
dotnet test  DexTui.slnx

# Run against the dex store for the current directory:
cd ~/some/project && dotnet run --project ~/Developer/DanielCarmingham/dex-tui/src/DexTui.App

# Print the whole data pipeline as plain text and exit (no TUI):
dotnet run --project src/DexTui.App -- --selftest
```

`--selftest` resolves the store, lists tasks, builds the tree under every filter,
and renders the detail pane as text. Use it whenever you change Core, and to
verify behaviour in environments where no interactive terminal exists.

## Layout

| Project | Purpose |
| --- | --- |
| `src/DexTui.Core` | All dex-facing logic. **No Terminal.Gui reference.** |
| `src/DexTui.App` | Terminal.Gui views, dialogs, keybindings. |
| `tests/DexTui.Core.Tests` | xUnit. Everything worth testing lives here. |

The split is deliberate: Terminal.Gui v2 is a young, fast-moving API, so anything
valuable is kept where that churn cannot reach it. **Put new logic in Core, not in
the view.** If you find yourself wanting to test something in `MainWindow`, that
is a sign it belongs in Core — this is how `TaskDetail` came to exist.

## Things about dex that will bite you

These were all discovered the hard way; do not re-derive them.

- **The store is not always `./.dex`.** Inside a git repo dex uses a repo-local
  `.dex` directory; outside one it silently falls back to a *global* store at
  `~/.config/dex/local`. Always resolve it with `dex dir` and watch whatever that
  reports. Writing to the wrong store pollutes the user's global task list.
- **The JSON mixes naming conventions.** `parent_id` and `created_at` are
  snake_case, but `blockedBy` and `blocks` are camelCase — in the same object. Every
  property in `DexTask` is mapped with an explicit `[JsonPropertyName]`; a blanket
  naming policy cannot work.
- **There is no status field.** `DexStatus` is derived: `completed` true means
  completed, otherwise a non-null `started_at` means in progress.
- **`dex delete` prompts interactively** when a task has subtasks, which would hang
  a TUI with no way to answer. `DexClient.DeleteAsync` always passes `--force`,
  behind our own confirmation dialog.
- **`dex complete` requires `--commit` or `--no-commit`** for tasks synced to
  GitHub/Shortcut, and refuses outright when subtasks are incomplete. We always send
  `--no-commit`, and offer a "Force" button when the error mentions subtasks.
- **Task ids are short slugs** (`b4d5gfpl`), not integers. Never assume ordering
  from them — `dex list --json` is sorted by id, which is meaningless to a reader,
  so `TaskTree` re-sorts by priority, then creation time, then name.
- **A `dex` call costs ~180ms** (Node startup). That is cheap on a change, and
  unaffordable per keystroke — hence no `dex show` call on selection change.

## Architecture: reads vs writes

**Reads** are triggered by a `FileSystemWatcher` on the store directory, but the
watcher only reports *that* something changed. The actual data always comes from
`dex list --json --all`. This keeps us off dex's private on-disk format while
costing nothing at all when the store is idle. A 10s safety poll backstops it,
because macOS can drop events for atomic-rename writes.

**Writes** always shell out to the dex CLI — never to `tasks.jsonl` directly — so
dex's validation and its GitHub/Shortcut sync hooks always run.

Arguments go through `ProcessStartInfo.ArgumentList`, never a shell and never a
concatenated string. Task names and result text routinely contain quotes,
apostrophes, ampersands and newlines.

## The invariant: refresh must never disturb the user

This is the core product requirement, not a nicety. A refresh may never move the
selection, collapse an expanded node, or interrupt typing.

- Selection and expansion are keyed by **task id, never row index** — indices shift
  the moment anything is added or removed.
- `Reconciler` is a pure function holding all of these rules, and is thoroughly
  tested. Change the rules there, not in the view.
- A deleted selection falls back to its nearest surviving **sibling**, then an
  **ancestor**, then the first root.
- New tasks always arrive **collapsed**, so an agent creating subtasks in the
  background cannot explode the tree under the cursor.
- While any dialog is open, refreshes set `_pendingRefresh` and are applied on
  close (`DrainPendingRefresh`). Never let one land mid-dialog.

## Terminal.Gui v2 notes

Pinned to **2.4.17**. The 2.x line churns hard and most examples online are v1 and
will not compile. Things already learned:

- `MessageBox.Query`/`ErrorQuery` take the `IApplication` as the **first** argument.
- `Key` has named constants only for letters, digits and control keys. Punctuation
  like `/` and `?` must be matched via `key.AsRune.Value`.
- **Window shortcuts must override `OnKeyDownNotHandled`, not `OnKeyDown`.** The
  focused view sees keys first, so an `OnKeyDown` override on the window never fires
  while the tree has focus. The "not handled" hook also makes typing in the search
  box work for free, since `TextField` consumes printable characters first.
- **`TreeView.AllowLetterBasedNavigation` must be `false`.** It defaults to on and
  silently eats every letter for type-to-jump, so all single-key shortcuts die.
- **`MessageBox` centres every line**, which destroys column-aligned text. Use a
  plain `Dialog` with a left-aligned view for anything tabular.
- **Fixed-width `Label`s truncate silently.** Keep every variant of a swappable
  label the same width or the longest one loses its tail.
- The first focusable view added takes initial focus. Call `SetFocus()` explicitly
  on the view that should own the keyboard.
- `TextView` is marked obsolete in favour of the separate `tui-cs/Editor` package.
  We suppress CS0618 in `MainWindow.cs`; revisit only if it is actually removed.
- Update views from a background thread **only** via `app.Invoke(...)`; touching a
  view directly from a `Task.Run` crashes.
- `TreeView<T>` uses reference identity, so a rebuild must `ClearObjects()`, re-add
  roots, then re-expand and re-select by id.

## Verifying the UI

Terminal.Gui negotiates terminal capabilities at startup (`[18t`, `[6n`, `[0c`).
Under a bare pty — `script`, a plain pipe — nothing answers and it renders **zero
frames**, so those tools are useless here.

**tmux works**, because it is a real terminal emulator. `scripts/render-check.sh`
starts the app in a detached tmux session on a private socket, optionally sends
keys, and prints the rendered pane:

```bash
scripts/render-check.sh                    # just render
scripts/render-check.sh "Down Down"        # navigate, then render
scripts/render-check.sh "f"                # cycle the filter
scripts/render-check.sh "?"                # open the help dialog
```

Every UI bug found so far — the tree loading collapsed, shortcuts being swallowed,
the truncated filter label, the centred help text — was invisible to the compiler
and to the tests, and obvious the moment the pane was captured. **Use it after any
change to `MainWindow`.**

## Scope

In: browse, search, filter, start, complete, edit, create, subtask, delete.
Out (run these from the shell): `sync`, `import`, `export`, `plan`, `archive --older-than`,
and multi-project views. dex-tui shows the current directory's store only.
