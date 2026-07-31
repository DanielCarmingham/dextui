# Handoff — the repo and worktree sidebar

**Branch:** `add-repo-support`, in the worktree at
`~/Developer/DanielCarmingham/dextui.worktrees/add-repo-support`
**State:** 27 commits from `main` (`64046dd`), 342 tests passing, clippy clean,
working tree clean. **Nothing has been pushed.** The branch has had a
whole-branch review and one fix wave; one finding is parked, see *Still open*.

```bash
cd ~/Developer/DanielCarmingham/dextui.worktrees/add-repo-support
cargo test && cargo clippy --all-targets   # both must be silent
scripts/render-check.sh                    # look at it
```

Use `/Users/daniel/.cargo/bin/cargo` from a non-interactive shell; plain `cargo`
may not be on `PATH`.

## What it does

A third pane, left of the task tree, lists registered repositories with their git
worktrees nested underneath. Selecting a worktree decides which dex store the task
tree and detail pane read — so a branch's tasks live with the branch, and you can
move between them without relaunching.

| key | does |
| --- | --- |
| `3` | focus the sidebar |
| `j` `k` `g` `G` | move within it |
| `enter` `l` | select the worktree, switch the store |
| `a` | register the repo you are in |
| `D` | unregister, with confirmation |

Three panes at ≥110 columns (`repos_pane_above`), two below, one below
`single_pane_below` (80). Widening never removes a pane —
`the_pane_ladder_is_monotone` pins that, for the reason `header_sides` already
had the same rule.

## Where things live

| | |
| --- | --- |
| `src/registry.rs` | the writable repo list, `~/.config/dextui/repos.toml` |
| `src/worktree.rs` | `git worktree list --porcelain`, parsed |
| `src/repos.rs` | registry + worktrees, flattened into rows |
| `src/log.rs` | the sync log |
| `Dex::for_store` | the one place `--storage-path` is built |
| spec | `docs/superpowers/specs/2026-07-31-repo-worktree-sidebar-design.md` |
| plan | `docs/superpowers/plans/2026-07-31-repo-worktree-sidebar.md` |
| ledger | `.superpowers/sdd/2026-07-31-repo-worktree-sidebar/progress.md` (gitignored) |

The ledger is worth reading before changing anything here. It records every
deferred finding, every place the plan was wrong and was amended mid-flight, and
the reasoning behind decisions that look arbitrary from the code alone.

## Four things that will bite you

**The registry is a second file on purpose.** `config.toml` is read-only to the
app — persisting toggles into it would silently change someone's defaults and
clobber a file they hand-edited. `repos.toml` is app-owned state. Do not merge
them.

**Writing to the wrong dex store is silent.** `dex --storage-path` pointed at a
`tasks.jsonl` instead of a `.dex` directory returns an **empty list**, not an
error — so a wrong store reads as an empty project. That is why every verb goes
through `Dex::for_store`'s single argv builder rather than a flag threaded
through call sites, and why it rejects a path ending `.jsonl`.

**Three separate data-loss paths were found and fixed here**, each only after the
previous one was closed: `main()` never called `Registry::load()` before saving; 
`load()` mapped every I/O error to "empty registry"; and `save()` was
truncate-then-write, which `parse()` then read as a legitimately empty registry.
It now writes a `.tmp` beside the target and renames. Treat any new write path to
this file with suspicion.

**`cargo build` has repeatedly reported `Finished` while leaving
`target/debug/dextui` hours stale.** `cargo test` rebuilds its own binary, so the
suite goes green while the pane you capture runs old code. This produced two
wrong screenshots and one wrong conclusion during this work. `cargo clean -p
dextui` is the fix; `ls -la target/debug/dextui` settles it.

## The refresh model, and why it changed

The spec accepted a staleness gap: unselected worktrees would be watcher-only,
because polling them all meant N node spawns every 10 seconds forever.

