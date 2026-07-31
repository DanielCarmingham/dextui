# Repo and worktree sidebar

**Status:** approved, not yet implemented
**Branch:** `add-repo-support`

A third pane, left of the task tree, listing registered repositories with their
git worktrees nested underneath. Selecting a worktree is what decides which dex
store the task tree and detail pane read.

This reverses a stated boundary. CLAUDE.md's Scope section currently reads
*"multi-project views… dextui shows the current directory's store only"* — that
sentence is part of what this changes, and it should be rewritten rather than
quietly contradicted.

## Why

dex resolves its store from the working directory, so every worktree of a repo
has its own separate task list. That is not a quirk to route around; it is how
dex works, and it means a branch's tasks live with the branch. What is missing is
any way to see across them without relaunching dextui somewhere else.

The shape already exists on this machine. `TaxCommHub` has five worktrees, two of
which hold dex stores; `dextui` has two. Nothing today can show that.

## Non-goals

**An embedded terminal per worktree is explicitly out of this spec**, and gets
its own. It is a much larger piece — `portable-pty` + `vt100` + `tui-term`, and
in the comparable tool (`wtcc`) 1,646 lines of session handling plus most of a
2,071-line event loop — and it collides with three of dextui's load-bearing
decisions:

- the event loop **polls** rather than using a reader thread, precisely so a
  blocked `event::read()` cannot swallow the first keystroke meant for `$EDITOR`;
- *"redraws only when something changed, idles at zero"* would become "redraws
  whenever the child writes";
- `e` hands the **whole** terminal to `$EDITOR`, mouse capture and all.

Those are solvable — `wtcc` solves them — but not as a side effect of adding a
navigation pane. The terminal spec depends on worktree selection existing, so it
follows this one naturally.

Also out: creating or removing worktrees, and anything to do with branches beyond
displaying their names. dextui browses; git manages worktrees.

## What was measured first

Recorded because each of these changed the design, and re-deriving them is waste:

- **`dex --storage-path <dir> list --json --all` reads any store.** dex has no
  multi-repo concept of its own; this global option is the entire mechanism this
  feature stands on.
- **It wants the `.dex` *directory*, not `tasks.jsonl`.** Pointed at the file it
  returns `[]` — an empty list, not an error. A wrong path here fails silently,
  which is exactly the failure mode worth a test.
- **81 git repos under `~/Developer`, but only 10 dex stores.** Discovery is a
  smaller problem than the repo count suggests, and scanning was rejected anyway.
- **Most worktrees have no store.** Three of `TaxCommHub`'s five do not. The
  sidebar must show them as ordinary rows, not as errors or omissions.
- **Worktrees are commonly `locked`.** All four of `TaxCommHub`'s are. The
  porcelain parser has to handle that attribute.

## Data model

Three units, each usable and testable on its own.

### `registry` — which repos to show

A writable list of repo paths at `~/.config/dextui/repos.toml`.

**Deliberately not `config.toml`.** That file is read-only to the app on purpose:
*"Persisting every toggle would mean turning wrap off for one wide table silently
changed your default forever, and it would clobber comments in a file you had
hand-edited."* The registry is app-owned state and the config is user-owned text;
keeping them in separate files means neither rule has to be qualified.

Failure is soft, matching how config already behaves: an entry whose path no
longer exists is reported in the status bar and skipped, never fatal. A repo
someone has since deleted must not stop the app starting.

### `worktree` — what is in each repo

`git worktree list --porcelain` per registered repo, parsed to
`{ path, branch, is_main, is_locked, is_detached }`. No dex involvement, so it is
cheap, and the parser is testable against captured porcelain output — including
the detached and locked cases, both of which occur here.

### `stores` — what is in each worktree

A cache keyed by worktree path. Unselected worktrees hold **counts only**; the
selected worktree additionally holds the task list that drives the tree and
detail panes. This is what keeps a ten-repo sidebar from meaning ten task lists
in memory and ten trees to rebuild.

## Registration

`a` registers, `D` unregisters.

`a` registers the **main repo**, not the worktree the cursor is in. Registering
from `add-repo-support` should give you `dextui` with both of its worktrees
underneath, not a lone detached entry — the main repo is the thing that has
worktrees, so it is the thing worth registering. `git worktree list` from any
worktree reports the main checkout first, so this needs no extra call.

Registering an already-registered repo is a **no-op with a status message**, not
an error. Pressing `a` twice is not a mistake worth an error dialog.

`D` unregisters only. It never touches the worktree, the branch, or the store —
this is a view, and a key that deletes work while looking like it hides a row
would be the worst thing in the app.

## Layout

