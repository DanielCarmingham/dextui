# Status colours, glyphs and progress design

Bring dextui's status colours into line with the dex CLI, redesign the state
glyphs, extend the colour semantics to the progress meters, and add a small
amount of motion to in-progress tasks.

## Why

dextui and dex are used together, on the same tasks, in the same directory,
often on the same screen. Today they disagree about what colour a task is:

| state | `dex list` / `dex-report` | dextui today |
| --- | --- | --- |
| todo | `[ ]` yellow (ANSI 33) | `○` terminal default |
| in progress | `[>]` blue (34) | `◐` **yellow** |
| done | `[x]` green (32) | `✓` green |
| blocked | `[!]` red (31) — `dex-report` only | `×` red, **never used** |

Yellow means "not started" in dex and "running" in dextui. That is the whole
problem: the two tools are not merely different, they are contradictory.

Two further gaps: the meters use their own shading rather than the status
colours, so the one place progress is quantified does not participate in the
colour language; and `Status` has no `Blocked` variant, so the red `×` glyph and
`BLOCKED` colour already in the code are dead — nothing can ever produce them.

Sources: `dist/cli/formatting.js` (`getTaskStatusDisplay`) in dex 0.16.0, and
`~/.local/bin/dex-report` (`glyph` / `glyphcolor` / the stacked bar).

## Decisions

### Colour is dex's, and lands on the glyph only

`TODO` yellow, `ACTIVE` blue, `DONE` green, `BLOCKED` red — matching dex exactly.

Both dex and dex-report colour **only the status marker** (`cc($gcol; $g)`) and
leave the task name in the default foreground. dextui does the same today and
keeps doing it. This is what stops a mostly-unstarted tree from becoming a wall
of yellow, and it is the reason adopting yellow for todo is safe at all.

Tree connectors stay dimmed, as in dex-report, so the coloured marker reads as
the foreground of the row.

### Glyphs: a hollow shape that fills in

| state | nerd | unicode | ascii |
| --- | --- | --- | --- |
| todo | `󰜌` md-rhombus_outline U+F070C | `◇` U+25C7 | `-` |
| in progress | `` fa-play U+F04B | `►` U+25BA | `>` |
| done | `󰜋` md-rhombus U+F070B | `◆` U+25C6 | `x` |
| blocked | `` fa-ban U+F05E | `×` U+00D7 | `!` |

Hollow diamond fills in as the task completes, so the state change is carried by
the shape itself rather than by colour alone. In-progress deliberately breaks
the family: it is the one state that is *happening*, and a play marker says so.

The ascii tier's todo changes from a blank to `-`, so every state has a visible
marker in every tier.

**`►` U+25BA is not `▶` U+25B6.** `▶` is already the collapsed-node marker, and
the two are nearly indistinguishable at terminal size — a rendered comparison
confirmed this. They are told apart by colour and weight, not shape: the tree
marker is dim, the status glyph is coloured. Do not swap one for the other.

Every codepoint above was verified twice, per the project's rule: present in
FiraCode Nerd Font via fontTools, **and** resolving natively at exactly 1.00
cells via CoreText's `CTFontCreateForString`.

### Blocked becomes a real status

Derived in dex-report's precedence, which is not obvious and must be preserved:

1. `completed` → `Completed`
2. `started_at` is set → `InProgress`
3. at least one entry in `blocked_by` that **exists in the task set and is not
   completed** → `Blocked`
4. otherwise → `Pending`

A started task that is also blocked reads as in progress, not blocked — work is
happening on it, which is the more useful signal. A `blocked_by` id that is
absent from the set does not count as blocking; a dangling reference is not a
blocker.

This is an API change. Blocked-ness cannot be computed from `&self`, so
`Task::status()` is replaced by a free `dex::status(task, by_id)`, and `App`
caches a `HashMap<String, Status>` beside the existing `progress` map. Keeping
both `Task::status()` and the map would leave two sources of truth that could
disagree.

The map is computed from the **unfiltered** task list, exactly as the progress
rollups are: a blocker hidden by the current filter must still block.

**No new `Filter` variant.** A blocked task is a pending task, and
`Filter::Pending` continues to include it. Blocked is a display state.

### Meters carry the same colours

Green for done, blue for in flight, dim for untouched — the stacked bar from
dex-report, in the colours the rest of the UI now uses.

The unicode tier builds the bar from eighth-blocks (`▏▎▍▌▋▊▉█`, all verified).
The nerd tier composes it from the U+EE00–EE05 bar-segment kit that Nerd Fonts
3.3.0 added — open left cap, open middle, open right cap, then filled variants —
giving a properly capped, seamless bar. The ascii tier keeps whole-cell `#`,
`+` and `.`, since sub-cell precision has no 7-bit representation.

**Partial cells appear only at the outer edge**, where colour meets the dim
remainder, and only in the tiers that have partial glyphs. The done→active
boundary always snaps to whole cells. Rendering a true sub-cell colour boundary
would require setting a background (fg=green, bg=blue on `▌`), which introduces
backgrounds the colour policy avoids and would be inverted by the selected row's
styling.