That turned out to rest on a false premise. `watch::spawn`'s safety tick emitted
**unconditionally**, and each emission costs a ~180ms `dex list` — so dextui was
already spawning one every 10 seconds while idle, with a single store, before
this branch existed. `CLAUDE.md`'s "idles at zero" measured the event loop and
redraws; it never covered that subprocess.

The tick now `stat`s `tasks.jsonl` (mtime, length, inode) and emits only on a real
change. An atomic rename — how dex writes — changes all three, so the dropped-event
case the net exists for is still caught. This is better than the spec asked for:
the net stays on **every** store, so the staleness gap is gone, and idle cost went
to zero for the selected store too.

The log shows it working:

```
10:35:00  watch  tick …/dextui-demo/.dex unchanged
10:35:10  watch  tick …/dextui-demo/.dex unchanged
10:35:15  watch  event …/dextui-demo/.dex
10:35:16  dex    list dextui-demo - 9 tasks 135ms
```

## The sync log

Always on, at `$XDG_STATE_HOME/dextui/log` (else `~/.local/state/dextui/log`),
size-capped by truncation at startup, and **silent on failure** — if it cannot be
written the app behaves exactly as if logging were off. A logger that can break
the program it exists to diagnose is worse than no logger.

It exists because the stat gate above added a branch that is invisible by design:
"ticked, nothing changed, did nothing" is correct behaviour and indistinguishable
from a broken watcher. The log is how you tell them apart. `tail -f` it while
reproducing.

## Still open

**The one worth deciding first.** The `?` help dialog clips silently below 80×36.
`HELP` is 31 lines and 76 columns; `centered()` clamps to the area. At 80×24 the
closing paragraph vanishes; at 60 columns a line truncates mid-sentence. There is
no scroll, no ellipsis and no "more" indicator, and the regression test only
renders 120×40.

This is *better* than before — the dialog was a fixed 74×16 against 31 lines, so
two thirds of it had never been visible at any size — but it bites the narrow and
phone use the README specifically sells. Parked as not load-bearing; it blocks
nothing and loses no data.

Everything else, in rough order of how much anyone would notice:

- **`app.worktree_counts` is write-only.** Counts are loaded concurrently at
  startup and kept current by per-store watchers, but nothing renders them, so
  the sidebar shows no task counts yet. The machinery is real and tested; the
  renderer is not wired to it.
- **A repo registered mid-run gets no watcher until relaunch.** The watcher
  channels are set up once in `main`. Inert while the point above holds.
- **A repo row cannot be collapsed** — `Repo::open` is read by the renderer but
  no key toggles it.
- `selected_repo_row` is not clamped after `a` re-sorts the list, so the sidebar
  cursor can land on a different repo than it was on. `unregister_repo_path`
  clamps for the mirror case; `a` does not.
- `registry::save` leaves a stray `.tmp` if the temp write fails partway (ENOSPC),
  and does not `fsync`, so the atomicity guarantee covers process crash and
  concurrent writers but not power loss.
- The `dex` log lines use two formats — `refresh()` logs a bare store label, the
  other three sites log the full directory. Grep by name still matches both.
- `std::os::unix::fs::MetadataExt` in `watch.rs` is the crate's first unix-only
  import; it no longer builds on Windows. No CI, no declared Windows support.
- `src/editor.rs` keeps its own `OnceLock<Mutex>` rather than using
  `src/test_support.rs`. Different variables, so no shared-state bug — just two
  mechanisms for one job.
- `src/config.rs`'s comment on `repos_pane_above = 0` says "never shows it",
  which now needs "as a third pane" — focusing the sidebar still shows it
  single-pane.

## If you continue this

Read `CLAUDE.md` first. Its rules are unusually specific because each one was
bought with a real bug, and this branch added to them. The two that constrain new
work here most: *a refresh must never disturb the user* (now covering the worktree
selection and the per-worktree cursor memory), and *colour lives in
`src/theme.rs`* — the sidebar deliberately introduced no new palette.
