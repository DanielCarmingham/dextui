# dex-tui design

**Date:** 2026-07-26
**Status:** implemented (v1)

## Goal

A terminal UI for **browsing and triaging** dex tasks in the current directory.
It must stay current as tasks change underneath it — including while agents write
to the store — without ever disturbing what the user is doing.

## Decisions

### Platform: .NET 10 + Terminal.Gui v2

The starting assumption was Charmbracelet, which was discarded: those are Go
libraries, and nothing about the problem requires Go. `dex` is a Node CLI that
emits JSON, so the TUI can be written in anything. .NET is the owner's strongest
language.

Considered:

- **Terminal.Gui 2.4.17 — chosen.** A real TUI application framework: event loop,
  focus, mouse, `TreeView<T>`, dialogs. The only .NET option that is an app
  framework rather than a rendering library.
- **Spectre.Console 0.57.2.** Excellent rendering and prompts, but no focus model
  or persistent event loop. Right for pretty one-shot output, wrong for a
  navigable app you sit inside.
- **Consolonia.** Avalonia/XAML in the terminal. Interesting, far more niche, a
  heavier bet.

Risk accepted: the 2.x line churns hard and most online examples are v1. Mitigated
by pinning the version exactly and keeping all valuable logic out of the UI layer.

### Data flow: watcher triggers, CLI reads

Measured first: `dex list --json` costs **~180ms** (Node startup), and the store is
`tasks.jsonl` — line-delimited JSON.

Considered:

- **Poll the CLI on a timer.** Simple, but burns a process spawn forever even when
  nothing changes, and staleness is bounded only by the interval.
- **Watch and parse `tasks.jsonl` directly.** Instant and free when idle, but
  couples us to an undocumented on-disk schema and forces us to re-implement dex's
  own tree-building and filtering.
- **Watcher triggers, CLI reads — chosen.** A `FileSystemWatcher` reports *that*
  something changed (debounced 250ms); the data always comes from
  `dex list --json --all`. Zero cost when idle, ~400ms to reflect a change, and we
  never parse dex's private format. A 10s safety poll backstops missed macOS
  atomic-rename events.

**Writes always go through the dex CLI**, never the file, so validation and the
GitHub/Shortcut sync hooks run. Arguments use `ProcessStartInfo.ArgumentList`,
never a shell — task text routinely contains quotes, ampersands and newlines.

Filtering is client-side: we always fetch `--all` and filter in memory, so changing
the filter is instant and costs no process spawn.

The detail pane renders from the already-fetched list. `dex show` is never called,
because selection changes on every arrow key and 180ms per keystroke is unusable.

### Layout

Two-pane master/detail with a persistent search + status-filter bar, and a
shortcut bar along the bottom.

```
┌─ dex-tui — <project> ─────────────────────────────┐
│ / refresh_                        [all|pending|◐] │
├─────────────────────────────┬─────────────────────┤
│ ▾ ◐ Ship dex-tui v1         │ Wire up refresh     │
│   ▾ ◐ Core data layer       │ ─────────────────── │
│     ● ○ Wire up refresh     │ id/status/priority  │
│   ▸ ○ Terminal.Gui views    │ full description    │
├─────────────────────────────┴─────────────────────┤
│ s start  c complete  e edit  n new  a subtask  …  │
└───────────────────────────────────────────────────┘
```

## Components

| Type | Responsibility |
| --- | --- |
| `DexClient` | One method per dex verb. The only type that knows dex exists. |
| `IProcessRunner` | Test seam. Lets every verb's argv be asserted without running dex. |
| `DexTask` | The wire model. Every property mapped explicitly. |
| `TaskTree` | Flat list → hierarchy; query and status filtering; sibling ordering. |
| `Reconciler` | Pure function deciding view state after a refresh. |
| `TaskDetail` | Detail-pane text and the store label. In Core so it is testable. |
| `DexStoreWatcher` | Debounced FS events plus the safety poll. |
| `MainWindow` | Views, keybindings, dialogs, modal interlock. |

`DexTui.Core` has no Terminal.Gui reference at all.

## The core invariant

A refresh must never move the selection, collapse an expanded node, or interrupt
typing. Concretely:

- Selection and expansion are keyed by **task id, never row index**.
- A deleted selection falls back to nearest surviving **sibling**, then
  **ancestor**, then first root.
- New tasks arrive **collapsed**, so background agents cannot explode the tree.
- Refreshes arriving while a dialog is open are **deferred**, then applied on close.

All of this lives in `Reconciler` as a pure function, and is the most heavily
tested part of the codebase.

## dex behaviours that shaped the design

- The store is **not** always `./.dex`; outside a git repo dex uses a global store
  at `~/.config/dex/local`. Resolved via `dex dir` and watched accordingly.
- The JSON mixes snake_case (`parent_id`) and camelCase (`blockedBy`) in one object.
- There is no status field; it is derived from `completed` and `started_at`.
- `dex delete` prompts interactively with subtasks → always `--force`, behind our
  own confirmation.
- `dex complete` needs `--commit`/`--no-commit` when synced, and refuses on
  incomplete subtasks → we send `--no-commit` and offer a Force retry.

## Error handling

Preflight (`dex dir`, first list) runs before the TUI starts, so failures print a
plain message instead of a broken screen. After that, dex's stderr is surfaced in a
dialog; a failed refresh keeps the last good model rather than blanking the view;
malformed JSON is reported, not thrown.

## Testing

40 tests, all in Core:

- `DexClient` argv construction (including quotes/newlines) and JSON parsing,
  against a fake `IProcessRunner`.
- `TaskTree` hierarchy, ordering, filtering, orphans, and parent cycles.
- `Reconciler` selection and expansion rules.
- `DexStoreWatcher` against the real filesystem: fires on write, collapses bursts,
  silent when idle, safety-poll fallback, silent after disposal.

The UI layer is deliberately thin and not unit tested. `--selftest` exercises the
full data path as plain text. Anything visual must be checked in a real terminal:
Terminal.Gui negotiates capabilities at startup and renders nothing under a bare
pty, so automated visual verification is not possible.

## Out of scope for v1

`sync`, `import`, `export`, `plan`, bulk `archive`, and multi-project views — all
fine to run from the shell. Current directory's store only.

## Known follow-ups

- A headless render harness using `Application.ForceDriver` + `TestInputSource`
  would let the view layer be tested; deferred as undocumented and churn-prone.
- Priority editing and blocker management are not exposed; dex supports both.
