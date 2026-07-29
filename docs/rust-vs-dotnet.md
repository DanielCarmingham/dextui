# Rust vs .NET: what the rewrite actually bought

dextui was first built in .NET 10 + Terminal.Gui 2.4.17, then rebuilt in Rust
with ratatui 0.30 + crossterm 0.29. The Rust version is now the project; the .NET
one is preserved at the **`v0.1.0-dotnet`** tag.

```bash
git show v0.1.0-dotnet          # the .NET implementation
git checkout v0.1.0-dotnet      # if you want to run or diff it
```

Both were feature-identical: same two-pane layout, same keybindings, same dex
handling, same refresh guarantees.

## Measured

On one machine, against the same 9-task demo store.

| | Rust | .NET |
| --- | --- | --- |
| Startup (`--selftest`, best of 7) | 267 ms | 512 ms |
| Binary / build output | 1.4 MB, single file | 16.7 MB + runtime |
| Source lines (excl. tests) | 1537 | 1328 |
| Tests | 38 | 40 |

Two things worth reading carefully, because the naive reading of each is wrong.

**Startup.** Both figures include the same ~180 ms `dex list` subprocess, so the
runtime's own share is roughly **87 ms vs 330 ms**. A real difference, but both
are dominated by dex itself, which is Node.

**The Rust version is slightly *longer*.** This is the more useful finding.
Terminal.Gui supplied `TreeView`, dialogs, text input and a focus system for
free; ratatui supplies none of those, so tree rendering, the editable text
buffer, and all four dialogs are hand-written. Rust bought a smaller, faster,
dependency-free binary — **not** less code.

## Where immediate mode genuinely helped

The .NET version needed a whole `Reconciler` type largely because Terminal.Gui's
`TreeView` holds reference-identity state that has to be torn down and restored
on every rebuild. That machinery produced its worst bug: a tree that loaded fully
collapsed, because the "new tasks arrive collapsed" rule also applied on first
load, where every task is new.

In the Rust version the selection is just a `String` id in `App`, and the tree is
rebuilt from scratch each frame, so there is nothing to restore. What survives is
only the genuine product rule: when the selected task is deleted by someone else,
fall back to sibling → ancestor → first root.

The collapse-on-first-load trap still exists though, and is still handled
explicitly in `App::new`. It is a consequence of the product rule, not of the
framework — which is exactly why it is worth knowing about.

## What each framework cost in surprises

Terminal.Gui v2 is a young, fast-moving API, and most examples online are v1 and
do not compile. Traps hit: `MessageBox` takes the `IApplication` first; `Key` has
no constants for punctuation; window shortcuts need `OnKeyDownNotHandled` rather
than `OnKeyDown`; `TreeView.AllowLetterBasedNavigation` defaults to on and
silently eats every single-key shortcut; fixed-width `Label`s truncate without
warning; `TextView` is already deprecated.

ratatui had essentially none of these, because it hands you far less: no focus
system to fight, no widget lifecycle, no key routing. The cost is that you write
those yourself, which is exactly where the extra 200 lines went.

## Verdict

For this program — a small, long-running, single-binary CLI companion — Rust wins
on distribution (1.4 MB, no runtime needed) and on the absence of framework
surprises. It did not win on brevity or on development speed, and it would not
have won at all if the app needed many more dialogs and form widgets.
