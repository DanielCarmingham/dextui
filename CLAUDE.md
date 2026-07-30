# dextui

Conventions and hard-won gotchas for anyone changing this code. For what it is,
how to install it and what the keys do, see [README.md](README.md) — user-facing
documentation belongs there, not here.

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
| `src/icons.rs` | Glyph sets in three tiers (nerd / unicode / ascii). |
| `src/theme.rs` | Every colour, and nothing else. |
| `src/tree.rs` | Flat list → hierarchy, search and status filtering, row prefixes. |
| `src/app.rs` | All view state, the refresh-survival rules, and the header counts. |
| `src/ui.rs` | Immediate-mode rendering, and `selftest`. |
| `src/watch.rs` | Debounced FS events plus the safety poll. |
| `src/pulse.rs` | The animation clock, and the guard on its idle cost. |
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
f = TTFont('/Users/daniel/Library/Fonts/NerdFonts/FiraCodeNerdFont-Regular.ttf')
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

## Two panes, one set of movement keys

`Tab` moves focus between the tree and the detail pane; the focused one has the
brighter border. `j/k/h/l`, `g/G` and page keys drive whichever has focus, while
the action keys (`s c e n a d f / r ?`) stay global because they always operate
on the selected task.

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
animate = true          # breathe the in-progress marker
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

`e` renames (a single line, so a prompt is honest). `E` hands the **description**
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

## The pulse, and the guarantee it must not break

In-progress markers breathe between `ACTIVE` and a bold `ACTIVE_PULSE` on a
700ms half-period. **The glyph never changes shape** — only its colour — so the
marker column cannot jitter, and no new codepoint had to be measured.

The whole design constraint is the cost. This app redraws **only when something
changed** and idles at zero. So `pulse::poll_timeout` clamps the loop's timeout
to the next phase flip **only while at least one task is in progress and
`animate` is on**; otherwise it returns the original constant, on the same code
path. `App::is_animating` tests the `animate` flag *before* scanning the task
list, so the opt-out is genuinely free rather than merely quiet. Measured over
30s: 0.02s of CPU with it off, 0.27s with it on.

Three things to preserve if you touch this:

- **No reader thread.** The epoch is an `Instant` on the main thread. A thread
  blocked in `event::read()` would swallow the first keystroke meant for
  `$EDITOR`, which is the reason the loop polls at all.
- **`Instant`, not `SystemTime`** — an NTP step or a laptop waking from sleep
  must not jump the phase.
- **The clamp is pinned by an exact equality**, not an inequality. The 100ms poll
  already dominates a 700ms half-period, so the clamp looks like dead code and
  only matters in the last 100ms before a flip. An inequality would still pass
  with the clamp deleted; `poll_timeout(true, 680ms) == 20ms` would not.

There is no spinner and there cannot be a braille one — see the glyph section.

## Mouse

Capture is enabled explicitly — `ratatui::init` only sets raw mode and the
alternate screen, so mouse reporting is opt-in and ratatui has no mouse layer of
its own; the events come from crossterm.

- **Drag the divider** to resize the panes, clamped to 20–80% so neither can be
  dragged away.
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

The trade-off: while captured, the terminal stops doing its own text selection.
Hold **Shift** to bypass it, as most terminals allow.

Two things this requires:

- The renderer publishes geometry (`divider_x`, `body_top/bottom`,
  `terminal_width`) each frame, so mouse maths is exact rather than re-derived
  from assumptions about the layout.
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
```

Every UI bug found in this project — a tree loading collapsed, shortcuts being
swallowed, a truncated filter label, centred help text — was invisible to the
compiler and to the tests, and obvious the moment a pane was captured. **Use it
after any change to `ui.rs` or the key handling.**

## Scope

In: browse, search, filter, start, complete, edit, create, subtask, delete.
Out (run these from the shell): `sync`, `import`, `export`, `plan`,
`archive`, and multi-project views. dextui shows the current directory's store
only.
