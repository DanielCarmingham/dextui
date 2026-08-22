# dextui

A terminal UI for browsing and triaging [dex](https://dex.rip/) tasks —
task tree on the left, full detail on the right, and, press `1` away, a
sidebar of every repo and its worktrees.

![dextui showing a task tree with progress meters and a selected row beside the detail pane for that task, the markers on the two in-progress tasks turning](docs/img/dextui-dark.gif)

<sub>Nerd Font glyphs; the default set works in any terminal. The turning
markers are the real thing at its real speed — the animation is captured from
the running app, not staged. Regenerate with `scripts/screenshot.sh`.</sub>

dex is a CLI task tracker. It is excellent at *writing* tasks and at being driven
by an agent, but reading a tree of them means running `dex list` again and again.
dextui is the reading half: one screen, always current.

It **refreshes itself** whenever the store changes — including when an agent edits
tasks underneath you — without moving your selection, collapsing the tree, or
interrupting a dialog you have open. That is the whole point of it, and the rule
the code is built around.

## Requirements

- **[dex](https://dex.rip/)** on your `PATH`. Every read and write goes through
  it, so its validation and any GitHub/Shortcut sync you have configured still
  run. `dex --version` should print something.
- **Rust 1.88 or newer**, but *only* if you build from source. The first three
  install routes below are prebuilt binaries and need no toolchain at all.
  `rustup` from [rustup.rs](https://rustup.rs) is the usual way in.
- **A real terminal.** Piping it somewhere gets you an explanation and exit 1.
  Use `dextui selftest` to see the data without one.
- Optional: a [Nerd Font](https://www.nerdfonts.com/) if you want the fancier
  glyph set. The default works in any terminal.

## Install

macOS and Linux, on x86-64 and arm64. Every route below puts a `dextui` binary
in `~/.cargo/bin`, so whichever you pick, that directory needs to be on your
`PATH`.

**Homebrew**

```bash
brew install DanielCarmingham/tap/dextui
```

**One line, no package manager** — the shortest way onto a box you have only
SSH'd into:

```bash
curl -LsSf https://github.com/DanielCarmingham/dextui/releases/latest/download/dextui-installer.sh | sh
```

**Cargo**, if you already have a Rust toolchain:

```bash
cargo install dextui              # compiles it, a couple of minutes
cargo binstall dextui             # or grab the same prebuilt binary, seconds
```

**From source**, which is what you want if you are going to change it:

```bash
git clone https://github.com/DanielCarmingham/dextui.git
cd dextui
cargo install --path .
```

Windows is not supported. It compiles, but `dex` will not launch and the
config paths do not resolve; see the Windows notes in `CLAUDE.md`.

## Try it in 30 seconds

You do not need any tasks of your own yet. This seeds a throwaway store with a
realistic tree covering every state, and prints the directory it made:

```bash
scripts/seed-demo.sh ./demo
cd demo && dextui
```

Press `?` for help, `j`/`k` to move, `Tab` to switch panes, `q` to quit. `rm -rf`
the directory when you are done with it.

Then run it somewhere real:

```bash
cd ~/your/project && dextui
```

**dextui reads the store for the current directory**, so run it from the project
whose tasks you want. If the tasks look wrong, `dex dir` tells you which store is
actually in use — outside a git repo, dex falls back to a shared global one.
That is just where it starts, though — register other repos from inside the
app and switch between them without restarting; see
[Multiple repos](#multiple-repos) below.

## Reading the display

| marker | state |
| --- | --- |
| `◇` | todo |
| `⠋` | in progress — and it spins, so you can find it |
| `◆` | done |
| `×` | blocked |

The hollow shape fills in as a task completes. In progress breaks the family
because it is the one state that is *happening*. The colours — yellow, blue,
green, red — are dex's own, so the two tools never disagree about what a task is,
and only the marker is coloured, never the task name.

- **`██▌░░░░ 1/7` on a parent** is subtree progress: done, in flight, untouched.
  Computed from the *unfiltered* list, so hiding completed tasks does not zero it.
- **`4h` on an in-progress task** is how long it has been in flight. Only
  in-progress tasks get one, so it stays a signal rather than noise.
- **The header** counts what you can act on: `2 active · 5 ready · 1 blocked`.
  *Ready* means unstarted with nothing in its way. A parent with unfinished
  children is neither ready nor blocked — you cannot pick up an epic — so those
  three deliberately do not add up to the total outstanding.
- **The selected row** is marked by a `┃` in the left margin rather than a
  highlight bar, so the status colours stay readable on it.

Narrow the terminal and the header sheds what carries least first; which project
you are in survives all of it.

## Zoom: one pane at a time

On a narrow terminal the split gives way to a single pane, with `[1] [2]` tabs in
the header showing where you are. **This is what makes dextui usable on a phone**
— over SSH from Termius or Blink, in Termux, or in any terminal on a small
screen, where two panes would leave no room for either.

<table>
<tr>
<td><img alt="the task tree filling a 60-column terminal" src="docs/img/dextui-narrow.png"></td>
<td><img alt="the detail pane filling the same terminal after pressing enter" src="docs/img/dextui-narrow-detail.png"></td>
</tr>
<tr>
<td align="center"><sub>60 columns — <code>3</code> or <code>enter</code> →</sub></td>
<td align="center"><sub>← <code>2</code>, <code>tab</code> or <code>←</code> — the same terminal</sub></td>
</tr>
</table>

Press `z` to zoom at any width — handy on a wide screen when you want a long
description full-width. Below `single_pane_below` columns (80 by default) it
zooms on its own; `z` still overrides that either way, and your choice sticks
until you press it again. Set the value to `0` to always split.

| key | does |
| --- | --- |
| `z` | zoom / unzoom |
| `1` `2` `3` | jump to the repo sidebar / the tree / the detail |
| `enter`, `→` | open the detail (`→` from a task with no subtasks) |
| `←` `tab` | back to the tree |

The tabs are clickable, and the header sheds the same way the wide one does —
but the tabs are reserved before any of that, so the way back is never the thing
that disappears.

## Multiple repos

dextui can watch more than one repo at once — other projects, or other
worktrees of the one you are in. Press `1` to focus the sidebar, and moving
the cursor switches the tree and detail panes to whatever worktree it lands
on, the same way moving the tree cursor changes the detail.

![the repo sidebar focused beside the task tree and detail pane, listing the current repo under a "here" heading and a second registered repo and its worktrees under "saved"](docs/img/dextui-repos.png)

<sub>The same terminal as the shot at the top, after pressing `1`. The
shortcut strip follows the focus, since `a` means something different
here.</sub>

The sidebar has two sections. **`here`** is the repo you launched in, and it
sits there while it is still unsaved. **`saved`** is the list you can reach
from anywhere. `a` saves the repo dextui is running in — not whichever row
the cursor is on — which moves its row down from `here` into `saved`; `D`
unregisters an entry, with confirmation, and moves it back. Unregistering
only removes the sidebar row: nothing on disk, in the worktree, or in the
store itself is touched. `A` saves a repo you are *not* in, by path.

The sidebar starts hidden — the common case is one repo read from its own
directory, where it has nothing to add. Press `1` or `b` to show it, or set
`repos_open = true` to start every session with it open. Once shown, width
still decides how it's shown: wide enough — `repos_pane_above` columns, 110
by default — and it gets a third pane alongside the tree and detail;
narrower than that, it shares the width with the tree instead and the detail
yields; narrower still, focusing it zooms the app down to just the sidebar,
the same way the whole app zooms below `single_pane_below`.

Every registered repo is watched, not only the one on screen, so a change in
any of them is noticed as promptly as a change in the one you are looking at —
and each row carries its own outstanding count, read from that cache rather
than from a fresh `dex` call. Moving the cursor onto a store already read is
therefore instant; a store this run has not touched yet pays one `dex list`,
around 180ms, the first time.

| key | does |
| --- | --- |
| `1` | focus the repo sidebar |
| `b` | show / hide it |
| `j` `k` `g` `G` | move the cursor — and the store the other panes read |
| `enter`, `l` | follow it over to the tasks |
| `a` | save the repo dextui is running in, moving it into `saved` |
| `A` | save a repo by path, for one you are not in (`~` works) |
| `D` | forget a saved repo, with confirmation |

## Keys

Press `?` in the app for this list at any time.

| key | does |
| --- | --- |
| `↑ ↓` `j k` | move, or scroll the focused pane |
| `→ ←` `h l` | expand/collapse, or scroll sideways |
| `g` `G` | first / last |
| `tab` | switch pane (the focused one has the brighter border) |
| `enter` | open the detail pane |
| `1` `2` `3` | jump straight to the repo sidebar / the tree / the detail |
| `z` | zoom — one pane at a time |
| `-` `+` | collapse / expand all |
| `/` | search names, descriptions and ids (an id matches from its start) |
| `f` `F` | cycle filter forward / backward |
| `o` `O` | cycle sort / reverse it |
| `w` | toggle wrapping |
| `Ctrl-R` | refresh now (it refreshes itself; this is the escape hatch) |
| `,` | edit the config in `$EDITOR` (created if missing, reloaded on save) |
| `?` | help |
| `q` `esc` | quit |

Acting on the selected task:

| key | does |
| --- | --- |
| `s` | start |
| `c` | complete (prompts for a result) |
| `r` | rename |
| `e` | edit the description in `$EDITOR` |
| `n` | new top-level task |
| `a` | new subtask of the selection |
| `d` | delete, with confirmation |

`a` means something different with the repo sidebar focused — see
[Multiple repos](#multiple-repos).



**The header is clickable.** When there is room, sort and filter both show word
menus: click a sort or filter word to switch directly. When the terminal is too
narrow for the menus, each collapses to its current label; clicking a collapsed
label cycles it, and right-clicking the collapsed sort label reverses it.

**Mouse**: drag the divider to resize the panes; the wheel or a trackpad drag
scrolls whichever pane is under the pointer, content moving with your fingers in
all three; click selects, in the repo sidebar as well as the tree. Scrolling
only ever moves the view — in neither pane does the wheel change what is
selected, so a trackpad or touch scroll through the sidebar cannot switch which
repo you are reading. Clicking a sidebar row does switch, the same way moving
the cursor there does.
Clicking a task's **expand marker** — the `▾`/`▸` before its status glyph — opens
or closes it, the same thing `+`/`-` and the arrow keys do to the selection. It
selects that row too, so the cursor never ends up inside something you just
closed. The sidebar's markers are decoration: its repos are always open.
Mouse capture means the terminal no longer does its own text selection — hold
**Shift** to select and copy as usual.

**Wrapping vs wide tables.** Wrapping and sideways scrolling are mutually
exclusive: wrapping removes the overflow there would be anything to scroll to.
Prose wants wrap on; a table wider than the pane wants `w` to turn it off, after
which `h`/`l` reach the rest.

## Descriptions

Rendered as markdown — headings, lists, tables, fenced code — but written however
you like. A single newline stays a line break and leading indentation is
preserved, so a plain-text description that is not markdown at all still looks
the way you typed it.

`e` opens the description in `$EDITOR` (then `VISUAL`, then `vi`). Quitting
without changing anything writes nothing, so it will not touch `updated_at`.

## Configuration

Entirely optional — dextui works with no config at all. Three ways to get a file
to edit:

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
animate = true          # spin the in-progress marker
single_pane_below = 80  # below this width, one pane at a time (0 = always split)
repos_pane_above = 110  # at/above this, a shown sidebar gets a pane of its own (0 = never)
repos_open = false      # start every session with the sidebar shown
```

A project file need only mention what it changes:

```toml
# .dextui.toml — this repo has wide tables
wrap = false
```

Both files are **read-only** to the app — `w`, `o`, `O`, `f` and `F` change only the
current run, so nothing you toggle is written back over a file you hand-edited.

Registered repos live in their own file, `~/.config/dextui/repos.toml` — not a
third config layer, and not read-only like the two above. `a` and `D` write it
directly, which is exactly why it is kept separate: folding it into
`config.toml` would mean either giving up that file's read-only guarantee or
starting to persist every other toggle too.

Set `animate = false` if you would rather nothing moved. dextui only redraws when
something changes, so with it off the app costs nothing at all while you are not
touching it — and with it on, only while a task is actually running.

`DEXTUI_ICONS` overrides the icon tier for a single run.

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

## Troubleshooting

**"this needs a real terminal"** — it draws a full-screen interface, so it cannot
render into a pipe, a file, or a job with no terminal attached. `dextui selftest`
prints the same data as text.

**"dex is required"** — every read and write goes through the dex CLI. If the
message says dex *is* at a path but could not be started, dex itself is fine and
its interpreter is not: dex is a Node script, and a node upgrade can move the
runtime out from under it. Reinstalling dex under the current node fixes it.

**Wrong tasks** — dex resolves its store from the working directory, and falls
back to a *global* store outside a git repo. `dex dir` shows which one is in use.

**Tofu (`□`) instead of icons** — `icons = "nerd"` without a patched font. Use
`unicode`, or `ascii` if that still looks wrong. `dextui icons` shows all three.

**A setting seems ignored** — `dextui config` shows which files were found; an
unknown key or value is reported in the status bar at startup.

**A build error mentioning `edition2024`** — your Rust is older than 1.85.
`rustup update`.

**Watching does not seem to notice a change** — dextui keeps a running log at
`$XDG_STATE_HOME/dextui/log` (falling back to `~/.local/state/dextui/log`). It is
always on, so `tail -f` it while reproducing the problem: it records every
watcher registration, filesystem event, and 10-second safety-poll tick —
including the ticks that found nothing changed, which is otherwise invisible.
It also records each `dex list` with how long it took and how many tasks came
back, worktree switches, and registry loads/saves. The file is truncated (not
rotated) if it grows past 1&nbsp;MB, so history is limited to the current run.

## Scope

In: browse, search, filter, start, complete, edit, create, subtask, delete —
across every repo and worktree you register, not just the one you started
dextui in.

Out, because the CLI already does them well: `sync`, `import`, `export`,
`plan`, `archive`.

## Development

```bash
cargo test
cargo clippy --all-targets
cargo run -- selftest       # the whole data pipeline as text, no TUI

scripts/seed-demo.sh        # a throwaway store with a realistic task tree
scripts/render-check.sh     # render in tmux and print the pane
scripts/screenshot.sh       # regenerate the README images from real output
scripts/glyph-check.py      # check candidate glyphs against the actual font
```

Rust 1.88 or newer, edition 2024. CI runs the tests and `clippy -D warnings`
on Linux, the tests again on macOS — `watch.rs`'s backend differs per platform —
and a build on the declared minimum.

`cargo run` preserves your working directory, which matters here: dex resolves
its store from the cwd, so running it from another project browses that project's
tasks. `cargo install` copies the binary, so re-run it to pick up changes.

[CLAUDE.md](CLAUDE.md) documents the conventions and the traps worth knowing
before changing anything — several are non-obvious and were expensive to find.
It is long because it is mostly *why*; start with **Contributing: the short
version** at the top, which says what to run before submitting and points at
the one section covering whatever you are touching. Changes people can notice
get an entry in [CHANGELOG.md](CHANGELOG.md) under `Unreleased`.

## License

[MIT](LICENSE).
