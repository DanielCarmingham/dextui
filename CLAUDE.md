# dextui

Conventions and hard-won gotchas for anyone changing this code. For what it is,
how to install it and what the keys do, see [README.md](README.md) — user-facing
documentation belongs there, not here.

A terminal UI for browsing and triaging [dex](https://dex.rip/) tasks. Rust,
with [ratatui](https://ratatui.rs) + crossterm. Two panes: task tree on the left,
full task detail on the right, with a search/filter bar on top.

## Build, test, run

```bash
cargo build
cargo test
cargo clippy --all-targets

# Install it. ~/.cargo/bin is on PATH, so `dextui` then works anywhere, and you
# get the release build (1.4 MB, ~87ms of runtime startup) by default.
cargo install --path .
cd ~/some/project && dextui

# Development loop. cargo run preserves YOUR working directory, which is what
# matters here: dex resolves its store from the cwd, so running it from another
# project browses that project's tasks.
cargo run -- selftest         # data pipeline as text, no TUI
cargo run -- icons            # list glyph tiers
DEXTUI_ICONS=nerd cargo run   # Nerd Font icons + powerline header
```

`cargo install` copies the binary, so it will not pick up code changes until you
run it again — use `cargo run` while iterating.

`selftest` resolves the store, lists tasks, builds the tree under every filter,
and renders the detail pane as text. Use it whenever you change the data path,
and to check behaviour where no interactive terminal exists.

## Layout

| File | Purpose |
| --- | --- |
| `src/dex.rs` | The only module that knows dex exists: model, argv, JSON, status. |
| `src/worktree.rs` | Parses `git worktree list --porcelain` into `Worktree` rows. |
| `src/registry.rs` | The writable list of registered repos -- `repos.toml`, kept apart from the read-only config. |
| `src/repos.rs` | Flattens repos and worktrees into sidebar rows, mirroring `tree::visible_rows`. |
| `src/icons.rs` | Glyph sets in three tiers (nerd / unicode / ascii). |
| `src/theme.rs` | Every colour, and nothing else. |
| `src/tree.rs` | Flat list → hierarchy, search and status filtering, row prefixes. |
| `src/app.rs` | All view state, the refresh-survival rules, and the header counts. |
| `src/ui.rs` | Immediate-mode rendering, and `selftest`. |
| `src/watch.rs` | Debounced FS events plus the stat-gated safety net. |
| `src/pulse.rs` | The spinner clock, and the guard on its idle cost. |
| `src/log.rs` | The always-on sync log -- see below. |
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
- **There is no status field.** `Status` is derived, in dex's own precedence
  (`cli/status.js`): `completed` → `Completed`; else `started_at` set →
  `InProgress`; else an incomplete blocker → `Blocked`; else `Pending`. A started
  task that is *also* blocked reads as in progress, because work is happening on
  it — that is the more useful signal, and it is what dex does.
- **`blockedBy` is never cleared when a blocker finishes.** So the list alone
  says nothing: blockers must be resolved against the rest of the set and
  checked. `dex::is_blocked` mirrors dex's `isBlocked` exactly — ids absent from
  the set are skipped, as are completed blockers. Treating `!blocked_by
  .is_empty()` as blocked was a real bug: tasks showed the red marker for the
  rest of their lives. Only *direct* blockers count, which also means a blocking
  cycle in a hand-edited store cannot recurse.
- **dex contradicts itself about "blocked".** `cli/status.js` counts a parent
  with unfinished children as blocked; `dex list --blocked` counts only
  incomplete blockers. Measured across four real stores, five of the six tasks
  `dex status` calls blocked have no blocker at all — two of those stores hold no
  blocking relationship anywhere and it still reported some. We follow
  `dex list --ready` / `--blocked`, so "blocked" means one thing in the header,
  the row glyph, the detail pane and dex-report alike. See `App::counts`.
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
- **`--storage-path` wants the `.dex` directory, not `tasks.jsonl`, and says
  nothing when you get it wrong.** Pointed at the file, dex finds no tasks and
  returns an empty list rather than an error — a wrong store looks exactly
  like an empty project, discovered nowhere near the mistake that caused it.
  `Dex::for_store` is the *only* place that builds this flag, ahead of every
  verb's own argv rather than threaded through each call site, so no verb can
  forget it or misconstruct it; it also rejects a `.jsonl`-suffixed path
  outright, where the mistake can still be seen.

## Architecture: reads vs writes

**Reads** are triggered by a `notify` watcher on the store directory, but the
watcher only reports *that* something changed. The actual data always comes from
`dex list --json --all`. This keeps us off dex's private on-disk format.

A 10s safety net backstops the watcher, because macOS can drop notify events
for atomic-rename writes (write a temp file, rename it over the original —
exactly how dex writes `tasks.jsonl`). **This net used to fire blindly**: every
10s, whether or not anything had actually changed, it told the caller to
re-read, which meant a `dex list` call (~180ms of Node startup) on a fixed
timer for as long as the app ran. That is not "idle cost zero" — it is one
subprocess spawn every 10 seconds, forever, per watched store. It went
unnoticed because nothing had ever measured it; the app's other zero-idle-cost
claims (`pulse.rs`, the event loop's poll timeout) are all about redraw cost,
which this never touched.

It is now **stat-gated**: on each tick, `watch::stat` reads `tasks.jsonl`'s
modification time, length and inode (a few microseconds, no subprocess) and
only tells the caller to re-read when that fingerprint actually disagrees with
the one from the last tick. An atomic rename changes all three, so this still
catches the dropped-event case the net exists for — it just no longer pays for
a `dex list` to discover, almost always, that nothing happened. An idle store
now costs nothing until something really changes. This applies to every store
the app watches, not just the one currently selected — `watch::spawn_many`
gives each one its own copy of the same stat-gated net.

### A store that does not exist yet

You can watch a directory; you cannot watch one that is not there. dex creates
`.dex` on the first `dex create`, so a fresh project starts with nothing to
attach to, and `watch::attach` returns `None`. Three rules follow, each bought
by a real failure:

- **Whether a watcher can exist is asked every tick, not once at startup.**
  It used to be decided in `spawn` and never revisited, so a store created after
  launch was found by the poll — correctly, which is exactly what made this hard
  to see — and then stayed on the poll's interval for the whole life of the
  process, with nothing ever attaching to it. The timeout branch now re-attaches
  when it finds itself without a watcher, and drops one whose directory has gone,
  so a store deleted and recreated is watched again. `is_attached` exists
  purely so a test can assert this: nothing on screen distinguishes watched from
  polled, both keep the view *correct*, and only the latency differs — which is
  why being stuck on the poll went unnoticed in the first place.
- **While there is no watcher the tick is `DISCONNECTED_POLL` (1s), not
  `SAFETY` (10s).** A tick with no watcher does two stats and no subprocess, so
  the frequency is nearly free; and this is the interval a brand-new store's
  first tasks wait behind. Ten seconds of an empty pane reads as "it never loads
  anything" rather than as "it has not looked yet" — which is how this was
  reported.
- **The watcher *and* the stat baseline are both established before `spawn`
  returns**, not on the thread. The thread owns them afterwards, but taking
  either one there means it happens at some unknowable moment after the caller
  is already running. For the watcher that loses an immediate write to the poll;
  for the baseline it loses it altogether — the write lands *in* the baseline,
  and every later tick then correctly reports it as unchanged. With no watcher
  attached, nothing else will ever report it. This raced about one run in three
  under a loaded test suite before it was moved.

`StoreWatcher` therefore signals rather than owns: it holds a stop flag and a
sender used only to wake the thread out of `recv_timeout`, so a dropped guard
stops the thread *now*. Dropping it used to drop the notify watcher and leave
the thread polling forever, still able to trigger a refresh — and `switch_store`
drops one on every worktree change.

**Writes** always shell out to the dex CLI — never to `tasks.jsonl` directly — so
dex's validation and its GitHub/Shortcut sync hooks always run.

Arguments go through `Command::args`, never a shell and never a concatenated
string. Task names and result text routinely contain quotes, apostrophes,
ampersands and newlines.

Filtering is client-side: we always fetch `--all` and filter in memory, so
changing the filter is instant and costs no process spawn.

## The sync log

The stat-gated safety net above has a branch that is invisible **by design**: on
a quiet tick it decides nothing changed and does nothing at all. That silence is
correct, and indistinguishable from a broken watcher without something recording
it. `app.status` cannot be that something — it is one line, overwritten, with no
history — and once the alternate screen is up, stdout/stderr belong to the TUI,
so nothing can be printed there either.

`src/log.rs` is a small, **always-on** log at `$XDG_STATE_HOME/dextui/log`
(falling back to `~/.local/state/dextui/log` — resolved the same way
`config::path` resolves `XDG_CONFIG_HOME`, but state rather than config, because
a log is disposable and machine-local and must never sit beside the user's
hand-edited `config.toml`). It is on unconditionally, with no toggle: a sync
fault is exactly the kind of bug that will not reproduce on demand, so an
opt-in log would be off precisely when it was needed.

Four rules, all deliberate:

- **Failure is silent and total.** An unwritable path, a directory that cannot
  be created — `log::line` just does nothing. No `unwrap`, no propagated
  `Result`, no status-bar complaint. A logger that can break the program it
  exists to diagnose is worse than no logger.
- **File only, never stdout/stderr.** Once the TUI owns the terminal, those
  belong to it.
- **Truncated at startup, not rotated.** A log you `tail -f` while reproducing
  a fault does not need history; rotation is machinery for a problem this does
  not have. Past 1 MB it is reset to empty on the next launch.
- **No buffering held across calls.** Every `line` call opens the file fresh
  with `append` + `create` and writes immediately. An append per event is cheap
  at the tempo these events happen, and a buffer would lose exactly the lines
  you need if the app died before it flushed.

`log::init()` runs once, early in `main`, before the terminal is touched, and
resolves the path into a process-wide `OnceLock` that every later `log::line`
call reads. Areas are `watch`, `dex`, `store`, `registry` — padded to a fixed
column so the file reads straight. What lands in each: `watch` gets every
watcher registration, FS event, and safety tick outcome (including
`unchanged`, the branch this exists for). A registration logged `(late)` is
one the tick made after the store appeared, which is the only visible sign the
re-attach above ever ran — a run of 1s `unchanged` ticks followed by
`registered … (late)` and then 10s ticks is what a store created after launch
looks like, and is how that fix was checked. `dex` gets every `list` call once
the terminal is up — `refresh()` (the function every watcher-triggered,
Ctrl-R-triggered and post-write reload goes through), the startup
worktree-counts join, the background per-worktree watcher thread, and a
worktree switch's own list all share one `log_list_outcome(store, result,
elapsed)` helper, logging the store *directory* rather than a display label so
a background store's chain is greppable end to end by the same string
`watch`'s `registered`/`event`/`tick` lines already use for it. Only the very
first `dex list`, in the startup preflight before the terminal is touched,
stays dark — a failure there is already printed to stderr and the process
exits before any of this exists to log it. `store` gets worktree switches,
from and to; `registry` gets every load and save, success or failure.

## Repos, worktrees and the registry

**`~/.config/dextui/repos.toml` is a second file, not a new key in
`config.toml`.** That file is deliberately read-only to the app — see
Preferences below — and folding the registry into it would mean either giving
up that guarantee or teaching `w`/`o`/`O`/`f` to persist too, since there
would be no longer a principled reason for `a` and `D` to write back while
those do not. `registry.rs` owns `repos.toml` the way `config.rs` owns the
other file — same XDG resolution, same "missing is normal, an unreadable file
is a hard stop rather than a silent empty registry" rule on load — but
write-side, because unlike every runtime toggle, a registration is meant to
survive the run that made it.

**`repos.toml` is written through a rename, never in place.** `Registry::save`
writes `repos.toml.tmp` beside the target and renames it over, which is atomic
within a filesystem — hence *beside*, since a temp on another volume degrades
to copy-then-delete and the guarantee is gone. `std::fs::write` truncates and
then writes, so a crash, a full disk or a second instance mid-write can leave
an **empty** file, and `parse` reads empty as a legitimately empty registry,
silently; the next `a` then persists one entry over everything that was there.
That is the third distinct route into the same silent data loss on this
feature — `load` refusing to treat an unreadable file as empty and
`add_and_save` re-reading before it writes were the first two — and it is the
one that also closes the concurrent-instance race properly rather than merely
narrowing it.

**Registering writes two things: the file and the row.** `a` used to do only
the first, so the repo it had just registered did not appear until the next
launch, against a README that promises switching between repos without
restarting. `register_repo` builds the `repos::Repo` from the `git worktree
list` the same keypress already ran, and re-sorts by path because
`Registry::add` keeps the file sorted — otherwise the row would move the first
time you restarted. Its per-store watcher, though, is still only set up at
startup: a repo registered mid-run has no watcher until the next launch.

**Every verb that can target a chosen store goes through `Dex::for_store` /
`for_store_with`** — see the `--storage-path` bullet above for why a wrong
store is silent, and why that is what makes this the *only* place the flag
gets built rather than a parameter threaded through each call site.
`repos::store_dir` is the one place that knows a worktree's store lives at
`<worktree path>/.dex`, and it is what every sidebar call site hands to
`for_store`.

**There is no staleness gap for a worktree you are not looking at.** The
original plan for this feature accepted one on purpose: poll only the
selected store right away, and let every other registered store go stale
until you switched to it, on the theory that watching all of them on a timer
would multiply the exact idle cost `watch.rs`'s safety net exists to control.
That trade stopped being necessary once the safety net itself became
stat-gated instead of firing blindly (see "Architecture: reads vs writes"
above) — at that point watching ten stores costs the same as watching one, for
as long as nothing changes in nine of them. `watch::spawn_many` gives every
sidebar store — the selected one included — its own notify watcher and its own
copy of the same stat-gated net, so a change in a worktree nobody is looking at
is picked up exactly as promptly as a change in the one on screen.
`App::store_tasks`, keyed by store directory, is where the result lands, and it
is read twice over: the sidebar draws each store's outstanding count from it,
and a store switch **is** a lookup in it rather than a synchronous `dex list`.
It held only counts at first, and was drawn nowhere — a write-only map, and
the reason a switch went on paying ~180ms for a list of tasks that had already
been fetched and discarded.

**A task list is tagged with the store it came from.** `refresh()` spawns a
thread holding a clone of the `Arc<Dex>` current at the time, and
`switch_store` replaces that `Arc` on the main loop — so a refresh spawned
just before a switch lands just after it. Untagged, that painted the *old*
store's tasks under the *new* store's label, which is the silent-wrong-store
failure this file treats as worse than a crash. `Msg::Tasks` carries the store
directory and `handle_msg` drops anything that is not `app.store_dir`. Both
`App::new` and `App::load_store` take that directory and derive `store_label`
from it rather than accepting a label, so the tag and the header cannot be set
to two different stores; and `refresh` takes the whole `&App` for the same
reason.

**How many panes fit and *which* panes they are, are two questions.** They
used to be one: `repos_pane_fits()` answered both, so asking for the sidebar at
a width the app had already judged fits two panes added a *third* anyway,
cramming three into that room. `App::laid_out` now decides the set —

- sidebar not shown → tree, detail
- shown, and `repos_pane_above` columns available → all three
- shown, with room for two → **repos and tree; the detail yields**

The detail is what gives way, not the tree, because the sidebar's whole job is
choosing which store the *tree* shows: those two side by side is the pairing
that makes asking for the sidebar worth anything, and the detail is a keypress
away and the pane most often read rather than acted on.

`single_pane()` is stated generally as a consequence: focusing a pane the
layout has no slot for zooms it. That was previously written as a special case
about the sidebar; it now covers a displaced detail pane too, with no new rule.
`repos_pane_above = 0` therefore means "never as a *third* pane" rather than
"no repo pane" — `1` still reaches it, now as one of two.

`D`'s confirmation dialog reuses `Mode::Confirm` rather than adding a second
one: its `id` field carries a `repo:`-prefixed path instead of a task id, and
since a real dex id is a short slug with no colon in it the two can never
collide. Cheaper than a second dialog, and the one thing to keep in mind if
`Mode::Confirm` ever grows a third caller — the prefix trick stops being safe
the moment something else's id could contain a colon too.

**The sidebar cursor drives the panes, exactly as the tree cursor drives the
detail.** It used to take `enter`, which made two panes that look identical --
same list, same `┃` gutter -- behave differently. The justification was cost:
a switch meant a ~180ms `dex list`. That had quietly stopped being true. Both
paths feeding the sidebar already fetched the **whole task list** for every
store and threw it away, the startup join reducing `Ok(store_tasks)` to counts
and the watchers shipping `Result<Vec<Task>, String>` across the channel for
the handler to do the same. `App::store_tasks` keeps them instead, so
`switch_store` does no I/O on the common path -- a `Dex` swap and a lookup.

Three things that follow:

- **A cache miss shows an empty tree under the new label, never the old
  store's tasks under the new name.** A repo registered mid-run has no
  watcher and no cache entry, so it falls back to an async list. dex reports a
  wrong store as an *empty project* rather than an error, so a wrong store
  that looks plausible is the failure this whole area is designed against.
- **Watchers are one fleet over every sidebar store**, the selected one
  included, rather than `watch::spawn` for the selected store plus
  `spawn_many` for the rest. Two mechanisms for one job was survivable while a
  switch cost a list anyway; with the list gone, restarting a watcher per
  cursor move would have been the only thing left making it expensive. Nothing
  in `switch_store` touches a watcher now.
- **A click and the wheel follow the cursor too.** The old rule that a click
  must not switch stores was reasoned entirely from the ~180ms it would spend;
  that reason is gone, and consistency with the tree is what is left.

`b` shows and hides the sidebar at any width, the way `z` toggles zoom -- an
`Option<bool>` that outranks `repos_pane_above`, flipping the *effective*
state so the first press always does the visible thing. Hiding it while it has
focus returns you to the tree, since `Tab` never lands there; and `1` reveals
it, so `b` cannot be a way to lose the pane with no way to ask for it back.

**Each sidebar row's numbers escalate with the pane's width.** One rung is not
enough for a pane that now drags from twelve columns to half the terminal, so
`repo_stat_spans` walks a ladder: the outstanding count alone, then every state
as colour-coded numbers, then the task tree's own rollup meter alongside them.
Three rules it inherits rather than invents:

- **Richest first, and every rung a prefix of the one above**, so widening can
  only ever add. `header_sides` learned that the hard way — an element that
  came *back* as the pane narrowed made the layout look broken — and
  `the_sidebar_stats_escalate_with_the_room_and_only_ever_shed` checks every
  width from 0 to 40 rather than a few chosen ones.
- **Colour is the only key**, which it can afford to be because yellow, blue,
  green and red already mean todo, in progress, done and blocked everywhere
  else here. Bare numbers in those colours need no labels; zeros are omitted
  rather than drawn, as in the header.
- **The bar is `bar_spans`**, the same meter the task tree rolls up with — same
  arithmetic, same glyph tiers, same three colours — not a second thing that
  looks like one.

A caution learned drawing it: in the nerd tier the meter is U+EE00–EE05, which
`tmux capture-pane -p` dumps as blanks. A text capture will tell you the bar is
missing when it is on screen. Check the codepoints, not the transcript.

**The sidebar always carries the store being read**, registered or not. Without
that, launching anywhere unregistered showed an empty pane beside a full task
tree — a box saying "no repos" while you are plainly looking at one — and `a`
appeared to *create* the repo rather than to keep it. `main::current_repo`
builds that row at startup, `Repo::registered` is what marks it (a dim `·`, not
a colour: the four colours all mean task states and a fifth meaning here would
break that language), and the list is sorted by path either way so registering
moves the marker and never the row.

Three consequences, each of them a thing that would otherwise be silently
wrong:

- **`D` on the repo you are reading clears the mark instead of dropping the
  row.** Dropping it would take the store you are looking at off the sidebar
  and leave the pane contradicting the header. `D` unregisters an entry; it
  does not stop you looking at the tasks on screen.
- **`a` marks the row it already has**, rather than pushing a second one for
  the same repo.
- **The global store gets a row too.** Outside a git repo dex silently falls
  back to `~/.config/dex/local` — the single most confusing thing about it,
  which is why the sidebar naming it is worth the special case. It is not a
  repo: no worktrees, and its `path` **is** its store rather than a checkout
  with a `.dex` inside. `Repo::store` and `App::store_for_path` are the only
  two places that know, because `<path>/.dex` derived from it points at a
  directory that does not exist — which dex reports as an *empty project*,
  never as an error. `main::current_repo` decides which shape it is from the
  `.dex` suffix on what `dex dir` already said, rather than taking a second
  opinion from a fresh `is_dir` check that could disagree with dex itself.

One residual worth knowing about: `repos::Row`/`repos::rows` already support a
closed repo hiding its worktrees (`Repo::open`), the same way the task tree
collapses a node, but nothing currently sets `open` to anything but `true` —
every registered repo shows every worktree, always. There is no key bound to
toggle it yet.

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
  once up front. Skipping this opens the app onto a single collapsed root, which
  has been shipped as a bug once already.
- While a dialog is open, refreshes set `pending_refresh` and are applied on
  close. Never let one land mid-dialog.
- **A worktree switch is deliberately not a refresh.** Everything above is
  `apply_tasks` reconciling a new task list against the *same* store's ids.
  Switching stores loads an entirely different store's ids, so `App::load_store`
  follows `App::new`'s first-load rule instead — expand everything, do not try
  to resolve `self.selected` against ids that belong to nowhere — rather than
  reusing `apply_tasks` and letting `next_ids.contains(&sel)` succeed by
  coincidence on another store's slug. `task_memory`, keyed by worktree path,
  is what makes this feel like it kept your place anyway: `select_worktree`
  restores whatever was selected the last time you were in that worktree,
  immediately before `load_store` runs and would otherwise reset it.

## Display conventions

- **Progress rollups** appear on any task with descendants, as a three-state
  meter plus the raw fraction: done (green), in flight (blue), untouched (dim) —
  the same colours the status glyphs use, and dex-report's stacked bar. Computed
  from the **unfiltered** task list, so hiding completed tasks does not make
  every meter read `0/n`. Leaves get no meter, because a rollup over nothing is
  meaningless.

  The arithmetic is in `Bar`, shared by every tier; only the glyph table differs.
  Three rules earned their tests the hard way:

  - **A run that does not exist is never drawn.** Whole-cell snapping can push
    the bar past the done run's own rounding, and handing the leftover to
    `active` painted an in-flight cell for a task with nothing started — the
    meter contradicting the very glyph beside it. 987 inputs under total 199 hit
    this, and only in tiers without partial glyphs, so the default tier hid it.
  - **A non-zero run always gets a whole cell**, never a 1/8 sliver. One finished
    subtask out of a hundred is the most useful thing a meter can say.
  - **The sub-cell remainder sits at the outer edge only**, and the done→active
    boundary always snaps to a whole cell. A true sub-cell boundary would need
    `fg=green` on `bg=blue`, a background the colour policy forbids and which the
    selected row would invert.

- **The header splits "pending"** into `N active · N ready · N blocked`, each in
  its state's colour, with an overall bar and percentage when there is room. It
  degrades by what carries least: bar, then percentage, then the words — at which
  point the status glyphs stand in. A zero `active` or `blocked` is omitted
  rather than drawn as `0`. `ready + blocked` deliberately does **not** sum to
  `pending`; see `App::counts`.
- **Age** appears only on in-progress tasks (`47m`, `21h`, `3d`). Putting one on
  every row would bury the signal it exists to give. Under a minute is `now`,
  which everything that *suffixes* it puts through `ui::since` — "now ago" is not
  a duration and reads as a bug. The bare `now` is still what the tree rows show,
  where there is no suffix and the column has to stay narrow. Both the detail
  pane's `started …` line and its absolute timestamps go through the one helper,
  because when only the timestamps special-cased it the two contradicted each
  other about the same instant on the same screen.

### The header row is split, not overlaid

`draw_header` picks the two ends **as a pair** (`header_sides`) and then splits
the row into two `Rect`s, one per end. Both of those are load-bearing:

- Two `Paragraph`s over the *same* `Rect` — identity left-aligned, sort/filter
  right-aligned — is what this used to be, with only the counts reserving any
  room for the other. Below ~48 columns they overwrote each other: at 44 the
  store label was eaten and left a dangling `·`, at 36 the row read `dexA-Z`. A
  long project name did the same thing at 60. Splitting first means an overlap
  cannot be expressed, rather than merely being arithmetically avoided.
- Sizing the two ends *one after the other* looks like the obvious way to do it
  and is wrong. The right-hand block's steps are large — a 24-cell menu
  collapsing to a 3-cell name — so narrowing the terminal can free more room than
  the narrowing cost, and an element already dropped comes **back**: the app name
  was absent at 43 columns and present at 42. Nothing about a smaller window
  should reveal more. `header_sides` walks one ladder of (identity, right) pairs
  instead, and every element sits in a **prefix** of that ladder, so first-fit can
  only ever shed. `the_header_never_brings_back_what_it_has_already_dropped`
  pins it, and fails on the two-stage version.

The ladder's order is a claim about what each fact is worth: the store label is
in every rung (`identity_store`) because "wrong tasks" is this app's most common
confusion and the label is the only thing on screen that answers it; which filter
is active outlives both the menu around it and the sort order, being the one of
the three that changes *what you can see*; and the app's own name outlives none
of them, because you know what you launched. Every rung also leaves the counts
room to say something — without that the menu outbid them, and at 52 columns the
header showed the whole filter menu and no numbers while 44 showed `2 ready`.

**The known residual**: the counts can still shed one step as the terminal
*widens*, at the four rung boundaries, because a rung that costs more than the
width gained squeezes them. Making all three components jointly monotone means
requiring the counts' full width at every rung, which pushes the filter menu out
to ~78 columns — and further on a busy store, where the counts are wider. That
trade was not worth it; a percentage blinking out between 57 and 66 columns is a
smaller cost than losing the menu at 88. The elements that must not flicker do
not.

The right-hand block owns a **leading** space, so the left side can fill its own
`Rect` to the last cell without the two ending up flush — `2 readyA-Z` looked
exactly like the overlap this replaced.
- **Colour lives entirely in `src/theme.rs`.** `markdown` and `tree` describe what
  things *are*; `ui.rs` is the only module that decides how they look. Use only
  `Color::Reset` and the ANSI-16 names, which the user's terminal remaps —
  `Indexed` and `Rgb` are fixed values that cannot follow a theme that changes
  under the running app. A test walks `theme::ALL` and enforces this, so a new
  colour must be added to that list or it is unguarded.
- **Descriptions render through `tui-markdown`**, not a hand-rolled parser. It
  was swapped in because tables need column measurement and terminal display
  widths; the old parser skipped them, so tables appeared as raw `|---|` rows.
  Its **default stylesheet is not** light-mode safe, so `markdown::Adaptive`
  replaces it: the stock H1 is `on_cyan().bold().underlined()` — a background
  with no foreground, leaving the text in the terminal's default colour, which
  is white on cyan in dark mode and dark grey on cyan in light. Inline code is
  `white().on_black()`, equally wrong on a light background. Ours carries
  structure with modifiers and sets no backgrounds at all; a test walks every
  rendered span and fails on any background without a foreground. `highlight-code` is off — syntect costs 21 crates for syntax
  highlighting we do not use.

## What the detail pane shows, and why

Everything comes from `dex list --json`, which carries **all** of it including
`metadata` — so none of this costs an extra process spawn, and the rule about
never calling `dex show` on a keypress still holds.

- **Duration** (`took 4h 12m`) is derived from `completed_at - started_at`. Not
  stored by dex. Nothing here needs sync configured.
- **`updated_at`** renders only when it differs from created/started/completed.
  Every one of those bumps it, so without that guard the row just echoes
  whichever happened last — which is exactly how it looked before the guard.
- **`blocks`** is the reverse of `blockedBy`. A task holding up three others is a
  priority signal that `blocked by` alone cannot show.
- **`metadata.commit`** comes from `dex complete --commit <sha>` and is entirely
  local — it reads your git repo, no GitHub involved.

`metadata` can also carry `github`, `shortcut` and `beads` blocks, which only
appear once sync is configured. They are deliberately **not** modelled: serde
ignores unknown keys, so their presence is harmless and adding them later is
additive. The shapes are in dex's `src/types.ts`.

Fields that exist **only** on `dex show --json` — `ancestors`, `depth`,
`subtasks`, `grandchildren`, `isBlocked` — are not used. They would cost a
~180ms spawn per selection change, and we already derive equivalents from the
list (parent chain, progress rollups).

## Colour policy: the terminal owns it

There is no theme system, on purpose. Colour appears only where it carries
meaning — todo, in-progress, done, blocked — and everything else is left to the
terminal, so the app inherits whatever scheme the user runs.

**The four state colours are dex's own**, taken from `cli/formatting.js`
(`getTaskStatusDisplay`) and `~/.local/bin/dex-report`:

| state | | ANSI |
| --- | --- | --- |
| todo | yellow | 33 |
| in progress | blue | 34 |
| done | green | 32 |
| blocked | red | 31 |

That is not cosmetic. The two tools are used on the same tasks in the same
directory, often on the same screen, so disagreeing made them *contradictory*
rather than merely different — yellow meant "not started" in dex and "running"
here.

**Colour lands on the status glyph only, never the task name.** That is what
keeps a mostly-unstarted tree from becoming a wall of yellow, and it is what dex
itself does (`cc($gcol; $g)` in dex-report wraps the glyph and stops). Tree
connectors stay dimmed so the coloured marker reads as the row's foreground.

That is not just taste. This machine's Ghostty is configured
`theme = light:"...",dark:"..."`, so the terminal follows the macOS appearance
and **flips under the app at runtime**. Any palette with a fixed light-or-dark
assumption is wrong half the time. An earlier version shipped four palettes and
the default assumed dark, which produced two bugs invisible in dark mode:
`title: Color::White` rendered the task title white on white, and a fixed dark
selection band left the selected row dark-on-dark.

The rules — values in `src/theme.rs`, placement in `src/ui.rs`:

- Use only `Color::Reset` and the ANSI-16 names. The terminal remaps those per
  mode. `Indexed`/`Rgb` are fixed values and cannot adapt.
- Never `Color::White`/`Color::Black` for text — effectively fixed.
- The selected row carries a **`┃` accent gutter and a bold name**, never a
  background band. See below for why, and for what it replaced.
- `COLORFGBG` is unset here and there is no reliable way to detect the
  background, so do not try — adapt instead of detecting.

### The selected row: a gutter, not an inversion

`theme::ACCENT` paints a `┃` in a two-cell column reserved on **every** tree row
and drawn only on the selected one, and the name goes bold. An unfocused pane
drops to `ACCENT_DIM`.

`Modifier::REVERSED` used to do this job, and on the face of it it was the safer
choice: it inverts whatever the terminal's current colours are, so it cannot be
wrong in either mode. It was replaced because it inverts *everything on the row*,
including the two things that now carry meaning there — the coloured status glyph
and the three-colour progress meter. A selected parent's meter turned into a
solid green band, which is precisely the row where the colour language matters
most.

**This was decided by looking at it, not by reasoning about it.** The captures
were rendered to PNG in the user's actual Ghostty schemes (`GitHub Light
Default` / `GitHub Dark Default`), focused and unfocused, on rows carrying a
meter, a red blocked marker, and a completed strikethrough. If a future change
makes the gutter unreadable, **`REVERSED` is the documented fallback and
reverting to it is not a failure** — it is the only emphasis guaranteed to adapt.
If you do revert, invert the "no `REVERSED`" assertions in `ui.rs` rather than
deleting them.

Two things that look like the obvious implementation and are not:

- `List::highlight_style` is stamped across the whole row *after* the item
  renders, so it can only emphasise the meter and glyph along with the name —
  the same defect as the `REVERSED` it would be reintroducing.
- `List::highlight_symbol` narrows the item area by the symbol's width, while
  the right-hand meter gutter is measured against the full inner width. The
  meter would be pushed off the right edge.

Hence the gutter is the first span of each `ListItem`: `used` already sums every
span, so the right-alignment arithmetic stays self-consistent for free.
`state.select(...)` still has to be set — it is what scrolls the selection into
view and what keeps `tree_offset` truthful, which is what makes a click land on
the row actually drawn.

Magenta is the accent because it is the one ANSI hue with no other job here
(yellow, blue, green and red are the four states, cyan is inline code), so a
cursor can never be misread as a state. The unfocused pane dims *within* the hue
rather than to `DIM`, which is exactly the colour of the `│└├` connectors one
cell to the gutter's right. That difference is deliberately slight: focus is
already carried by the pane border, and the selection has to stay findable while
you are reading the detail pane.

## Glyphs: verify, never assume

**Check codepoints against the actual font before using them.** This is not
theoretical: `▾` U+25BE, `▸` U+25B8 and `⊘` U+2298 all looked like safe
box-drawing characters and were shipped as the tree markers — and none of them
exist in FiraCode Nerd Font. macOS silently substituted Lucida Grande and Apple
Symbols at 0.86–1.17 cells against a 1.00 cell, so the markers were a different
typeface at the wrong width. `✗` U+2717, `✘` U+2718 and `⊗` U+2297 are missing
too, which is why the unicode tier uses `×` U+00D7.

Two ways to check, both used here:

```bash
# Does the font contain it?
python3 -c "
from fontTools.ttLib import TTFont
f = TTFont('~/Library/Fonts/NerdFonts/FiraCodeNerdFont-Regular.ttf')
cm = set().union(*[t.cmap.keys() for t in f['cmap'].tables])
print(0x25BC in cm)"

# What does macOS actually substitute, and how wide?  (CoreText cascade list)
# See CTFontCreateForString + CTFontGetAdvancesForGlyphs.
```

Everything in `icons.rs` was verified this way and measures exactly 1.00 cells.

**Braille is unusable here, and this is the most tempting mistake.** FiraCode
Nerd Font contains *no braille at all*, so `⠋⠙⠹` — what `ora`, and therefore
yarn and npm, use for their spinners — falls back to **Apple Braille at 1.11
cells**. It looks fine in yarn because the spinner is one transient glyph at the
end of a line with nothing aligned beneath it. Here the status marker sits in a
column, so an 11%-oversized glyph makes the whole tree jitter as tasks start and
stop. Do not reach for it.

**Nerd Fonts 3.3.0 added two sets worth knowing**, both native at exactly 1.00
cells:

- **U+EE00–EE05, progress-bar segments** — open left cap / mid / right cap, then
  filled left / mid / right. Composed by position, they give the nerd tier a
  properly capped, seamless meter rather than seven stamped cells. In use.
- **U+EE06–EE0B, a real 6-frame arc spinner.** Deliberately **unused**: it exists
  in the nerd tier alone, and a second animation model for one tier is not worth
  two code paths. Motion is carried by colour instead, so the glyph never changes
  shape and the column cannot jitter.

**Write nerd glyphs as Rust escapes** (`"\u{f04b}"`), never literal characters.
Codepoints in the BMP Private Use Area (U+E000–U+F8FF) get silently stripped by
some tooling — it happened to this repo's own design doc, where the fa-play and
fa-ban glyphs vanished from a table while the Plane-15 md-rhombus pair survived,
and it read as a missing-font problem rather than lost bytes.

Forcing a fallback is possible — Ghostty's `font-codepoint-map = U+25BE=Menlo` —
but it is fixing the symptom. Of the installed monospace fonts only Menlo has
those three glyphs at all, and even it is 0.978 cells. Using glyphs the font
already has is both exact and portable to whatever font someone else runs.

## Zoom: one pane when there is no room for two

Below `single_pane_below` columns (80 by default, `0` disables it) only the
focused pane is drawn, filling the width. `Enter` opens the detail; `Left`/`h`
and `Tab` go back; `1` and `2` jump straight to a pane; `Right`/`l` also crosses
over, but **only from a leaf** — where it did nothing before — so its tree
meaning is untouched.

**`z` toggles it at any width**, which is what earns the feature its name: `z`
is tmux's zoom-pane, and that reflex is where the request came from. It cost
collapse/expand-all their keys, which moved to `-` and `+` — the better mnemonic
anyway, since minus closes and plus opens, where `z`/`Z` said nothing. `=` is
accepted for `+`, being the same physical key unshifted.

`App::zoom` is `Option<bool>`: `None` decides by width, `Some` is a manual
answer that **outranks it**. `toggle_zoom` flips the *effective* state rather
than a stored flag, so the first press always does the visible thing — toggling
a flag would appear to do nothing on a terminal the width had already zoomed.
The override deliberately survives a resize: pressing a key is a decision, and a
layout that reverted on its own would read as a fault.

Small screens are the point. The pair of README screenshots at 60 columns is
there because this is what makes the app usable over SSH from a phone.

The **`[1] [2] [3]` tabs** are the LazyGit/gitui idea, and they earn their place
only here: with every pane on screen there is nothing to navigate *to*, and the
numbers would be decoration on a row that already sheds elements to fit. There
are three because the repo sidebar is a pane you can be looking at *alone*;
with only two, the row you were on had no tab of its own while `3` worked
anyway, unadvertised. `TABS` is the single list `tab_spans` draws from and
`tab_zone` matches clicks against, so a tab cannot disagree with its key. They
are **reserved before the ladder runs** rather than competing inside it, so they
outlive the sort label and the filter menu — a rung that dropped them would hide
the only sign that there is a way back. Below `IDENTITY_FLOOR` columns of
remaining room they yield in turn, because the store label is the one thing the
ladder promises always survives.

Both tab states are the same width — `[1]` against ` 2 ` — so switching cannot
shove the rest of the header sideways. The keys work at any width even though
the tabs are only drawn in zoom mode: they were unbound, and refusing them with
both panes visible would be a rule to remember for no benefit.

`draw` must publish `terminal_width` **before** `draw_header`, because the header
is the earliest thing to ask `single_pane()` — it decides there whether to draw
tabs at all. With the assignment after, the first frame and every frame after a
resize used a stale width and drew the wrong thing for exactly one frame.

**`App::focus` is the whole implementation.** It already existed to say which
border is brighter; below the threshold it says which pane is *drawn* instead.
No new mode, no second source of truth, and the refresh-survival rules keep
working unchanged because the selection and expansion were never tied to
layout. `crossing_to_the_detail_and_back_keeps_the_selection_and_the_tree`
pins that.

Three things that are easy to get wrong:

- **`divider_x` must be zeroed**, not left stale from the last wide frame.
  `on_divider` is what makes a drag start, and a leftover x would be an
  invisible drag target down the middle of the screen.
- **The mouse handlers compare against `divider_x` to pick a pane**, so with it
  at 0 every click and wheel tick would land on the detail — including while
  looking at the tree. They go through `App::pane_at`, which answers "the one on
  screen" when there is only one.
- **The single-pane path returns early**, so dialogs have to be drawn on the way
  out. `draw_overlays` is shared by both layouts; forgetting it would make `?`
  and every confirmation silently do nothing on a narrow terminal.

The pane-crossing fallbacks are deliberately gated on single-pane mode. With
both panes visible, `Right` on a leaf moving focus would silently redirect
`j`/`k` to the other pane mid-walk through the tree — a surprise there, and the
entire point here.

## Three panes, one set of movement keys

`Tab` moves focus to the next pane and `Shift-Tab` to the previous, left to
right as they are drawn; the focused one has the brighter border. `j/k/h/l`,
`g/G` and page keys drive whichever has focus, while the action keys
(`s c e n a d f / r ?`) stay global because they always operate on the
selected task.

**The sidebar is in the cycle exactly when it is on screen.**
`App::focus_cycle` keys off `repos_pane_fits()` — the same predicate that
decides whether the sidebar is drawn as a third pane — so `Tab` can never land
on a pane that is not there, and the two cannot drift apart. `b` hiding the
sidebar therefore removes it from the cycle for free. The order matches
`[1] [2] [3]`, because two ways of reaching the same three panes disagreeing
about their order would be worse than either alone.

`Tab` used to alternate the tree and the detail only, deliberately, on the
grounds that its contract was "the other of two panes" and a third destination
would make it ambiguous which one it returned to. That held while the sidebar
was somewhere you visited with a dedicated key and left again; it stopped
holding once the sidebar drove the other two panes and earned a number of its
own. An ordered cycle answers the old objection rather than ignoring it — with
a direction, "back" is never in doubt.

**Both surfaces that advertise keys have to be kept honest, and a test does
it.** `SHORTCUTS` (the status strip) and `HELP` (the `?` dialog) name the same
keys to the same person, so `the_shortcut_strip_and_the_help_dialog_agree`
holds them against each other. The strip is focus-aware for exactly one
reason: `a` creates a subtask in the tree and *registers a repo* in the
sidebar, so `REPO_SHORTCUTS` replaces it there — one strip claiming `a sub`
beside a focused sidebar was not a shorter truth but a wrong one. The dialog
is sized from `HELP` itself rather than a constant; the constant was 16 rows
against ~30 lines of text, so two thirds of what `?` documented had never been
readable at any terminal size, and adding a line to `HELP` is not documenting
a key unless someone can see it.

**The help scrolls, because sizing it to `HELP` only moved the problem.** At 31
lines and 76 columns it still outgrows an 80×24 terminal, and `centered` clamps
the dialog to the frame — so the closing paragraph simply stopped existing, with
nothing on screen to say so, which is the same silent truncation the fixed
74×16 box was replaced for. `App::help_scroll` and `j/k`, arrows, page keys and
`g/G` fix the vertical half; folding the text to the dialog's width fixes the
horizontal half, where a 60-column terminal used to cut a sentence off at the
border. Three things are load-bearing:

- **Every other key still dismisses.** "Any key" was this dialog's whole
  contract for its entire life, and narrowing it to `esc`/`q` would be a rule
  to remember in exchange for nothing. `handle_help` intercepts the movement
  keys and falls through on the rest, and the hint row says which is which.
- **The `↑`/`↓` markers are computed from a fold, not from `wrapped_height`.**
  That helper deliberately over-estimates, because for the detail pane a wrong
  guess should err towards blank space you can scroll into — but here the same
  slack becomes a `↓` promising lines that do not exist, at the bottom of the
  text, which is the exact lie the markers were added to stop telling. `fold`
  does the wrapping itself, so the height is a count. It is also why the
  paragraph no longer uses `Wrap`: something has to own the line breaks, and
  it cannot be both.
- **`draw_help` writes back to `App`**, so `draw_overlays` takes `&mut App` and
  handles `Mode::Help` before the `match &app.mode` the other dialogs share —
  those two borrows cannot coexist.

`?` always reopens at the top (`App::open_help`): it is pressed by someone
looking for a key, and resuming halfway down hides the first ten of them. The
wheel scrolls the dialog while it is up, and every other mouse gesture is
swallowed rather than reaching the pane underneath — a click that moved the
selection behind a dialog is unasked-for movement you cannot even watch happen.

**`w` toggles wrapping, and that is not cosmetic.** Wrapping and horizontal
scrolling are mutually exclusive in ratatui: `Paragraph::scroll((y, x))` honours
the x offset only when wrapping is off, because wrapping removes the overflow
there would be anything to scroll to. Prose wants wrap on; a table wider than the
pane wants it off, where it clips cleanly and `h/l` reach the rest. With wrap on,
a wide table wraps mid-border and turns to noise.

`App::scroll_detail` ignores horizontal input while wrapping, and turning wrap
back on resets the offset — a stale one would hide content.

Vertical scrolling is clamped against `detail_content_height`, which the renderer
measures each frame and writes back (hence `draw` takes `&mut App`). Wrapped
height depends on pane width, so only the renderer knows it, and
`Paragraph::line_count` is private in ratatui 0.30. `wrapped_height` estimates it
by character wrapping and adds a small allowance: over-estimating only lets you
scroll into blank space, while under-estimating would make the last line
unreachable.

Changing selection resets the scroll, or you land halfway down a task you have
not read.

## Preferences

Layered **defaults < global < project < environment**, matching dex's own
precedence so both behave the same way in the same repository:

| layer | path |
| --- | --- |
| global | `~/.config/dextui/config.toml` |
| project | `.dextui.toml` at the git root |

`config` prints both paths and whether each exists; `config init` writes the
template (refusing to clobber without `--force`); `config edit` opens it in
`$EDITOR`; `-l`/`--local` targets the project file. `,` does the same for the
global file from inside the app, reloading on save.

The CLI uses **subcommands and `-l`/`-g`, deliberately mirroring dex**, since the
two are always used together and a second vocabulary would be friction for no
gain. `--project` is accepted as an alias for `--local` because it says what it
means.

Reloading is the one moment the file's values are meant to override the runtime
toggles — otherwise saving an edit would appear to do nothing.

Arguments are parsed properly rather than scanned with `.any()`. An unknown flag
is an **error**, not a fall-through into launching the TUI: `--help` used to be
ignored, and outside a terminal it panicked trying to initialise one. A test
walks the usage text and asserts every flag it advertises is actually accepted. A
project file need only mention what it changes; everything else falls through to
the global file. A bad value in one layer is reported and leaves the layer
beneath intact, rather than resetting to the built-in default.

The git root is found by walking up for a `.git` entry rather than shelling out
to git — no process spawn, and it matches worktrees too, where `.git` is a file
rather than a directory.

```toml
sort = "priority"       # priority | updated | created | name
sort_reversed = false
filter = "pending"      # pending | active | all
wrap = true
icons = "unicode"       # nerd | unicode | ascii
animate = true          # spin the in-progress marker
```

`animate` is the one setting that reaches the **event loop**, not just the
drawing. Turning it off restores the original poll timeout exactly, so idle cost
returns to zero rather than merely going quiet — see below.

**The file is read-only.** `w`, `o`, `O` and `f` affect only the current run and
are never written back. Persisting every toggle would mean turning wrap off for
one wide table silently changed your default forever, and it would clobber
comments in a file you had hand-edited.

Precedence is defaults < file < `DEXTUI_*` env, so `DEXTUI_ICONS=nerd` still
works as a one-off override. `config` owns that precedence; other modules just
receive a resolved `Config`.

Failure is deliberately soft. A missing file is normal and silent; a malformed
one, or an unknown value, is reported in the status bar and then ignored.
Refusing to start over a typo in a preferences file would be the worse failure.
Unknown *keys* are an error (`deny_unknown_fields`) rather than a silent no-op,
so a misspelled setting tells you instead of pretending to work.

Soft line breaks and indentation are **not** configurable: a single newline is
always a line break, and leading whitespace on an ordinary line is rewritten to
non-breaking spaces so markdown keeps it — which also stops four spaces becoming
an indented code block. Lines that begin a markdown block (list, quote, heading,
table, fence) keep their real indentation, since there it is structural and
rewriting it would flatten nested lists. Descriptions are frequently not markdown at all, and plain text should
survive as written — joining lines was a regression from the tui-markdown swap,
and a switch to re-enable a regression has no use.

## Editing, and why input is polled

`r` renames (a single line, so a prompt is honest). `e` hands the **description**
to `$EDITOR` — `VISUAL`, then `EDITOR`, then `vi`. The value may carry arguments
(`code -w`), so it is split rather than run through a shell.

This is why the main loop **polls** for input instead of running a reader thread:
a thread blocked in `event::read()` would swallow the first keystroke meant for
the editor, since both it and the child read the same terminal. Nothing is drawn
unless something changed, so the poll timeout bounds how quickly a store change
is noticed rather than acting as a frame rate.

The old flow prompted for the description in a one-line field. A multi-line
description showed only its first line while the cursor sat at the end of the
last, so typing appended to text you could not see.

Returning unchanged text writes nothing, so opening the editor and quitting does
not bump `updated_at`. A trailing newline alone does not count as a change —
editors add one habitually.

**Render-time transforms must never round-trip into a write.** Soft breaks and
indentation preservation build a local string inside `markdown::render`; the
edit path reads `by_id[…].description`, the raw task from dex. A round-trip test
proves the stored bytes are identical after an edit that changes nothing.

## The spinner, and the guarantee it must not break

In-progress markers **turn**, one glyph per `pulse::FRAME` (80ms). The frames
live in `icons`, per tier — braille `⠋⠙⠹…` for nerd and unicode, `*oO` for ascii
— and `pulse` only answers *which* frame, so the tier and the schedule stay
independent.

This replaced a two-frame colour breath, and it costs about **nine times more**:
12.5 repaints/sec against ~1.4. `a_running_store_repaints_once_per_frame` states
that as an exact number rather than a comment. Two things make it acceptable,
and both are the real invariants here:

- **It is paid only while a task is running.** `pulse::poll_timeout` returns the
  untouched `IDLE_POLL` otherwise, on the same code path, so an idle store costs
  exactly what it did before animation existed — 0.02s of CPU over 30s.
- **The opt-out reaches the event loop, not just the drawing.**
  `App::is_animating` tests the `animate` flag *before* scanning the task list,
  so switching it off restores the old wakeup schedule rather than leaving a
  fast loop redrawing a still glyph.

Three things to preserve if you touch this:

- **No reader thread.** The epoch is an `Instant` on the main thread. A thread
  blocked in `event::read()` would swallow the first keystroke meant for
  `$EDITOR`, which is the reason the loop polls at all.
- **`Instant`, not `SystemTime`** — an NTP step or a laptop waking from sleep
  must not jump the frame.
- **The still glyph is not one of the frames.** With animation off, or nothing
  running, the marker is `ic.active` — a play triangle, which reads as "in
  progress" with no motion to help it. A lone braille dot does not.
  `row_glyph` takes `Option<usize>` so "not animating" is a state rather than a
  frame index, and `with_animation_off_the_marker_is_the_still_glyph` pins it.

### Why braille, after it was rejected twice

The rejection was based on a correct measurement and a wrong inference, which is
worth keeping straight because the same trap is one grep away.

FiraCode Nerd Font contains **no braille at all** — 0 of its 11,992 codepoints —
so macOS substitutes Apple Braille, measured via CoreText at **1.111 cells**.
That number is right. What was inferred from it, that the marker column would
jitter, is not: a terminal lays out its own fixed grid and snaps each glyph into
one cell regardless of what the font's advance asks for. The same snapping is
why `done` (U+F070B, a *double*-width Material Design glyph at 2.000 cells) has
always looked correct.

So measure the font, then **verify against the terminal**. `scripts/glyph-check.py`
prints every candidate with `|` bars after it; if any glyph is genuinely mis-sized
the bars go ragged. That is the check that settled it.

If a terminal that honours the advance ever turns up, the fallbacks are ready:
`ASCII_SPIN` works in any font, and Nerd Fonts 3.3.0's 6-frame arc at
U+EE06–EE0B is native at exactly 1.000 cells — but exists in the nerd tier alone.

The ascii tier deliberately does **not** use the classic `-\|/`. That tier's
structure already owns most of it: `-` is `expanded`, `|` is the `gutter`, `.` is
`pending`, `>` is the still `active`. A spinner cycling through those puts
tree-drawing characters in the state column. `*oO` collides with nothing, and
swelling reads as motion as clearly as turning.

## Mouse

Capture is enabled explicitly — `ratatui::init` only sets raw mode and the
alternate screen, so mouse reporting is opt-in and ratatui has no mouse layer of
its own; the events come from crossterm.

- **Both dividers drag.** `App::divider_at` answers which one a column is on:
  the sidebar's edge or the tree/detail split. The sidebar's edge is tested
  first, because at the minimum width on a narrow terminal the two can be
  within a cell of each other and the one you cannot otherwise reach should
  win.

  The tree/detail split is a **percentage**, clamped to 20–80% so neither pane
  can be dragged away. The sidebar is a **width** in cells, clamped to
  `REPOS_WIDTH_MIN`..half the terminal — it holds names, not prose, so it
  neither wants nor needs to grow with the terminal, which is also why it is a
  `Length` in the layout rather than a share.

  **`set_split` has to subtract `repos_right` first.** The layout is
  `[Length(repos_width), Percentage(p), Fill(1)]`, so the divider lands at
  `repos_width + p% of W` — computing `p` from the raw column silently assumes
  the tree starts at the body's left edge. It does in two panes, and does not
  once the sidebar exists, so grabbing the divider jumped it a full
  sidebar-width (26 cells) to the right before the drag had moved anywhere.
  `dragging_the_split_puts_the_divider_where_the_pointer_is` states it as the
  distance between pointer and divider, which is the thing that was wrong.
- **`App::pane_at` must know about all three panes.** It answered `Focus::Tree`
  for the sidebar's own columns — they sit left of `divider_x` — so a click on
  a repo row moved the *task* selection and the wheel over the sidebar scrolled
  the tree, both of them the selection-disturbing behaviour this app exists to
  avoid. The renderer publishes `repos_right` exactly the way it publishes
  `divider_x`, and only at `Panes::Three`: a stale width from an earlier wide
  frame would be an invisible dead zone down the left of the tree. A click
  there **selects only** — switching store stays on `enter`/`l`, because a
  stray click must not spend a ~180ms dex call and replace both other panes.
- **Wheel** acts on the pane under the pointer regardless of focus, which is
  what people expect. Both panes slide their **content** with the gesture.

  Having the tree move its *selection* instead is the obvious implementation and
  reads as backwards. Mid-list the view does not move at all, so the only thing
  the eye can track is the cursor — and the cursor travels *against* the fingers
  while the detail pane's text travels with them. One drag, two directions, in
  panes an inch apart. `App::scroll_tree` moves the offset and the selection by
  the same delta, so the list slides and the cursor holds its screen row.

  The offset is clamped against the row count rather than the viewport height,
  which `App` does not know. Overshooting is harmless — the list widget pulls the
  offset back far enough to keep the selection visible, and the renderer writes
  the corrected value back into `tree_offset`. It also means **a list shorter
  than its viewport does not appear to scroll at all**, which is correct and is
  why this has to be checked on a list long enough to move: a first attempt at
  verifying it used an 11-row store in a 14-row terminal and showed nothing.
- **Click** focuses a pane, and in the tree selects the row under the cursor.
- **The header row is clickable**: a word of the filter menu picks that filter,
  and the sort label cycles on the left button and reverses on the right,
  mirroring `o` and `O`.

  `ui` publishes `header_zones` each frame by walking the spans it *actually
  drew*, so the degradation ladder is not restated anywhere — a rung that
  dropped the menu contains no filter words and therefore offers no zones. The
  block's vocabulary is closed and tiny, so matching a span's content against
  the sort label and `Filter::MENU` is exact rather than a guess.

  Finding exactly **one** filter word means the header fell back to naming the
  current filter with no menu around it; there is nothing to pick from, so that
  zone cycles instead. The zones are cleared while the search box owns row 0,
  or a click would act on a menu that is not on screen.

  A click that hits no zone must do **nothing** — not steal focus, not move the
  selection. `click_header` returns whether it acted, and a test pins it.

The trade-off: while captured, the terminal stops doing its own text selection.
Hold **Shift** to bypass it, as most terminals allow.

Two things this requires:

- The renderer publishes geometry (`divider_x`, `repos_right`, `body_top/bottom`,
  `terminal_width`) each frame, so mouse maths is exact rather than re-derived
  from assumptions about the layout.
- **Scrollbars are inset past the corners.** Handed a pane's whole `Rect`,
  ratatui draws the bar on its rightmost column — which is the border, corners
  included, so `┐` and `┘` became track and thumb glyphs and the `[n]` marker
  lost the corner beside it. `draw_scrollbar` applies a one-row vertical
  margin: the bar stays *in* the border, which is deliberate and costs no
  content width, but the frame survives.
- `tree_offset` persists the list's scroll offset across frames. Without it the
  offset restarted at zero every frame, and a click would address the top of the
  list rather than the row actually drawn.

Capture is disabled around `$EDITOR` and on exit; the child must own the
terminal completely.

## Sorting

`o` cycles the order, `O` reverses it, and the current one shows in the header.
Sorting is applied to **siblings at every level**, so the hierarchy, progress
rollups and expand/collapse keep working — a subtask never escapes its parent.

`reversed` flips each order's *natural* direction rather than meaning a blanket
ascending/descending, because the useful default differs per key: newest-first
for timestamps, lowest-number-first for priority, A-Z for names. That is why the
labels read `newest`/`oldest` and `updated`/`stalest` rather than showing arrows.

Two rules worth keeping:

- **Name is the final tiebreak in every order.** Without it, tasks with equal
  timestamps could swap places between refreshes, which looks like the tree
  twitching on its own.
- **A missing timestamp sorts last in both directions.** An absent date is
  unknown, not "oldest"; floating it to the top under one direction would be
  actively misleading.

## Verifying the UI

`cargo test` covers the data path. The UI needs a real terminal emulator: under a
bare pty (`script`, a pipe) capability queries go unanswered and you get no
usable frames.

**To look at it with real data**, seed a throwaway store:

```bash
scripts/seed-demo.sh          # prints where it went
cd <that dir> && dextui
```

It runs `git init` on purpose — outside a git repo dex writes to the *shared
global* store at `~/.config/dex/local`, which would pollute your real task list.

**Headless render tests are the primary check.** ratatui's `TestBackend` renders
a real frame into a buffer, so `ui.rs` has tests asserting the header draws, every
icon tier draws, and narrow panes do not panic. Prefer these — they are
deterministic and run in CI, unlike anything involving a terminal.

**tmux is the secondary check.** `scripts/render-check.sh` starts the app in a detached tmux
session on a private socket, optionally sends keys, and prints the pane:

```bash
scripts/render-check.sh                    # just render
scripts/render-check.sh "Down Down"        # navigate, then render
scripts/render-check.sh "f"                # cycle the filter
scripts/render-check.sh "?"                # open the help dialog

# The pane is 120x36 by default. Most of what this app gets wrong, it gets
# wrong only at a size it has to shed something at, so reach for these.
DEXTUI_RENDER_COLS=60 DEXTUI_RENDER_ROWS=20 scripts/render-check.sh "?"
```

Every UI bug found in this project — a tree loading collapsed, shortcuts being
swallowed, a truncated filter label, centred help text — was invisible to the
compiler and to the tests, and obvious the moment a pane was captured. **Use it
after any change to `ui.rs` or the key handling.**

**A wrong screen does not heal on its own, and `^L` is why that key exists.**
`terminal.draw` writes only the cells that changed since the frame *ratatui*
last drew. Corruption from outside the app — a terminal that drops output, a
multiplexer repainting a pane, another process writing over it — is therefore
invisible to it: those cells are already correct as far as its buffer knows, so
they are never rewritten. And since the app draws only when something it knows
about changes, nothing else brings them back either. The result is a screen
that stays wrong indefinitely while the state underneath is perfectly fine —
which reads as "clicking does nothing", because the click *did* land and the
cells that would show it were never repainted.

`Ctrl-L` sets `App::force_redraw`, and the event loop calls `terminal.clear()`
before the next draw, discarding ratatui's idea of what is on screen. Worth
knowing when comparing against a program that repaints unconditionally: one
that issues `Clear(ClearType::All)` every frame cannot get stuck this way at
all, so it can look perfect in a terminal where this app looks broken — which
is not evidence that the coordinates are at fault.

The binding **must stay above the unguarded `KeyCode::Char('l')`** that means
expand / cross to the detail: match arms are tried in order and that one does
not inspect modifiers. `ctrl_l_forces_a_redraw_and_is_not_swallowed_by_the_plain_l`
pins it, because the layout of a `match` is not something anything else checks
— the compiler warns here only by luck, and would say nothing if the shadowing
arm were reachable-but-wrong.

**Check the binary is actually fresh before believing a capture.** `cargo build`
has twice reported `Finished` while leaving `target/debug/dextui` hours stale:
`cargo test` rebuilds its own binary, so the suite goes green against new code
while the pane you capture is running old code. Deleting the binary does not
help — cargo restores the previous one from its fingerprint. `cargo clean -p
dextui` does. A build that reports `Finished` in under a second after a real
source change is the tell, and `ls -la target/debug/dextui` settles it. Both
times this happened it produced a confident and wrong conclusion.

`capture-pane -p` strips colour, so it cannot answer "is this the right colour"
or "does the selection read as selection". Add `-e` to keep the escapes, and
either read the SGR codes directly or render them — which is what the screenshot
script does.

**`scripts/screenshot.sh` regenerates the README image.** It seeds a throwaway
store, captures the pane *with* escapes, and draws it with the real font and the
real palette, so the picture is the app's own output rather than a photograph of
it — reproducible, and incapable of showing a colour the app does not emit.
`scripts/screenshot.py` does the ANSI-to-PNG half and is useful on its own for
looking at colour work. Needs Pillow on whichever `python3` is first on `PATH`.

## Scope

In: browse, search, filter, start, complete, edit, create, subtask, delete —
across every worktree of every repo you register, not just the one dextui
started in. This file used to end with "dextui shows the current directory's
store only"; the repo sidebar is exactly the feature that made that false, and
removing the sentence is the point of this section.

Out (run these from the shell): `sync`, `import`, `export`, `plan`, `archive`.
Registering a repo only teaches the sidebar about it — nothing about sync
configuration, importing or the rest becomes a dextui concern just because
there is now more than one store on screen.