`Focus` gains `Repos`, and the pane tabs become `1 2 3`.

```
≥110  │ repos │ tasks │ detail │
 ≥80  │ tasks │ detail │           repos still reachable with 1 or zoom
 <80  │ tasks │                    tabs 1 2 3
```

The repo pane sheds first because it is navigation rather than content: you
usually know which worktree you are in, and the way back is a keypress. This
extends the existing zoom ladder rather than introducing a second mechanism:
`single_pane_below` keeps its current meaning and default of 80, and a new
`repos_pane_above` (default 110) governs the rung above it. Setting either to
`0` disables that rung, matching how `single_pane_below = 0` already means
"always split".

**The ladder must stay monotone.** Widening the terminal may never remove a pane.
This is the same rule `header_sides` already enforces, and for the same reason:
`the_header_never_brings_back_what_it_has_already_dropped` exists because a
two-stage size calculation made an element reappear as the terminal narrowed.
The pane ladder is to be a single ordered list for the same reason.

Two dividers now, both draggable, each clamped so no pane can be dragged away.

### Keys

| key | does |
| --- | --- |
| `1` `2` `3` | jump to repos / tasks / detail |
| `j` `k` | move within the repo pane |
| `enter`, `l` | select the worktree, move focus to the task pane |
| `-` `+` | collapse / expand a repo (the existing keys) |
| `a` | register the current repo |
| `D` | unregister, with confirmation |

`D` acts on the repo the cursor is **under** — pressing it on a worktree row
unregisters that worktree's repo, since a worktree is not separately registered
and there is nothing else it could mean. It confirms first, reusing the existing
delete dialog, because the row it removes takes its whole subtree off screen.

## Reads, writes and refresh

Every dex invocation gains `--storage-path <worktree>/.dex`.

This is mechanical but touches every verb, and it is the one place where a bug
writes to the **wrong store** — a category of failure worse than a crash, because
it is silent and it is someone else's task list. It therefore goes through a
single `Dex::for_store(path)` rather than a `--storage-path` argument threaded
through call sites, so there is exactly one place that can be wrong and exactly
one place to test.

Refresh:

- **Selected worktree:** watcher plus the 10s safety poll. Unchanged from today.
- **Every other registered worktree:** watcher only. Its counts are re-read when
  its own store changes.
- **Startup:** all stores read **concurrently**, so ten stores cost ~180ms of
  wall clock rather than 1.8s.

**The known gap, stated rather than hidden:** if macOS drops an event for an
unselected worktree — which is the exact reason the safety poll exists — that
worktree's counts stay stale until it is selected. This is the deliberate price
of keeping idle cost at zero. Polling every store on the 10s cycle would close
it, at ten node spawns every ten seconds forever, and *"this app redraws only
when something changed and idles at zero"* would stop being true.

## Invariants this must not break

- **A refresh never disturbs the user.** Now including the worktree selection,
  which must survive a refresh exactly as the task selection does.
- **Per-worktree task selection is remembered** for the session, so switching
  between two worktrees returns the cursor to where it was in each. Without this
  the pane is tedious for the comparison it exists to serve.
- **Idle cost stays at zero.** No new timer, no new poll cadence.
- **Writes still go through the dex CLI**, never to `tasks.jsonl`.
- **Colour stays in `theme.rs`**; the sidebar introduces no new palette.

## Behaviour worth pinning

- A worktree with **no `.dex`** shows an empty tree, not an error. Creating a
  task there creates the store, which is what dex does anyway.
- Launching dextui in an **unregistered** repo still shows that store — today's
  behaviour — with a status hint that `a` would register it. The feature must not
  make the simple case worse.
- Registering a repo whose path has since been deleted is reported and skipped.

## Tests

- Porcelain parsing, against captured `git worktree list --porcelain` output
  covering main, linked, locked and detached worktrees.
- The pane ladder is monotone across every width from 40 to 200.
- A refresh leaves the selected worktree unchanged.
- Switching worktrees and back restores each one's task selection.
- A worktree with no store renders an empty tree and no error.
- `Dex::for_store` puts `--storage-path` on every verb — asserted through the
  existing `Runner` seam, which already exists to assert argv without running dex.
- Pointing a store at a `tasks.jsonl` rather than its directory is rejected
  rather than silently yielding an empty list.
- Registry round-trip, and a malformed entry skipped rather than fatal.

## Documentation to update when this lands

- **CLAUDE.md's Scope section**, whose "current directory's store only" sentence
  this contradicts.
- The **config section**, to introduce `repos.toml` and say why it is a second
  file rather than a section of the first.
- **README** keys and the layout description; the screenshot will want redoing at
  a width that shows three panes.
