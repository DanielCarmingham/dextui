# dex-tui (Rust / ratatui)

A rebuild of dex-tui in Rust with [ratatui](https://ratatui.rs) 0.30 + crossterm
0.29, kept alongside the .NET version so the two can be compared directly.

The .NET implementation is tagged `v0.1.0-dotnet` on `main`.

```bash
cd ~/some/project
~/Developer/DanielCarmingham/dex-tui/rust/run.sh

./run.sh -n           # skip the build
./run.sh -r           # release build
./run.sh --selftest   # print the data pipeline as text, no TUI

cargo test --manifest-path rust/Cargo.toml
cargo clippy --manifest-path rust/Cargo.toml --all-targets
```

## Layout

| File | Purpose |
| --- | --- |
| `dex.rs` | The only module that knows dex exists: model, argv, JSON. |
| `tree.rs` | Flat list → hierarchy, search and status filtering, row prefixes. |
| `app.rs` | All view state, plus the refresh-survival rules. |
| `ui.rs` | Immediate-mode rendering, and `--selftest`. |
| `watch.rs` | Debounced FS events plus the safety poll. |
| `main.rs` | Event loop and key handling. |

Behaviour is identical to the .NET version, including every dex quirk it
handles: the store is resolved via `dex dir` (it is **not** always `./.dex`),
`blockedBy` is camelCase while everything around it is snake_case, `delete`
always passes `--force` because dex would otherwise prompt, and `complete`
always sends `--no-commit` and offers a force retry when subtasks block it.

## How it compares

Measured on this machine, against the same 9-task demo store.

| | Rust | .NET |
| --- | --- | --- |
| Startup (`--selftest`, best of 7) | 267 ms | 512 ms |
| Binary / build output | 1.4 MB, single file | 16.7 MB + runtime |
| Source lines (excl. tests) | 1537 | 1328 |
| Tests | 38 | 40 |

Two things worth reading carefully.

**Startup.** Both figures include the same ~180 ms `dex list` subprocess, so the
runtime's own share is roughly **87 ms vs 330 ms**. Real, but both are dominated
by dex itself, which is Node.

**The Rust version is slightly *longer*.** That surprised me and is the more
useful finding. Terminal.Gui supplied `TreeView`, dialogs, text input and a focus
system for free; ratatui supplies none of those, so tree rendering, the editable
text buffer, and all four dialogs are hand-written here. Rust bought a smaller,
faster, dependency-free binary — not less code.

## Where immediate mode genuinely helped

The .NET version needed a `Reconciler` largely because Terminal.Gui's `TreeView`
holds reference-identity state that has to be torn down and restored on every
rebuild — that machinery is what produced its worst bug, a tree that loaded fully
collapsed.

Here, the selection is just a `String` id in `App`, and the tree is rebuilt from
scratch each frame, so there is nothing to restore. What remains is only the
genuine product rule: when the selected task is deleted by someone else, fall
back to sibling → ancestor → first root. That is real logic and is still tested.

The collapse-on-first-load trap still exists, though, and is still handled
explicitly in `App::new` — it is a consequence of the product rule, not of the
framework.

## Verifying the UI

`cargo test` covers the data path; the UI needs a real terminal emulator, because
neither framework renders under a bare pty. Use tmux, as with the .NET version:

```bash
tmux -L t new-session -d -s r -x 120 -y 30 -c ~/some/project \
  ~/Developer/DanielCarmingham/dex-tui/rust/target/debug/dex-tui
sleep 3 && tmux -L t capture-pane -t r -p && tmux -L t kill-server
```

Verified this way: first render, arrow navigation updating the detail pane,
`f` cycling the filter, and a task created by a *separate process* appearing
automatically without moving the selection.
