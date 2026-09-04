# Changelog

Notable changes to dextui, in [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
format. Versions follow [semantic versioning](https://semver.org/spec/v2.0.0.html),
applied the ordinary way despite the crate still being 0.x: a feature is a minor
bump, a fix-only release is a patch.

`dist` reads the section matching the version in `Cargo.toml` and uses it as the
body of the GitHub Release, so this file is also what a release *says*. See the
Releasing section of `CLAUDE.md` for where in the sequence it gets renamed.

Add entries under `## [Unreleased]` as changes land — `### Added`, `### Changed`,
`### Fixed`, `### Removed` — for anything a user could notice. **Leave that
section empty when there is nothing**, and put no placeholder text in it: dist
takes whatever sits under the heading as the release body, so a "nothing yet"
line would sail through the check that is supposed to catch a release with no
notes. Empty is what makes that check work.

Releases before 0.5.1 predate this file. Their history is in the git tags
(`git log v0.4.0..v0.5.0`), which is where it will have to stay: writing those
entries now would mean reconstructing intent from diffs months later, and a
changelog that guesses is worse than one that starts honestly.

Link definitions live here, above the first version rather than at the foot of
the file where the convention puts them. `dist` takes a section to be
everything between its heading and the next one, so anything trailing the
oldest entry is swallowed into that entry's release body — which is exactly
what happened the first time this was checked.

[Unreleased]: https://github.com/DanielCarmingham/dextui/compare/v0.5.2...HEAD
[0.5.2]: https://github.com/DanielCarmingham/dextui/releases/tag/v0.5.2
[0.5.1]: https://github.com/DanielCarmingham/dextui/releases/tag/v0.5.1

## [Unreleased]

### Added

- Copy to the clipboard. `y` opens a chooser for the selected task's id,
  title, description (as stored) or the whole detail pane as plain text, and
  clicking the title, the `id` row or the description in the detail pane
  copies that field directly. The text is sent with the terminal's OSC 52
  clipboard escape, so it reaches the clipboard of the machine you are
  looking at over SSH and through tmux, and to `pbcopy`, `wl-copy`, `xclip`
  or `xsel` as well when one is on `PATH`, for terminals that ignore the
  escape.

### Changed

- The help dialog keeps a blank row and two cells of space between its
  border and the text, at every scroll position, instead of pressing the
  text against the frame.
- The header names the store in bold yellow when dex has fallen back to its
  shared global store at `~/.config/dex/local`, so the one case where you are
  not looking at the current project's tasks says so. The sidebar's `global`
  row is marked the same way. A project's own store is unchanged, and the
  header's is decided from the store path rather than the label, so a project
  that happens to be named `global` is still a project.

## [0.5.2] - 2026-08-23

### Fixed

- Scrolling the repo sidebar no longer moves the cursor, and so no longer
  switches which repo the tree and detail panes show. The wheel now moves only
  the view, matching the task tree. Awkward with a trackpad, worse on touch.
- Modifier chords no longer fire the plain key's binding. `Ctrl-D` opened the
  delete confirmation, and so did `Alt-D` and `Ctrl-Alt-D`; `Ctrl-Q` quit,
  `Ctrl-W` toggled wrapping, `Ctrl-J`/`Ctrl-K` walked the tree, and `Ctrl-A`
  made a subtask. In the search and rename fields a chord typed its bare letter
  into the text — `Ctrl-A`, the reflex for "go to the start of the line", put an
  `a` into the task name. `Ctrl-Y` could confirm a delete.

### Changed

- Keys now resolve through a binding table to an action, rather than an ordered
  `match` on the key code. Chords are matched exactly, so an unbound one
  resolves to nothing by construction. No key changed meaning.

## [0.5.1] - 2026-08-15

### Added

- Matching sort and filter controls in the header, with `f`/`F` cycling the
  filter forward and back.
- The package version is shown in the help dialog.

### Fixed

- Two watcher tests raced the events macOS replays, and are now stable.
