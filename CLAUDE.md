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

# Install it. ~/.cargo/bin is on PATH, so `dex-tui` then works anywhere, and you
# get the release build (1.4 MB, ~87ms of runtime startup) by default.
cargo install --path .
cd ~/some/project && dex-tui

# Development loop. cargo run preserves YOUR working directory, which is what
# matters here: dex resolves its store from the cwd, so running it from another
# project browses that project's tasks.
cargo run -- --selftest       # data pipeline as text, no TUI
cargo run -- --themes         # list palettes
cargo run -- --icons          # list glyph tiers
DEXTUI_ICONS=nerd cargo run   # Nerd Font icons + powerline header
DEXTUI_THEME=temperature cargo run
```

`cargo install` copies the binary, so it will not pick up code changes until you
run it again — use `cargo run` while iterating.

`--selftest` resolves the store, lists tasks, builds the tree under every filter,
and renders the detail pane as text. Use it whenever you change the data path,
and to check behaviour where no interactive terminal exists.

## Layout

| File | Purpose |
| --- | --- |
| `src/dex.rs` | The only module that knows dex exists: model, argv, JSON. |
| `src/markdown.rs` | Small markdown reader for descriptions; emits neutral emphasis. |
| `src/theme.rs` | Every colour decision, as swappable palettes. |
| `src/icons.rs` | Glyph sets in three tiers (nerd / unicode / ascii). |
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

## Display conventions

- **Progress rollups** appear on any task with descendants, as a three-state
  meter plus the raw fraction: done, in flight, untouched. Computed from the
  **unfiltered** task list, so hiding completed tasks does not make every meter
  read `0/n`. Leaves get no meter, because a rollup over nothing is meaningless.
- **Age** appears only on in-progress tasks (`47m`, `21h`, `3d`). Putting one on
  every row would bury the signal it exists to give. Under a minute is `now`, and
  renders as "just now" rather than "now ago".
- **Colour lives entirely in `src/theme.rs`.** `markdown` and `tree` describe what
  things *are*; `ui.rs` is the only module that decides how they look. Prefer
  named/indexed colours, which adapt to the user's terminal theme -- `Color::Rgb`
  looks identical everywhere but ignores their scheme and assumes a dark
  background (that trade-off is why `ember` exists and is not the default).
- **Markdown delimiters are kept and dimmed, never stripped.** Headings, list
  markers, quotes, backticks and `**` all stay on screen as dim markers. That is
  consistent across block and inline syntax, and guarantees no input character is
  ever dropped -- a round-trip test enforces exactly that.

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

Forcing a fallback is possible — Ghostty's `font-codepoint-map = U+25BE=Menlo` —
but it is fixing the symptom. Of the installed monospace fonts only Menlo has
those three glyphs at all, and even it is 0.978 cells. Using glyphs the font
already has is both exact and portable to whatever font someone else runs.

## Verifying the UI

`cargo test` covers the data path. The UI needs a real terminal emulator: under a
bare pty (`script`, a pipe) capability queries go unanswered and you get no
usable frames.

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
`archive`, and multi-project views. dex-tui shows the current directory's store
only.