The cell arithmetic is shared across tiers; only the glyph table and whether a
partial cell is available differ.

The `n/total` fraction stays. At seven cells a bar cannot distinguish 2/7 from
3/7, and for triage the exact count is the useful part.

Existing behaviour that must not regress: meters are computed from the
unfiltered list, and leaves get no meter.

### Motion: the shape holds still, the colour breathes

In-progress rows pulse between `Blue` and bold `LightBlue` on a ~700ms phase.
The glyph never changes shape.

This was chosen over the alternatives deliberately. The classic braille spinner
(`⠋⠙⠹`, what `ora` and therefore yarn/npm use) is **impossible here**: FiraCode
Nerd Font contains no braille at all, and CoreText resolves it to **Apple
Braille at 1.11 cells**. It looks fine in yarn because the spinner is one
transient glyph at the end of a line with nothing aligned beneath it. In dextui
the status marker sits in a column, so an 11%-oversized glyph would make the
whole tree jitter as tasks start and stop — the exact failure already documented
for `▾ ▸ ⊘`.

Nerd Fonts 3.3.0 *does* ship a genuine 6-frame arc spinner at U+EE06–EE0B, and
it is native at 1.00 cells. It is not used: it exists only in the nerd tier, and
a second animation model for one tier is not worth two code paths. It is
recorded in `icons.rs` as a known option.

**The cost is real and must stay bounded.** The event loop polls for input and
redraws only when something changed; idle cost is currently zero. The change is
to clamp the poll timeout to the next phase flip **only while at least one task
is in progress**. Idle cost stays exactly zero when nothing is running, and
becomes roughly 1.4 repaints/second when something is. Animation is not a frame
rate for the whole app — nothing else gains a tick.

Opt-out via `animate` in the config file (default `true`) and `DEXTUI_ANIMATE`,
following the existing defaults < global < project < env precedence.

### Header: one line

The header keeps its single row. Today's plain counts become coloured ones, and
an overall bar plus percentage appears when the terminal is wide enough:

```
dextui · my-project  ████░░ 62%  3 active  8 ready  2 blocked   priority  [ all PENDING active ]
```

The vocabulary is `dex status`'s, and **"ready" is new to dextui**: a ready task
is pending *and* not blocked, so `ready + blocked` equals the `pending` count the
header shows today. Splitting the old single number is the point — "8 pending"
hides whether those tasks can actually be picked up.

Degradation as width shrinks: drop the bar, then the percentage, then the word
labels. A second header line would cost a tree row permanently, and this is a
task browser — rows are the scarce resource.

### Selection: an accent gutter instead of inversion

The selected row gets a `┃` U+2503 gutter in the accent colour and a bold name;
`Modifier::REVERSED` is dropped. An unfocused pane dims its gutter.

This is the one decision that cannot be confirmed without looking at it. If the
gutter does not read strongly enough in either terminal mode, `REVERSED` is the
documented fallback — it is the only emphasis guaranteed to adapt, because it
inverts whatever is already there.

### `src/theme.rs` exists now

CLAUDE.md already asserts that "colour lives entirely in `src/theme.rs`". There
is no such file; six constants sit at the top of `ui.rs`. This work roughly
doubles the palette, so the module is created and the claim becomes true.
`theme.rs` holds colour only. `ui.rs` remains the only module that decides how
things look.

## Testing

Unit:

- status derivation: blocker incomplete → blocked; blocker completed → not
  blocked; blocker id absent → not blocked; started **and** blocked → in
  progress; the map is built from the unfiltered list.
- meter arithmetic: rounding, the existing "non-zero always gets at least one
  cell" rule, `done + active` never exceeding the width, and the partial-cell
  index at the outer edge.
- pulse phase derived from elapsed time, including that phase is stable within a
  window and flips across one.
- poll timeout is clamped only when a task is in progress, and left alone
  otherwise. This is the guard on idle cost and is the part most likely to rot.

Render (`TestBackend`, the primary check):

- every tier draws, including a blocked row and a pulsing row in both phases.
- the header stat row draws, and degrades rather than panicking at narrow
  widths — extending the existing narrow-pane tests.

By eye (`scripts/render-check.sh`), **in both light and dark**:

- yellow todo markers are legible on a light background. A dark-mode-only check
  is what produced two of this project's past colour bugs.
- the selection gutter reads as selection.

## Out of scope

Archived tasks (dex-report dims them; archive is out of scope for dextui), any
new filter, the nerd-tier arc spinner, and per-priority colouring.

## Task breakdown

1. `theme.rs`, and adopt dex's colours.
2. Redesign the glyph set across all three tiers.
3. `Status::Blocked`, and the `App` status map.
4. Meters in status colours, with sub-cell edges and the nerd bar kit.
5. Play glyph, colour pulse, and the bounded animation tick.
6. Header stat row.
7. Selection gutter.
8. Update CLAUDE.md and README.

1 and 2 come first; the rest depend on them and are otherwise independent.
