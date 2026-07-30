# dextui

A terminal UI for browsing and triaging [dex](https://dex.rip/) tasks.

```
 dextui · my-project · ██████▍░░░ 62% · 2 active · 5 ready · 1 blocked   priority  [ all  PENDING  active ]
┌──────────────────────────────────────────┐┌───────────────────────────────────────────┐
│┃├▼ ► Ship v1                   ██▌░░░░ 1/7││Wire up the file watcher                   │
││ ├▼ ► Core data layer          ░░░░░░░ 0/2││► in progress · started 4h ago · priority 1│
││ │ ├  ► Wire up the watcher           4h ││                                           │
││ │ └  ◇ Parse the JSON                   ││██▌░░░░ 1/7  subtasks done                 │
││ ├  ◆ Keybindings                        ││                                           │
││ └  × Write the docs                     ││id        9pljm7cu                         │
│└  ◇ Long task title …                    ││blocks    Write the docs                   │
└──────────────────────────────────────────┘└───────────────────────────────────────────┘
 s start  c done  e rename  E edit  n new  a sub  d del  f filter  o sort  , config  ? help
```

Two panes: the task tree on the left, full detail on the right. It **refreshes
itself** whenever the dex store changes — including when an agent edits tasks
underneath you — without moving your selection, collapsing the tree, or
interrupting a dialog you have open.

## Requirements

- [`dex`](https://dex.rip/) on your `PATH`. Every read and write goes through it,
  so validation and any GitHub/Shortcut sync you have configured still run.
- A real terminal. Piping it somewhere gets you a blank screen.
- Optional: a [Nerd Font](https://www.nerdfonts.com/) for `icons = "nerd"`.

## Install

```bash
cargo install --path .
cd ~/any/project && dextui
```

`~/.cargo/bin` needs to be on your `PATH`. dextui reads the store for the
**current directory**, so run it from the project whose tasks you want.

While developing, `cargo run` preserves your working directory the same way.
`cargo install` copies the binary, so re-run it to pick up changes.

## Keys

| | |
| --- | --- |
| `↑ ↓` `j k` | move, or scroll the focused pane |
| `→ ←` `h l` | expand/collapse, or scroll sideways |
| `g` `G` | first / last |
| `tab` | switch pane (the focused one has the brighter border) |
| `z` `Z` | collapse / expand all |
| `/` | search names and descriptions |
| `f` | cycle filter — pending / active / all |
| `o` `O` | cycle sort / reverse it |
| `w` | toggle wrapping |
| `r` | refresh now |
| `,` | edit the config in `$EDITOR` (created if missing, reloaded on save) |
| `?` | help |
| `q` `esc` | quit |

Acting on the selected task:

| | |
| --- | --- |
| `s` | start |
| `c` | complete (prompts for a result) |
| `e` | rename |
| `E` | edit the description in `$EDITOR` |
| `n` | new top-level task |
| `a` | new subtask of the selection |
| `d` | delete, with confirmation |

**Mouse**: drag the divider to resize the panes, wheel scrolls whichever pane is
under the pointer, click selects. Mouse capture means the terminal no longer
does its own text selection — hold **Shift** to select and copy as usual.

## Reading the display

- **`◇` todo · `►` in progress · `◆` done · `×` blocked.** The hollow shape fills
  in as a task completes; in progress breaks the family because it is the one
  state that is *happening*, and it breathes gently so you can find it. Colours
  match the `dex` CLI exactly — yellow, blue, green, red — so the two tools never
  disagree about what a task is.
- `██▌░░░░ 1/7` on a parent: subtree progress — done, in flight, untouched — from
  the **unfiltered** task list, so hiding completed tasks does not zero it.
- `4h` on an in-progress task: how long it has been in flight. Only in-progress
  tasks get one, so it stays a signal rather than noise.
- The **header** counts what you can act on: `2 active · 5 ready · 1 blocked`.
  *Ready* means unstarted with nothing in its way. A parent with unfinished
  children is neither ready nor blocked — you cannot pick up an epic — so those
  three numbers deliberately do not add up to the total outstanding.
- **Narrowing the terminal** sheds the header in order of what carries least: the
  bar, then the percentage, then the words behind the counts, then the filter
  menu collapses to just the active filter's name, then the sort order goes, then
  `dextui` itself. Which project you are in survives all of it, and is elided with
  a `…` rather than clipped silently if even that will not fit.
- The selected row is marked by a `┃` in the left margin rather than a
  highlight bar, so the status colours stay readable on it.

**Wrapping vs wide tables.** Wrapping and sideways scrolling are mutually
exclusive: wrapping removes the overflow there would be anything to scroll to.
Prose wants wrap on; a table wider than the pane wants `w` to turn it off, after
which `h`/`l` reach the rest.

## Configuration

Optional, and there are three ways to get a file to edit:

```bash
dextui config init            # write the template to the global config
dextui config edit            # open it in $EDITOR, creating it if needed
dextui config                 # print it instead, with both resolved paths
```

Add `-l` (or `--local` / `--project`) to act on the project file instead:

```bash
dextui config init --local    # write .dextui.toml at the git root
dextui config edit -l
```

Or press `,` inside the app, which opens the global file in `$EDITOR`, creating
it from the template first if it does not exist, and reloading it when you save.

Both files are **read-only** to the app otherwise — `w`, `o`, `O` and `f` change
only the current run, so nothing you toggle is written back over a file you have
edited.

Layered **defaults < global < project < environment**:

| layer | path |
| --- | --- |
| global | `~/.config/dextui/config.toml` |
| project | `.dextui.toml` at the git root |

```toml
sort = "priority"       # priority | updated | created | name
sort_reversed = false   # flips it: newest→oldest, updated→stalest
filter = "pending"      # pending | active | all
wrap = true
icons = "unicode"       # nerd | unicode | ascii
animate = true          # breathe the in-progress marker
```

Set `animate = false` if you would rather nothing moved. dextui only redraws
when something changes, so with it off the app costs nothing at all while you
are not touching it — and with it on, only while a task is actually running.

A project file need only mention what it changes:

```toml
# .dextui.toml — this repo has wide tables
wrap = false
```

`dextui config` prints both paths and whether each exists — the first thing to
check when a setting seems not to apply. `DEXTUI_ICONS` overrides the icon tier
for a single run.

## Command line

Subcommands follow dex's own shape, including `-l`/`-g`.

```
dextui                     Run the TUI (default)
dextui config              Show the config paths and a commented template
dextui config init         Write a config template
dextui config edit         Open a config in $EDITOR, creating it if needed
dextui icons               List the glyph tiers
dextui selftest            Print the data pipeline as text (no TUI)

-h, --help                  Show help
-V, --version               Show the version
-g, --global                Act on ~/.config/dextui/config.toml (default)
-l, --local, --project      Act on .dextui.toml at the git root
    --force                 With `config init`, overwrite an existing file
```

## Descriptions

Rendered as markdown — headings, lists, tables, fenced code — but written how you
like. A single newline stays a line break and leading indentation is preserved,
so a plain-text description that is not markdown at all still looks the way you
typed it.

`E` opens the description in `$EDITOR` (then `VISUAL`, then `vi`). Quitting
without changing anything writes nothing, so it will not touch `updated_at`.

## Troubleshooting

**Blank screen** — it needs a real terminal; it cannot render into a pipe.

**Wrong tasks** — dex resolves its store from the working directory, and falls
back to a *global* store outside a git repo. `dex dir` shows which one is in use.

**Tofu (`□`) instead of icons** — `icons = "nerd"` without a patched font. Use
`unicode`, or `ascii` if that still looks wrong.

**A setting seems ignored** — `dextui config` shows which files were found;
an unknown key or value is reported in the status bar at startup.

## Development

```bash
cargo test
cargo clippy --all-targets
cargo run -- selftest       # the whole data pipeline as text, no TUI

scripts/seed-demo.sh        # a throwaway store with a realistic task tree
scripts/render-check.sh     # render in tmux and print the pane
```

[CLAUDE.md](CLAUDE.md) documents the conventions and the traps worth knowing
before changing anything — several are non-obvious and were expensive to find.
