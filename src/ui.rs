//! Immediate-mode rendering. Everything is redrawn from `App` each frame.
//!
//! This is the only module that knows about colour: `markdown` and `tree` emit
//! neutral descriptions of what things *are*, and this module decides how they
//! look.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
use ratatui::Frame;

use crate::app::{App, Counts, Focus, HeaderZone, Mode, Panes};

/// Colour is used only where it carries meaning. Everything else is left to the
/// terminal, so the app inherits whatever scheme the user runs -- including a
/// light/dark switch at runtime -- instead of imposing its own. The values live
/// in `theme`; this module decides where they go.
use crate::theme::{
    ACCENT, ACCENT_DIM, ACTIVE, BLOCKED, CODE, DIM, DONE, PLAIN, TODO,
};

use crate::icons::Icons;
use crate::dex::{self, age, local_time, Status, Task};
use crate::tree::{self, Progress};

const SHORTCUTS: &str =
    " s start  c done  r rename  e edit  n new  a sub  d del  f filter  o sort  1 repos  , config  ? help";

/// What the strip says while the sidebar has focus.
///
/// The keys genuinely differ there: `a` registers a repo rather than creating
/// a subtask, `D` unregisters one, and none of `s`/`c`/`e`/`d` apply to a row
/// that is not a task. A single strip advertising `a sub` beside a focused
/// sidebar was not a shorter truth, it was a wrong one.
const REPO_SHORTCUTS: &str =
    " enter switch store  a register repo  D unregister  tab tasks  ? help";

/// Width of the inline progress meter, in cells.
const METER_WIDTH: usize = 7;

/// Fixed width of the repo/worktree sidebar in the three-pane layout. Repo and
/// branch names, not prose, so it does not need a share of the percentage
/// split the way the tree/detail boundary does. `repos_pane_above` (110 by
/// default) already guarantees the tree/detail remainder stays usable once
/// this much is taken off.
const REPOS_PANE_WIDTH: u16 = 26;

/// What separates the header's parts, and the detail pane's summary fields.
/// Named because the header's width arithmetic has to account for it, and a
/// bare `+ 3` there is a number nobody can check.
const SEP: &str = " · ";

pub fn draw(frame: &mut Frame, app: &mut App, ic: &Icons) {
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    // Set before anything consults `single_pane`, the header included: it reads
    // the published width, and on the first frame -- or the one after a resize --
    // a stale value picks the wrong layout for exactly one frame. The header
    // asks earliest of all, to decide whether to draw the pane tabs.
    app.terminal_width = frame.area().width;
    app.body_top = body.y;
    app.body_bottom = body.y + body.height;

    draw_header(frame, app, ic, top);

    match app.panes() {
        Panes::One => {
            // One pane, filling the width, chosen by focus. There is no
            // divider, so `divider_x = 0` makes `App::on_divider` false and a
            // drag inert -- rather than leaving a stale x from the last wide
            // frame, which would be an invisible drag target in the middle of
            // the screen.
            app.divider_x = 0;
            // No sidebar column to hit: with one pane, `pane_at` answers
            // `focus` for every column anyway, and a stale width from the last
            // three-pane frame would be an invisible dead zone on the left of
            // whichever pane is up.
            app.repos_right = 0;
            match app.focus {
                Focus::Tree => draw_tree(frame, app, ic, body),
                Focus::Detail => draw_detail(frame, app, ic, body),
                Focus::Repos => draw_repos(frame, app, ic, body),
            }
        }
        Panes::Two => {
            let [left, right] = Layout::horizontal([
                Constraint::Percentage(app.split_percent),
                Constraint::Fill(1),
            ])
            .areas(body);

            // Published for mouse handling: the divider sits where the two
            // borders meet.
            app.divider_x = right.x;
            app.repos_right = 0;

            draw_tree(frame, app, ic, left);
            draw_detail(frame, app, ic, right);
        }
        Panes::Three => {
            // The sidebar gets a fixed width rather than a share of the
            // percentage split: it holds names, not prose, so it neither wants
            // nor needs to grow with the terminal the way the tree/detail split
            // does. The tree/detail boundary keeps the same arithmetic as the
            // two-pane case, just over the narrower remainder.
            let [repos, left, right] = Layout::horizontal([
                Constraint::Length(REPOS_PANE_WIDTH),
                Constraint::Percentage(app.split_percent),
                Constraint::Fill(1),
            ])
            .areas(body);

            app.divider_x = right.x;
            // Where the sidebar ends and the tree begins, so `pane_at` can
            // tell a click on a repo row from a click on a task.
            app.repos_right = left.x;

            draw_repos(frame, app, ic, repos);
            draw_tree(frame, app, ic, left);
            draw_detail(frame, app, ic, right);
        }
    }

    draw_status(frame, app, bottom);
    draw_overlays(frame, app);
}

/// Dialogs, drawn last so they sit over whichever layout was used.
fn draw_overlays(frame: &mut Frame, app: &mut App) {
    // Help is taken first and separately because it is the one dialog that
    // writes back into `App` -- the wrapped height it measured -- and so cannot
    // share the borrow on `app.mode` the others take.
    if matches!(app.mode, Mode::Help) {
        draw_help(frame, app);
        return;
    }

    match &app.mode {
        Mode::Prompt(prompt) => draw_prompt(frame, prompt),
        Mode::Confirm { message, .. } => draw_message(
            frame,
            "Delete task",
            message,
            "enter delete    esc cancel",
            BLOCKED,
        ),
        Mode::ForceComplete { message, .. } => draw_message(
            frame,
            "Incomplete subtasks",
            message,
            "enter force    esc cancel",
            ACTIVE,
        ),
        Mode::Error(e) => draw_message(frame, "dex error", e, "any key to dismiss", BLOCKED),
        _ => {}
    }
}

fn glyph(s: Status, ic: &Icons) -> &'static str {
    match s {
        Status::Completed => ic.done,
        Status::InProgress => ic.active,
        Status::Blocked => ic.blocked,
        Status::Pending => ic.pending,
    }
}

/// The marker for a tree row, turning if it is in progress and turning is on.
///
/// `frame` is `None` when nothing is animating -- animation switched off, or no
/// task running -- and the marker falls back to the still `ic.active`. That
/// still glyph is deliberately *not* one of the spinner's frames: it has to read
/// as "in progress" without any motion to help it, which a play triangle does
/// and a single braille dot does not.
///
/// Only tree rows turn. Everywhere the state is *named* rather than watched --
/// the header counts, the help legend -- keeps `ic.active`, because a glyph
/// changing under a static label reads as a fault.
fn row_glyph(s: Status, ic: &Icons, frame: Option<usize>) -> &'static str {
    match (s, frame) {
        (Status::InProgress, Some(f)) if !ic.spin.is_empty() => ic.spin[f % ic.spin.len()],
        _ => glyph(s, ic),
    }
}

fn status_color(s: Status) -> Color {
    match s {
        Status::Completed => DONE,
        Status::InProgress => ACTIVE,
        Status::Blocked => BLOCKED,
        Status::Pending => TODO,
    }
}

/// The status marker's colour: its state's, and nothing else.
///
/// Colour used to carry the animation, alternating with a bright variant. Motion
/// now lives in the glyph (see [`row_glyph`]), so this holds still -- animating
/// both would make one marker say the same thing twice, and loudly.
fn status_style(s: Status) -> Style {
    Style::default().fg(status_color(s))
}

/// How a stacked bar divides into cells. Tier-independent: the glyph table in
/// `icons` decides what each cell looks like, this decides how many there are.
///
/// Width is a parameter rather than `METER_WIDTH` so a wider bar can reuse it.
#[derive(Debug, Clone, Copy)]
struct Bar {
    done: usize,
    active: usize,
    /// Eighths of a cell spilling past the last whole one, `0..=7`. Drawn once,
    /// at the outer edge, in the colour of the run it extends.
    partial: usize,
    empty: usize,
}

impl Bar {
    /// Both coloured runs are laid out from one rounding of their *combined*
    /// extent, not two separate ones. That is what keeps the sub-cell remainder
    /// at the outer edge: rounding each run on its own would put a fraction at
    /// the done->active boundary too, and colouring that cell would need a
    /// background (fg=green on bg=blue), which the colour policy forbids and the
    /// selected row's styling would invert. So done->active always snaps.
    fn new(progress: Progress, width: usize, partials: bool) -> Bar {
        let Progress {
            done,
            active,
            total,
        } = progress;

        if total == 0 || done + active == 0 {
            return Bar {
                done: 0,
                active: 0,
                partial: 0,
                empty: width,
            };
        }

        let eighths = |n: usize| (n as f64 / total as f64 * width as f64 * 8.0).round() as usize;

        // Anything non-zero gets at least a whole cell, so a single finished or
        // in-flight subtask out of a hundred is never rounded away to nothing.
        let floor = (usize::from(done > 0) + usize::from(active > 0)) * 8;
        let mut outer = eighths(done + active).clamp(floor, width * 8);
        if !partials {
            // No sub-cell glyphs in this tier, so snap to the nearest cell. Both
            // clamp bounds are multiples of 8, so this stays inside them.
            outer = (outer as f64 / 8.0).round() as usize * 8;
        }

        let whole = outer / 8;
        let partial = outer % 8;

        // `whole >= 1` per non-zero run, from the floor above, so neither
        // subtraction can wrap.
        let done_cells = if done == 0 {
            0
        } else if active == 0 {
            // Every whole cell is done's. Without this the snap above can push
            // `outer` past done's own rounding, and the leftover would be handed
            // to `active` -- drawing an in-flight cell for a task with nothing
            // started, which contradicts the row's own status glyph.
            whole
        } else {
            let want = ((done as f64 / total as f64) * width as f64).round().max(1.0) as usize;
            want.min(whole - 1)
        };

        Bar {
            done: done_cells,
            active: whole - done_cells,
            partial,
            // `partial` is 0 whenever `whole == width`, since `outer` is capped
            // at `width * 8`.
            empty: width - whole - usize::from(partial > 0),
        }
    }
}

/// Which of `[left cap, middle, right cap]` a cell at `i` draws.
fn cap(i: usize, width: usize) -> usize {
    if i == 0 {
        0
    } else if i + 1 == width {
        2
    } else {
        1
    }
}

/// A compact meter plus the raw fraction, e.g. `██▋░░░░ 3/8`.
///
/// dex-report's stacked bar, in the colours the rest of the UI uses: green for
/// done, blue for in flight, dim for untouched.
///
/// The number is shown alongside the bar on purpose: at seven cells a bar cannot
/// distinguish 2/7 from 3/7, and for triage the exact count is the useful part.
fn meter_spans(progress: Progress, ic: &Icons) -> Vec<Span<'static>> {
    let mut spans = bar_spans(progress, ic, METER_WIDTH);
    spans.push(Span::styled(
        format!(" {}/{}", progress.done, progress.total),
        Style::default().fg(DIM),
    ));
    spans
}

/// The bar alone, at any width. The header draws one too, without the fraction.
fn bar_spans(progress: Progress, ic: &Icons, width: usize) -> Vec<Span<'static>> {
    let m = &ic.meter;
    let bar = Bar::new(progress, width, !m.partial.is_empty());

    let run = |glyphs: [&'static str; 3], from: usize, len: usize| -> String {
        (from..from + len).map(|i| glyphs[cap(i, width)]).collect()
    };

    let mut spans = Vec::new();
    let mut at = 0;

    if bar.done > 0 {
        spans.push(Span::styled(
            run(m.done, at, bar.done),
            Style::default().fg(DONE),
        ));
        at += bar.done;
    }
    if bar.active > 0 {
        spans.push(Span::styled(
            run(m.active, at, bar.active),
            Style::default().fg(ACTIVE),
        ));
        at += bar.active;
    }
    if bar.partial > 0 {
        // Extends whichever run reaches the outer edge, so the fraction reads
        // as more of that state rather than as a state of its own.
        let fg = if bar.active > 0 { ACTIVE } else { DONE };
        spans.push(Span::styled(m.partial[bar.partial - 1], Style::default().fg(fg)));
        at += 1;
    }
    if bar.empty > 0 {
        spans.push(Span::styled(
            run(m.empty, at, bar.empty),
            Style::default().fg(DIM),
        ));
    }

    spans
}

/// The header's count block, widest layout that fits in `room` cells.
///
/// Dropped in order of what carries least: the bar first, then the percentage,
/// then the words -- at which point the status glyphs stand in for them, which
/// is why the tier is needed here. A zero `active` or `blocked` is omitted
/// rather than shown as `0`, following dex-report.
///
/// Returns one group per `·`-separated part; the caller inserts the separators.
/// Cells a run of spans will occupy.
fn span_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

/// What a `·`-separated group of parts will occupy once the caller has joined
/// them, separators included.
///
/// Shared with the test that asserts the header never exceeds its room -- if the
/// test re-derived this, the two would agree about a wrong answer and it would
/// prove nothing.
fn parts_width(parts: &[Vec<Span<'static>>]) -> usize {
    parts.iter().map(|p| span_width(p)).sum::<usize>() + parts.len() * SEP.chars().count()
}

/// The narrowest room in which the counts still draw something.
///
/// Every rung of the header's ladder reserves this much, so the right-hand menu
/// can never outbid the numbers the header exists to show.
fn counts_floor(c: Counts, ic: &Icons) -> usize {
    count_candidates(c, ic)
        .iter()
        .filter(|parts| !parts.is_empty())
        .map(|parts| parts_width(parts))
        .min()
        .unwrap_or(0)
}

/// Where each clickable word of the right-hand block ended up.
///
/// Derived by walking the spans that were *actually rendered*, so the header's
/// degradation ladder does not need restating here -- a rung that dropped the
/// menu simply contains no filter words, and one that dropped the sort contains
/// no sort word. The block's vocabulary is closed and tiny (one sort label plus
/// filter names, which never collide), so matching on content is exact.
///
/// A single filter word means the header fell back to naming the current filter
/// with no menu around it. There is nothing to pick from, so that word cycles.
fn right_zones(right: &[Span], x0: u16, sort_label: &str) -> Vec<(u16, u16, HeaderZone)> {
    let mut found: Vec<(u16, u16, HeaderZone)> = Vec::new();
    let mut x = x0;

    for span in right {
        let w = span.content.chars().count() as u16;
        if w > 0 {
            let zone = if span.content == sort_label {
                Some(HeaderZone::Sort)
            } else if let Some(pane) = tab_zone(&span.content) {
                Some(pane)
            } else {
                tree::Filter::MENU
                    .iter()
                    .find(|f| f.name() == span.content)
                    .map(|f| HeaderZone::Filter(*f))
            };
            if let Some(z) = zone {
                found.push((x, x + w - 1, z));
            }
        }
        x += w;
    }

    let filters = found
        .iter()
        .filter(|(_, _, z)| matches!(z, HeaderZone::Filter(_)))
        .count();
    if filters == 1 {
        for entry in found.iter_mut() {
            if matches!(entry.2, HeaderZone::Filter(_)) {
                entry.2 = HeaderZone::FilterCycle;
            }
        }
    }
    found
}

/// Which pane a drawn tab span selects, if it is one.
///
/// Matched on content like every other zone here, so a tab that was not drawn
/// offers nothing to click. The vocabulary is four strings and cannot collide
/// with a sort label or a filter name.
fn tab_zone(content: &str) -> Option<HeaderZone> {
    // Derived from `TABS` rather than restating it. A hand-written match here
    // was a second copy of the numbering, and this file now has three places
    // that must agree on it -- the tabs, the click zones, and the `[n]` marker
    // on each pane's own border -- with the keys in `main.rs` a fourth.
    TABS.iter()
        .find(|(n, _)| content == format!("[{n}]") || content == format!(" {n} "))
        .map(|(_, f)| HeaderZone::Pane(*f))
}

/// Which pane each numbered tab is, in the order they are drawn -- and the
/// order the `1`/`2`/`3` keys use, since a tab that did not match its key
/// would be worse than no tab at all.
///
/// Numbered left to right as the panes appear on screen, which is the only
/// order anyone can guess from looking at it. They were originally numbered in
/// the order the panes were *built* -- tasks, detail, then the sidebar bolted
/// on as `3` even though it is drawn first.
pub const TABS: [(u8, Focus); 3] = [(1, Focus::Repos), (2, Focus::Tree), (3, Focus::Detail)];

/// The `[n]` a pane draws in its own top-right corner, so the key is on the
/// thing it acts on and not only in the header.
fn pane_number(focus: Focus) -> String {
    TABS.iter()
        .find(|(_, f)| *f == focus)
        .map(|(n, _)| format!("[{n}]"))
        .unwrap_or_default()
}

/// Every pane's border: what it is on the left, the key that reaches it on the
/// right, and a brightness that says whether it has focus.
///
/// One helper rather than three copies, so a pane cannot end up with a number
/// that disagrees with [`TABS`] or a title styled unlike its neighbours. The
/// number is bold when focused, matching the header tabs exactly -- two places
/// showing the same `[n]` must not disagree about what it means.
///
/// The tree deliberately had no title before this, on the grounds that the
/// header already names the store and the border would be the same fact twice.
/// That was right about the *store* and is why these titles name the pane
/// instead: ` tasks ` and ` detail ` say what you are looking at, which the
/// header has never said.
fn pane_block(title: &str, pane: Focus, focus: Focus) -> Block<'static> {
    let focused = pane == focus;
    Block::bordered()
        .title_top(Line::styled(
            format!(" {title} "),
            Style::default().fg(DIM),
        ))
        .title_top(
            Line::styled(
                pane_number(pane),
                if focused {
                    Style::default().fg(PLAIN).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(DIM)
                },
            )
            .right_aligned(),
        )
        .border_style(Style::default().fg(if focused { PLAIN } else { DIM }))
}

/// The pane tabs, `[1] 2 3`, drawn only when one pane is hidden.
///
/// LazyGit and gitui both number their panels so you can jump straight to one.
/// The same idea earns its place here only in zoom mode: with every pane on
/// screen there is nothing to navigate *to*, and the numbers would be
/// decoration competing for a row that already sheds elements to fit.
///
/// Three tabs, not two: the repo sidebar is a pane you can be looking at
/// alone, and leaving it out meant the row you were *on* had no tab -- while
/// `3` worked anyway, unadvertised.
///
/// Both states are three cells wide -- `[1]` against ` 2 ` -- so switching tabs
/// cannot shift anything else in the header sideways.
fn tab_spans(focus: Focus) -> Vec<Span<'static>> {
    let mut out = vec![Span::raw(" ")];
    for (n, f) in TABS {
        if f == focus {
            out.push(Span::styled(
                format!("[{n}]"),
                Style::default().add_modifier(Modifier::BOLD),
            ));
        } else {
            out.push(Span::styled(format!(" {n} "), Style::default().fg(DIM)));
        }
    }
    out
}

/// One filter's name, marked if it is the one in force.
///
/// The mark is weight plus the colour of the state it shows -- yellow for
/// pending, blue for active -- so the menu speaks the same colour language as
/// the rows beneath it. `all` is not a state and gets no colour of its own; it
/// is marked by weight alone.
///
/// This replaced UPPERCASING the active one, which was the only mark available
/// when the whole menu was a single baked string.
fn filter_name(f: tree::Filter, current: bool) -> Span<'static> {
    if !current {
        return Span::styled(f.name(), Style::default().fg(DIM));
    }
    let fg = match f {
        tree::Filter::Pending => TODO,
        tree::Filter::InProgress => ACTIVE,
        tree::Filter::All => PLAIN,
    };
    Span::styled(f.name(), Style::default().fg(fg).add_modifier(Modifier::BOLD))
}

/// The whole menu, `[ all  pending  active ]`, as one span per word.
///
/// Per-word spans are what let the current one be styled differently *and* what
/// let a click be resolved to the word under it -- the two asks that produced
/// this turned out to need the same thing.
fn filter_menu(current: tree::Filter) -> Vec<Span<'static>> {
    let dim = || Style::default().fg(DIM);
    let mut spans = vec![Span::styled("[ ", dim())];
    for (i, f) in tree::Filter::MENU.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(filter_name(*f, *f == current));
    }
    spans.push(Span::styled(" ]", dim()));
    spans
}

/// A leading icon and its trailing space, or nothing in the tiers that have none.
fn icon_span(glyph: &str) -> Vec<Span<'static>> {
    if glyph.is_empty() {
        Vec::new()
    } else {
        vec![Span::styled(
            format!("{glyph} "),
            Style::default().fg(DIM),
        )]
    }
}

/// The narrowest identity worth drawing: which store you are in, and nothing
/// else. The right-hand block yields to this rather than the reverse.
fn identity_store(store: &str, ic: &Icons) -> Vec<Span<'static>> {
    [
        vec![Span::raw(" ")],
        icon_span(ic.project),
        vec![Span::styled(store.to_string(), Style::default().fg(PLAIN))],
    ]
    .concat()
}

/// App identity plus the store, dropped to just the store when the row is tight.
///
/// "Wrong tasks" is this app's most common confusion -- dex resolves its store
/// from the working directory and falls back to a global one outside a git repo
/// -- and this label is the only thing on screen that answers it. The app's own
/// name goes first: you know what you launched.
fn header_identity(store: &str, ic: &Icons, room: usize) -> Vec<Span<'static>> {
    let full = [
        vec![Span::raw(" ")],
        icon_span(ic.app),
        vec![
            Span::styled("dextui", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(SEP, Style::default().fg(DIM)),
        ],
        icon_span(ic.project),
        vec![Span::styled(store.to_string(), Style::default().fg(PLAIN))],
    ]
    .concat();

    for candidate in [full, identity_store(store, ic)] {
        if span_width(&candidate) <= room {
            return candidate;
        }
    }

    // Last resort: the label alone, elided, so a clipped one cannot be mistaken
    // for a whole one -- which is exactly how `dexA-Z` used to read.
    let keep = room.saturating_sub(2); // the leading space, and the ellipsis
    if keep == 0 {
        return Vec::new();
    }
    let short: String = store.chars().take(keep).collect();
    let text = if short.chars().count() < store.chars().count() {
        format!("{short}…")
    } else {
        short
    };
    vec![
        Span::raw(" "),
        Span::styled(text, Style::default().fg(PLAIN)),
    ]
}

/// The sort order and filter, hard right, widest first. They shed what carries
/// least: the menu collapses to the active filter's name, then the sort order
/// goes, because it only reorders what you can already see.
///
/// The leading space is a gutter this block owns, so the left side can fill its
/// own Rect to the last cell without the two ending up flush against each other
/// -- which read as `2 readyA-Z` and looked exactly like the overlap this
/// replaced.
fn right_candidates(sort: &str, filter: tree::Filter) -> [Vec<Span<'static>>; 4] {
    let dim = || Style::default().fg(DIM);
    let with_sort = |rest: Vec<Span<'static>>| -> Vec<Span<'static>> {
        [
            vec![
                Span::raw(" "),
                Span::styled(sort.to_string(), dim()),
                Span::raw("  "),
            ],
            rest,
            vec![Span::raw(" ")],
        ]
        .concat()
    };

    [
        with_sort(filter_menu(filter)),
        with_sort(vec![filter_name(filter, true)]),
        vec![Span::raw(" "), filter_name(filter, true), Span::raw(" ")],
        Vec::new(),
    ]
}

/// The two ends of the header, chosen as a pair.
///
/// Sizing them one after the other looks obvious and is wrong. The right-hand
/// block's steps are large -- a 24-cell menu collapsing to a 3-cell name -- so
/// narrowing the terminal can free more room than the narrowing cost, and an
/// element already dropped would come *back*: at 44 columns the app name was
/// gone and at 36 it was there again. Choosing from one ladder fixes that
/// structurally rather than arithmetically. Every element occupies a *prefix* of
/// the ladder and the ladder descends in width, so first-fit can only ever shed.
///
/// The order encodes what each fact is worth. The store label is in every rung
/// (see `identity_store`). Which filter is active outlives the menu around it and
/// outlives the sort order, because it is the only one of the three that changes
/// *what you can see*. The app's own name outlives none of them: you know what
/// you launched.
fn header_sides(
    store: &str,
    ic: &Icons,
    sort: &str,
    filter: tree::Filter,
    counts_floor: usize,
    width: usize,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let full = header_identity(store, ic, usize::MAX);
    let short = identity_store(store, ic);
    let rights = right_candidates(sort, filter);

    // Every rung leaves the counts room to say *something*. Without that the
    // 24-cell menu outbid them: at 52 columns the header showed the whole filter
    // menu and no numbers at all, while at 44 it showed "2 ready" -- so widening
    // the terminal lost the headline. The menu is an affordance for a key you
    // press; the numbers are what the header is for.
    for (ident, right) in [
        (&full, &rights[0]),
        (&full, &rights[1]),
        (&short, &rights[1]),
        (&short, &rights[2]),
        (&short, &rights[3]),
    ] {
        if span_width(ident) + span_width(right) + counts_floor <= width {
            return (ident.clone(), right.clone());
        }
    }

    // Too narrow for any of that. The label alone still beats an elided one.
    if span_width(&short) <= width {
        return (short, Vec::new());
    }
    (header_identity(store, ic, width), Vec::new())
}

/// Every layout the counts can take, widest first, ending in nothing.
///
/// Built once and shared by both callers: [`header_counts`] takes the first that
/// fits, and [`counts_floor`] measures the narrowest that still says something.
fn count_candidates(c: Counts, ic: &Icons) -> Vec<Vec<Vec<Span<'static>>>> {
    const BAR: usize = 10;

    let numbers = |worded: bool| -> Vec<Vec<Span<'static>>> {
        let mut out: Vec<Vec<Span<'static>>> = Vec::new();
        let mut push = |n: usize, word: &str, glyph: &'static str, fg: Color| {
            let text = if worded {
                format!("{n} {word}")
            } else {
                format!("{glyph} {n}")
            };
            out.push(vec![Span::styled(text, Style::default().fg(fg))]);
        };
        if c.active > 0 {
            push(c.active, "active", ic.active, ACTIVE);
        }
        push(c.ready, "ready", ic.pending, TODO);
        if c.blocked > 0 {
            push(c.blocked, "blocked", ic.blocked, BLOCKED);
        }
        out
    };

    let pct = || -> Vec<Span<'static>> {
        vec![Span::styled(
            format!("{}%", c.percent),
            Style::default().add_modifier(Modifier::BOLD),
        )]
    };

    let bar = || -> Vec<Span<'static>> {
        let mut s = bar_spans(
            Progress {
                done: c.completed,
                active: c.active,
                total: c.total,
            },
            ic,
            BAR,
        );
        s.push(Span::raw(" "));
        s.extend(pct());
        s
    };

    vec![
        [vec![bar()], numbers(true)].concat(),
        [vec![pct()], numbers(true)].concat(),
        numbers(true),
        numbers(false),
        vec![pct()],
        vec![],
    ]
}

/// The widest layout of the counts that fits in `room`.
fn header_counts(c: Counts, room: usize, ic: &Icons) -> Vec<Vec<Span<'static>>> {
    for parts in count_candidates(c, ic) {
        if parts_width(&parts) <= room {
            return parts;
        }
    }
    Vec::new()
}

/// The header: app identity, which store you are in, and what is outstanding.
///
/// Plain text with dim separators, no coloured bands. The app does not impose a
/// look; the terminal's own scheme shows through.
///
/// While searching, the search box takes the whole line over. That is why the
/// header costs no vertical space: the two are never needed at once.
fn draw_header(frame: &mut Frame, app: &mut App, ic: &Icons, area: Rect) {
    // Nothing in the header is on screen while the search box owns the row, so a
    // stale zone would act on a menu that is not there.
    app.header_zones.clear();

    if matches!(app.mode, Mode::Search) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" search ", Style::default().fg(DIM)),
                Span::styled(app.query.value.clone(), Style::default().fg(ACTIVE)),
            ])),
            area,
        );
        frame.set_cursor_position(Position {
            x: (area.x + 8 + app.query.cursor as u16).min(area.right().saturating_sub(1)),
            y: area.y,
        });
        return;
    }

    let c = app.counts();
    let sep = || Span::styled(SEP, Style::default().fg(DIM));

    // These were once two Paragraphs over the *same* Rect -- identity left,
    // sort/filter right-aligned -- with only the counts reserving any room for
    // the other. Below ~48 columns they overwrote each other: the store label
    // vanished leaving a dangling " · ", and narrower still the row read
    // `dexA-Z`. Splitting the row first means an overlap cannot be expressed,
    // and each side is then free to degrade honestly inside its own space.
    // Reserved before the ladder runs rather than competing inside it, so the
    // tabs outlive the sort label and the filter menu. In zoom mode they are the
    // only thing on screen saying the other pane exists; a rung that dropped
    // them would hide the way back.
    // Room the identity needs to say *something* -- a glyph, a letter or two of
    // the store, an ellipsis. Below that the tabs would take the whole row and
    // the store label would vanish with nothing marking it as clipped, which
    // the ladder's own rule forbids: the label is in every rung because "wrong
    // tasks" is this app's most common confusion.
    const IDENTITY_FLOOR: usize = 8;

    let tabs = match app.single_pane() {
        true => tab_spans(app.focus),
        false => Vec::new(),
    };
    let tabs = if span_width(&tabs) + IDENTITY_FLOOR <= area.width as usize {
        tabs
    } else {
        Vec::new()
    };

    let (mut spans, right) = header_sides(
        &app.store_label,
        ic,
        app.sort.label(app.sort_reversed),
        app.filter,
        counts_floor(c, ic),
        (area.width as usize).saturating_sub(span_width(&tabs)),
    );
    let right = [tabs, right].concat();
    let [left_area, right_area] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(span_width(&right) as u16),
    ])
    .areas(area);

    // Whatever the pair left over goes to the counts, which are built to shed.
    let room = (left_area.width as usize).saturating_sub(span_width(&spans));
    for part in header_counts(c, room, ic) {
        spans.push(sep());
        spans.extend(part);
    }

    app.header_zones = right_zones(&right, right_area.x, app.sort.label(app.sort_reversed));

    frame.render_widget(Paragraph::new(Line::from(spans)), left_area);
    frame.render_widget(Paragraph::new(Line::from(right)), right_area);
}

/// The repo pane: registered repositories with their worktrees beneath.
///
/// Colour carries only what the task tree's already does -- a worktree with a
/// store is `PLAIN`, one without is `DIM` -- so the sidebar introduces no new
/// palette and `theme::ALL` stays the whole story.
fn draw_repos(frame: &mut Frame, app: &mut App, ic: &Icons, area: Rect) {
    let block = pane_block("repos", Focus::Repos, app.focus);

    let rows = app.repo_rows();
    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let selected = i == app.selected_repo_row;
            let gutter = if selected {
                Span::styled(format!("{} ", ic.gutter), Style::default().fg(ACCENT))
            } else {
                Span::raw("  ")
            };
            let mut spans = vec![gutter];
            match row {
                crate::repos::Row::Repo { index } => {
                    let r = &app.repos[*index];
                    spans.push(Span::styled(
                        format!("{} {}", ic.marker(true, r.open), r.name),
                        Style::default().fg(PLAIN).add_modifier(Modifier::BOLD),
                    ));
                    // A repo on screen only because it is the one being read,
                    // rather than because anyone saved it. Marked, not hidden
                    // and not coloured: the four colours all mean task states,
                    // and a fifth meaning here would break that language.
                    if !r.registered {
                        spans.push(Span::styled(" ·", Style::default().fg(DIM)));
                    }
                }
                crate::repos::Row::Worktree { repo, index } => {
                    let r = &app.repos[*repo];
                    let w = &r.worktrees[*index];
                    let has = crate::repos::has_store(&w.path);
                    spans.push(Span::styled(
                        format!("   {}", w.branch),
                        Style::default().fg(if has { PLAIN } else { DIM }),
                    ));
                }
            }

            // How much is outstanding in each store, right-aligned. Drawn from
            // the same cache a switch reads, so it costs no `dex` call and
            // cannot disagree with what selecting the row would show. A store
            // that has not been read yet gets nothing rather than `0`: absent
            // and empty are different answers and must stay tellable apart.
            let store = match row {
                crate::repos::Row::Repo { index } => app.repos[*index].store(None),
                crate::repos::Row::Worktree { repo, index } => {
                    let r = &app.repos[*repo];
                    r.store(Some(&r.worktrees[*index]))
                }
            };
            if let Some(c) = app.counts_for_store(&store).filter(|c| c.pending > 0) {
                let tail = c.pending.to_string();
                let used = span_width(&spans) + tail.chars().count();
                let inner = area.width.saturating_sub(2) as usize;
                if used < inner {
                    spans.push(Span::raw(" ".repeat(inner - used)));
                    spans.push(Span::styled(tail, Style::default().fg(TODO)));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    // Without a `ListState`, `render_widget` always draws from the top of the
    // list -- so `G`/`PageDown` could select a row below the visible area
    // with nothing on screen ever scrolling to show it, and `enter` would
    // then switch to a store the user could not see was selected. Mirrors
    // `draw_tree`'s use of `tree_offset` exactly, down to writing the
    // corrected offset back so the next frame does not jump.
    let selected = (!rows.is_empty()).then_some(app.selected_repo_row);
    let mut state = ListState::default().with_offset(app.repos_offset);
    state.select(selected);

    frame.render_stateful_widget(List::new(items).block(block), area, &mut state);
    app.repos_offset = state.offset();

    // Only worth drawing when there is something off-screen -- same
    // threshold `draw_tree`'s scrollbar uses.
    let visible = area.height.saturating_sub(2) as usize;
    if rows.len() > visible {
        let mut sb = ScrollbarState::new(rows.len()).position(app.selected_repo_row);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::default().fg(DIM))
                .thumb_style(Style::default().fg(DIM)),
            area,
            &mut sb,
        );
    }
}

fn draw_tree(frame: &mut Frame, app: &mut App, ic: &Icons, area: Rect) {
    // ` tasks `, not the store name: the header already says which store this
    // is, and repeating that here would be the same fact twice -- see
    // `pane_block`.
    let block = pane_block("tasks", Focus::Tree, app.focus);

    let inner_width = area.width.saturating_sub(2) as usize;
    let rows = tree::visible_rows(&app.tree, &app.expanded);

    // Hoisted: `selected_row` rebuilds the visible-row list on every call, so
    // asking per row would make each frame quadratic in the size of the tree.
    let selected = app.selected_row();
    let accent = if app.focus == Focus::Tree {
        ACCENT
    } else {
        ACCENT_DIM
    };

    // Once per frame, not per row: this scans every task.
    let spin = app.is_animating().then_some(app.spin_frame);

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let t = &row.node.task;
            let is_selected = selected == Some(i);
            // Once per row: deriving this resolves every blocker against the
            // store, and the row needs it three times over.
            let st = dex::status(t, &app.by_id);

            let mut spans = vec![
                // Always two cells, drawn or not, so selecting a row cannot
                // shift its name out of the column its siblings sit in.
                if is_selected {
                    Span::styled(format!("{} ", ic.gutter), Style::default().fg(accent))
                } else {
                    Span::raw("  ")
                },
                Span::styled(row.prefix.clone(), Style::default().fg(DIM)),
                Span::styled(
                    format!("{} ", ic.marker(row.has_children, row.is_open)),
                    Style::default().fg(DIM),
                ),
                Span::styled(
                    format!("{} ", row_glyph(st, ic, spin)),
                    status_style(st),
                ),
            ];

            let mut name_style = if !row.node.is_match {
                // Scaffolding: kept only because a descendant matched.
                Style::default().fg(DIM)
            } else if t.completed {
                Style::default()
                    .fg(DIM)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default().fg(PLAIN)
            };
            // Weight, not colour: the name has no colour of its own to brighten,
            // and bold is the one emphasis that survives a dim or struck-through
            // name. It stays on when the pane loses focus, so the selection is
            // still findable while you are reading the detail pane.
            if is_selected {
                name_style = name_style.add_modifier(Modifier::BOLD);
            }

            spans.push(Span::styled(t.name.clone(), name_style));

            // Only when the status glyph cannot carry it itself. A started task
            // that is also blocked reads as in progress -- dex's precedence --
            // so there the trailing marker is the only signal. Repeating it on
            // a row whose glyph already says blocked is just noise.
            if st != Status::Blocked && dex::is_blocked(t, &app.by_id) {
                spans.push(Span::styled(
                    format!(" {}", ic.blocked),
                    Style::default().fg(BLOCKED),
                ));
            }

            // Right gutter: a rollup for parents, otherwise how long this has been
            // in flight. Only in-progress tasks get an age -- putting one on every
            // row would bury the signal it exists to give.
            let trailing: Vec<Span> = match app.progress.get(&t.id) {
                Some(progress) => meter_spans(*progress, ic),
                None if t.is_in_progress() => match age(&t.started_at) {
                    Some(a) => vec![Span::styled(a, Style::default().fg(ACTIVE))],
                    None => vec![],
                },
                None => vec![],
            };

            if !trailing.is_empty() {
                let used = span_width(&spans);
                let tail = span_width(&trailing);
                // Drop the gutter rather than wrap when the pane is too narrow.
                if used + tail + 2 <= inner_width {
                    spans.push(Span::raw(" ".repeat(inner_width - used - tail)));
                    spans.extend(trailing);
                }
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    // The offset is carried across frames rather than recomputed from zero, so
    // the list does not jump and a click maps to the row actually on screen.
    let mut state = ListState::default().with_offset(app.tree_offset);
    // Still selected even though the highlight draws nothing: this is what
    // scrolls the selection into view, and what keeps `tree_offset` truthful so
    // a click lands on the row actually drawn.
    state.select(selected);

    // No `highlight_style`, and no `highlight_symbol`. The row builds its own
    // cursor above, for two reasons. `highlight_style` is stamped across the
    // whole row *after* the item renders, so it could only ever emphasise the
    // meter and the status glyph along with the name -- which is what ruled out
    // the REVERSED this replaces. And `highlight_symbol` narrows the item area
    // by the symbol's width while the right-hand gutter here is measured against
    // the full inner width, so the meter would be pushed off the right edge.
    frame.render_stateful_widget(List::new(items).block(block), area, &mut state);

    app.tree_offset = state.offset();

    // Only worth drawing when there is something off-screen.
    let visible = area.height.saturating_sub(2) as usize;
    if rows.len() > visible {
        let mut sb = ScrollbarState::new(rows.len()).position(app.selected_row().unwrap_or(0));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::default().fg(DIM))
                .thumb_style(Style::default().fg(DIM)),
            area,
            &mut sb,
        );
    }
}

/// Rows the content will occupy once wrapped.
///
/// Character-wrapping is assumed, which can under-count against ratatui's
/// word-wrapping, so a small allowance is added: over-estimating merely lets you
/// scroll into blank space, whereas under-estimating would make the last line
/// unreachable.
fn wrapped_height(line_widths: &[u16], width: u16, wrap: bool) -> u16 {
    if !wrap || width == 0 {
        return line_widths.len() as u16;
    }

    let rows: u16 = line_widths
        .iter()
        .map(|w| if *w == 0 { 1 } else { w.div_ceil(width) })
        .sum();

    rows.saturating_add(2)
}

/// Word-wraps text to `width`, returning the rows that will actually be drawn.
///
/// The sibling of [`wrapped_height`], and the opposite trade: that one guesses
/// a height for text ratatui wraps itself, and is allowed to guess high. This
/// one does the wrapping, so the caller gets a count instead of an estimate --
/// which is what the help dialog's overflow markers need, since a height one
/// row too many there is a `↓` promising a line that does not exist.
///
/// A word longer than the whole width is broken rather than allowed to overrun.
fn fold(text: &str, width: u16) -> Vec<String> {
    if width == 0 {
        return text.lines().map(str::to_string).collect();
    }
    let width = width as usize;

    let mut out = Vec::new();
    for line in text.lines() {
        if line.chars().count() <= width {
            out.push(line.to_string());
            continue;
        }

        let mut row = String::new();
        let mut row_len = 0;
        for word in line.split(' ') {
            let mut word = word;
            // Break a word too long to ever fit, one full row at a time, so it
            // cannot push the row past `width` and be clipped anyway.
            while word.chars().count() > width {
                if row_len > 0 {
                    out.push(std::mem::take(&mut row));
                    row_len = 0;
                }
                let head: String = word.chars().take(width).collect();
                word = &word[head.len()..];
                out.push(head);
            }

            let len = word.chars().count();
            let sep = usize::from(row_len > 0);
            if row_len + sep + len > width {
                out.push(std::mem::take(&mut row));
                row_len = 0;
            } else if sep == 1 {
                row.push(' ');
                row_len += 1;
            }
            row.push_str(word);
            row_len += len;
        }
        out.push(row);
    }
    out
}

fn draw_detail(frame: &mut Frame, app: &mut App, ic: &Icons, area: Rect) {
    // The wrap state rides on the pane's own name rather than replacing it:
    // `w` is a per-pane toggle whose only sign was this title, and a title
    // that says `detail` half the time and `no wrap` the other half tells you
    // one of the two things at a time.
    let block = pane_block(
        if app.wrap { "detail" } else { "detail · no wrap" },
        Focus::Detail,
        app.focus,
    );

    let inner_w = area.width.saturating_sub(2);
    let inner_h = area.height.saturating_sub(2);
    let scroll = app.detail_scroll;
    let wrap = app.wrap;

    let Some(task) = app.selected_task().cloned() else {
        let msg = if app.tasks.is_empty() {
            "No tasks yet.\n\nPress n to create one."
        } else {
            "No tasks match the current filter.\n\nPress f to change it, or clear the search."
        };
        frame.render_widget(
            Paragraph::new(msg)
                .block(block)
                .style(Style::default().fg(DIM))
                .wrap(Wrap { trim: false }),
            area,
        );
        app.detail_content_height = 0;
        app.detail_viewport_height = inner_h;
        return;
    };

    // Rendered inside its own scope: the lines borrow `app`, and the measured
    // heights cannot be written back until that borrow ends.
    let content_h = {
        let lines = detail_lines(&task, app, ic);
        let widths: Vec<u16> = lines.iter().map(|l| l.width() as u16).collect();
        let height = wrapped_height(&widths, inner_w, wrap);

        let mut paragraph = Paragraph::new(lines).scroll(scroll);
        if wrap {
            paragraph = paragraph.wrap(Wrap { trim: false });
        }
        frame.render_widget(paragraph.block(block), area);
        height
    };

    app.detail_content_height = content_h;
    app.detail_viewport_height = inner_h;

    if content_h > inner_h {
        let mut sb = ScrollbarState::new(content_h.saturating_sub(inner_h) as usize)
            .position(scroll.0 as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_style(Style::default().fg(DIM))
                .thumb_style(Style::default().fg(PLAIN)),
            area,
            &mut sb,
        );
    }
}

/// An age from `dex::age` phrased as time elapsed. Anything under a minute comes
/// back as "now", and "now ago" is not a duration -- it reads as a bug. Both the
/// in-progress summary and the absolute timestamps go through here, because when
/// only one of them did they contradicted each other about the same instant.
///
/// The bare "now" is still what the *tree rows* show, where there is no suffix
/// and a column of ages has to stay narrow.
fn since(age: &str) -> String {
    if age == "now" {
        "just now".to_string()
    } else {
        format!("{age} ago")
    }
}

/// Built entirely from the already-fetched list. `dex show` is never called,
/// because selection changes on every arrow key and a ~180ms process spawn per
/// keypress would make navigation unusable.
fn detail_lines<'a>(t: &'a Task, app: &'a App, ic: &Icons) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(Span::styled(
            t.name.clone(),
            Style::default().fg(PLAIN).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "─".repeat(t.name.chars().count().clamp(8, 60)),
            Style::default().fg(DIM),
        )),
    ];

    // One status line reads faster than three separate label/value rows.
    let st = dex::status(t, &app.by_id);
    let mut summary = vec![Span::styled(
        format!("{} {}", glyph(st, ic), st.label()),
        Style::default().fg(status_color(st)),
    )];

    if t.is_in_progress()
        && let Some(a) = age(&t.started_at) {
            summary.push(Span::styled(SEP, Style::default().fg(DIM)));
            summary.push(Span::styled(
                format!("started {}", since(&a)),
                Style::default().fg(ACTIVE),
            ));
        }

    // How long it actually took, which reads better than two raw timestamps.
    if let Some(took) = t.worked_duration() {
        summary.push(Span::styled(SEP, Style::default().fg(DIM)));
        summary.push(Span::styled(
            format!("took {took}"),
            Style::default().fg(DONE),
        ));
    }

    summary.push(Span::styled(SEP, Style::default().fg(DIM)));
    summary.push(Span::styled(
        format!("priority {}", t.priority),
        Style::default().fg(DIM),
    ));
    lines.push(Line::from(summary));

    if let Some(progress) = app.progress.get(&t.id) {
        lines.push(Line::from(""));
        let mut row = meter_spans(*progress, ic);
        row.push(Span::styled(
            format!(
                "  subtask{} done",
                if progress.total == 1 { "" } else { "s" }
            ),
            Style::default().fg(DIM),
        ));
        lines.push(Line::from(row));
    }

    lines.push(Line::from(""));

    let mut field = |k: &str, v: String, style: Style| {
        lines.push(Line::from(vec![
            Span::styled(format!("{k:<10}"), Style::default().fg(DIM)),
            Span::styled(v, style),
        ]));
    };

    field("id", t.id.clone(), Style::default().fg(DIM));

    if let Some(parent) = t.parent_id.as_ref().and_then(|id| app.by_id.get(id)) {
        field("parent", parent.name.clone(), Style::default().fg(PLAIN));
    }

    if dex::is_blocked(t, &app.by_id) {
        let names: Vec<String> = t
            .blocked_by
            .iter()
            .map(|id| {
                app.by_id
                    .get(id)
                    .map(|b| b.name.clone())
                    .unwrap_or_else(|| id.clone())
            })
            .collect();
        field("blocked", names.join(", "), Style::default().fg(BLOCKED));
    }

    // The reverse relationship. A task holding up three others is a priority
    // signal that `blocked by` alone cannot show.
    if !t.blocks.is_empty() {
        let names: Vec<String> = t
            .blocks
            .iter()
            .map(|id| {
                app.by_id
                    .get(id)
                    .map(|b| b.name.clone())
                    .unwrap_or_else(|| id.clone())
            })
            .collect();
        field("blocks", names.join(", "), Style::default().fg(ACTIVE));
    }

    // Absolute date plus relative age: one for the record, one for the feel.
    let stamp = |iso: &Option<String>| match age(iso) {
        Some(a) => format!("{}  ({})", local_time(iso), since(&a)),
        None => local_time(iso),
    };

    field(
        "created",
        stamp(&t.created_at),
        Style::default().fg(PLAIN),
    );
    if t.started_at.is_some() {
        field("started", stamp(&t.started_at), Style::default().fg(ACTIVE));
    }
    if t.completed_at.is_some() {
        field("done", stamp(&t.completed_at), Style::default().fg(DONE));
    }
    // Only when it is not just an echo of created/started/done.
    if t.has_distinct_update() {
        field("updated", stamp(&t.updated_at), Style::default().fg(DIM));
    }

    // Linked via `dex complete --commit <sha>`. Entirely local -- no sync needed.
    if let Some(c) = t.commit() {
        let mut parts = vec![Span::styled(
            format!("{:<10}", "commit"),
            Style::default().fg(DIM),
        )];
        parts.push(Span::styled(
            c.short_sha().to_string(),
            Style::default().fg(CODE).add_modifier(Modifier::BOLD),
        ));
        if let Some(m) = c.message.as_ref().filter(|m| !m.trim().is_empty()) {
            parts.push(Span::styled(
                format!("  {m}"),
                Style::default().fg(PLAIN),
            ));
        }
        if let Some(b) = c.branch.as_ref().filter(|b| !b.trim().is_empty()) {
            parts.push(Span::styled(
                format!("  ({b})"),
                Style::default().fg(DIM),
            ));
        }
        lines.push(Line::from(parts));
    }

    if let Some(d) = t.description.as_ref().filter(|d| !d.trim().is_empty()) {
        lines.push(Line::from(""));
        lines.extend(markdown_lines(d));
    }

    if let Some(r) = t.result.as_ref().filter(|r| !r.trim().is_empty()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "result",
            Style::default().fg(DIM),
        )));
        for line in r.lines() {
            lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(DONE),
            )));
        }
    }

    lines
}

/// Renders a description as markdown.
///
/// Delegates to `tui-markdown` rather than the small hand-rolled parser this
/// used to have. Tables were the reason: doing them properly needs column
/// measurement and terminal display widths, which that parser deliberately did
/// not attempt, so tables appeared as raw pipes.
///
/// It emits only `Reset`, `dark_gray` and `cyan` — ANSI names the terminal
/// remaps per mode — so it does not reintroduce the fixed-colour problem that
/// made the old theme palettes unreadable on a light background.
fn markdown_lines(text: &str) -> Vec<Line<'static>> {
    crate::markdown::render(text)
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (text, style) = if app.status.is_empty() {
        let keys = match app.focus {
            Focus::Repos => REPO_SHORTCUTS,
            _ => SHORTCUTS,
        };
        (keys.to_string(), Style::default().fg(DIM))
    } else {
        (format!(" {}", app.status), Style::default().fg(ACTIVE))
    };

    frame.render_widget(Paragraph::new(text).style(style), area);
}

fn draw_prompt(frame: &mut Frame, prompt: &crate::app::Prompt) {
    let area = centered(frame.area(), 70, 7);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(format!(" {} ", prompt.title))
        .border_style(Style::default().fg(ACTIVE));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [label_area, input_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(prompt.label.clone()).style(Style::default().fg(DIM)),
        label_area,
    );
    frame.render_widget(
        Paragraph::new(prompt.input.value.clone()).style(Style::default().fg(PLAIN)),
        input_area,
    );
    frame.render_widget(
        Paragraph::new("enter confirm    esc cancel").style(Style::default().fg(DIM)),
        hint_area,
    );

    frame.set_cursor_position(Position {
        x: (input_area.x + prompt.input.cursor as u16).min(input_area.right().saturating_sub(1)),
        y: input_area.y,
    });
}

fn draw_message(frame: &mut Frame, title: &str, body: &str, hint: &str, accent: Color) {
    let area = centered(frame.area(), 66, 9);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_style(Style::default().fg(accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [body_area, hint_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    frame.render_widget(
        Paragraph::new(body.to_string())
            .style(Style::default().fg(PLAIN))
            .wrap(Wrap { trim: false }),
        body_area,
    );
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(DIM)),
        hint_area,
    );
}

/// What `?` shows. Module-level rather than buried in `draw_help`, so a test can
/// hold it against [`SHORTCUTS`] and [`REPO_SHORTCUTS`] -- they advertise the
/// same keys to the same person and must not drift.
///
/// Left-aligned on purpose: centring would destroy the column alignment.
const HELP: &str = "\
tab        switch pane       s   start task
↑ ↓ j k    move / scroll     c   complete (prompts for result)
→ ← h l    expand / scroll   r   rename
g / G      first / last      e   edit description in $EDITOR
w / z      wrap / zoom       n   new top-level task
o / O      sort / reverse    a   new subtask of selection
/          search            d   delete (with confirmation)
f          cycle filter      ^R  refresh now
,          edit config       q   quit
- / +      collapse / expand all

In the repo sidebar these keys act on repos, not on tasks:
1          focus repos       a   register the repo dextui is running in
enter / l  switch the tree and detail panes to the worktree under the cursor
D          unregister the entry (the worktree and its store are untouched)

Movement follows the focused pane, shown by its brighter border. Turn wrap
off (w) to scroll a wide table sideways -- wrapping removes the overflow
there would otherwise be to scroll to.

Each pane carries its own key in its top right corner: [1] repos, [2] tasks,
[3] detail, left to right as they are drawn. Press the number to jump there.

Zoom (z) shows one pane at a time, and the same [1] [2] [3] appear in the
header as tabs -- or enter and left to cross over. Narrow terminals zoom on
their own below single_pane_below columns, usable even on a phone.

Mouse: drag the divider to resize, wheel scrolls the pane under the pointer,
click selects. In the header, click a filter, or the sort label to cycle it
-- right-click the sort to reverse. Shift bypasses capture to select text.

The view refreshes itself whenever the dex store changes, including when
another process or agent edits it. Your selection, expansion and any open
dialog are never disturbed.";

fn draw_help(frame: &mut Frame, app: &mut App) {
    // Sized to the text rather than to a constant that was quietly two thirds
    // of it: at 74x16 the dialog cut off mid-sentence, so most of what `?`
    // documents -- the mouse, the refresh guarantee, and the sidebar keys --
    // could not be read at any terminal size. `centered` still clamps to the
    // frame, but what does not fit now scrolls rather than vanishing.
    let widest = HELP.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let area = centered(frame.area(), widest + 2, HELP.lines().count() as u16 + 3);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(" dextui ")
        .border_style(Style::default().fg(ACTIVE));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [body, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    // Folded here rather than handed to `Paragraph::wrap`, because the markers
    // below have to be exactly right: they are the only thing on screen saying
    // whether anything is hidden, and `wrapped_height` -- which deliberately
    // over-estimates, since for the detail pane a wrong guess should err
    // towards blank space -- would have this claim there was more to read at
    // the very bottom of the text. Folding it ourselves makes the height a
    // count rather than an estimate.
    let folded = fold(HELP, body.width);
    app.help_content_height = folded.len() as u16;
    app.help_viewport_height = body.height;
    // A resize can shrink the content under a scroll taken at a larger size,
    // which would otherwise leave the dialog showing nothing at all.
    app.help_scroll = app.help_scroll.min(app.help_max_scroll());
    let scroll = app.help_scroll;

    frame.render_widget(
        Paragraph::new(folded.iter().map(|s| Line::from(s.as_str())).collect::<Vec<_>>())
            .style(Style::default().fg(PLAIN))
            .scroll((scroll, 0)),
        body,
    );

    // Two `Rect`s rather than two `Paragraph`s over one, so the hint and the
    // markers cannot overwrite each other -- the same rule the header learned.
    let marks = match (scroll > 0, scroll < app.help_max_scroll()) {
        (true, true) => "\u{2191}\u{2193}",
        (true, false) => "\u{2191}",
        (false, true) => "\u{2193}",
        (false, false) => "",
    };
    let mark_w = marks.chars().count() as u16;
    let [text, arrows] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(mark_w)]).areas(hint);

    // "any key" was the whole contract of this dialog for its entire life, so
    // it stays true of every key that is not a movement key -- the hint says
    // which ones now do something else instead, and sheds down to nothing
    // rather than being truncated into a lie.
    let ladder: &[&str] = if app.help_max_scroll() > 0 {
        &["j / k scroll  \u{b7}  any other key dismisses", "j / k scroll"]
    } else {
        &["any key to dismiss", "any key"]
    };
    let label = ladder
        .iter()
        .copied()
        .find(|s| s.chars().count() as u16 <= text.width)
        .unwrap_or("");

    frame.render_widget(
        Paragraph::new(label).style(Style::default().fg(DIM)),
        text,
    );
    frame.render_widget(
        Paragraph::new(marks).style(Style::default().fg(ACTIVE)),
        arrows,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// Plain-text render of the whole pipeline, for `dextui selftest`.
pub fn selftest(app: &App) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let ic = &crate::icons::UNICODE;

    let c = app.counts();
    let _ = writeln!(out, "label   {}", app.store_label);
    let _ = writeln!(
        out,
        "tasks   {} ({} pending: {} active, {} ready, {} blocked; {}% complete)\n",
        app.tasks.len(),
        c.pending,
        c.active,
        c.ready,
        c.blocked,
        c.percent
    );

    for filter in [
        tree::Filter::All,
        tree::Filter::Pending,
        tree::Filter::InProgress,
    ] {
        let forest = tree::build(&app.tasks, "", filter, app.sort, app.sort_reversed);
        let count = tree::flatten(&forest).len();
        let _ = writeln!(out, "--- filter: {filter:?} ({count} visible) ---");
        for node in &forest {
            print_node(node, 0, app, ic, &mut out);
        }
        let _ = writeln!(out);
    }

    if let Some(first) = app.tasks.first() {
        let _ = writeln!(out, "--- detail pane for {} ---", first.name);
        for line in detail_lines(first, app, ic) {
            let _ = writeln!(
                out,
                "{}",
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            );
        }
    }

    out
}

fn print_node(node: &tree::Node, depth: usize, app: &App, ic: &Icons, out: &mut String) {
    use std::fmt::Write;
    let scaffold = if node.is_match { "" } else { "  (scaffold)" };
    let rollup = match app.progress.get(&node.task.id) {
        Some(prog) => format!("  {}/{}", prog.done, prog.total),
        None => String::new(),
    };
    let _ = writeln!(
        out,
        "{}{} {}{}{}",
        "  ".repeat(depth),
        glyph(dex::status(&node.task, &app.by_id), ic),
        node.task.name,
        rollup,
        scaffold
    );
    for c in &node.children {
        print_node(c, depth + 1, app, ic, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dex::Task;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn task(id: &str, parent: Option<&str>, name: &str) -> Task {
        Task {
            id: id.into(),
            parent_id: parent.map(str::to_string),
            name: name.into(),
            description: Some("a description".into()),
            created_at: Some("2026-01-01T00:00:00Z".into()),
            ..Default::default()
        }
    }

    /// A click must select the task drawn on that row, and a click on a row
    /// with no task on it must change nothing at all.
    ///
    /// Every row of the body is walked, borders included, against a real
    /// rendered frame -- because the frame is the only thing that knows which
    /// row each task ended up on, and because the rows that are *not* items are
    /// where this went wrong. The pre-existing tests called `select_at_row`
    /// with hand-computed rows and `body_top = 0`, so they encoded the same
    /// assumption the bug did: clicking the pane's bottom border selected one
    /// index past the last drawn row -- a task not on screen -- and then
    /// scrolled the list to reveal what you had supposedly clicked.
    #[test]
    fn clicking_a_row_selects_the_task_drawn_on_it_and_nothing_otherwise() {
        let tasks: Vec<Task> = (0..12)
            .map(|i| task(&format!("t{i}"), None, &format!("TASK-{i:02}")))
            .collect();

        let mut app = App::new(tasks, "demo".into(), crate::config::Config::default());
        app.filter = tree::Filter::All;
        app.rebuild();

        // Deliberately shorter than the list, so the offset is load-bearing,
        // and wide enough for three panes -- the layout the sidebar puts the
        // tree in, where its x moves but its rows must not.
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();

        for scroll in [0isize, 3, 6] {
            app.scroll_tree(scroll);
            terminal
                .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
                .unwrap();

            let buf = terminal.backend().buffer().clone();
            let line = |y: u16| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            };

            for row in app.body_top..app.body_bottom {
                let drawn = line(row);
                let shown = drawn
                    .split_whitespace()
                    .find(|w| w.starts_with("TASK-"))
                    .map(str::to_string);

                let before = app.selected_task().map(|t| t.name.clone());
                app.select_at_row(row);
                let after = app.selected_task().map(|t| t.name.clone());

                match shown {
                    Some(name) => assert_eq!(
                        after.as_deref(),
                        Some(name.as_str()),
                        "row {row} (offset {}) draws {name:?}, click selected {after:?}",
                        app.tree_offset
                    ),
                    None => assert_eq!(
                        after, before,
                        "row {row} draws no task, but clicking it moved the selection:\n{drawn}"
                    ),
                }
            }
        }
    }

    /// The sidebar's own copy of the rule above. It shares `list_row_index`
    /// with the tree, so this is what stops the two drifting apart again.
    #[test]
    fn clicking_a_sidebar_row_selects_the_worktree_drawn_on_it() {
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.repos = (0..6)
            .map(|i| crate::repos::Repo {
                name: format!("REPO-{i}"),
                path: format!("/tmp/r{i}"),
                open: true,
                registered: true,
                is_global: false,
                worktrees: vec![crate::worktree::Worktree {
                    path: format!("/tmp/r{i}"),
                    branch: format!("BR-{i}"),
                    is_main: true,
                    is_locked: false,
                    is_detached: false,
                }],
            })
            .collect();
        app.focus = Focus::Repos;

        let mut terminal = Terminal::new(TestBackend::new(120, 12)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let rows = app.repo_rows();

        for row in app.body_top..app.body_bottom {
            let drawn = (0..buf.area.width)
                .map(|x| buf[(x, row)].symbol())
                .collect::<String>();
            let shown = drawn
                .split_whitespace()
                .find(|w| w.starts_with("REPO-") || w.starts_with("BR-"))
                .map(str::to_string);

            let before = app.selected_repo_row;
            app.select_repo_at_row(row);
            let after = app.selected_repo_row;

            match shown {
                Some(label) => {
                    let picked = match &rows[after] {
                        crate::repos::Row::Repo { index } => app.repos[*index].name.clone(),
                        crate::repos::Row::Worktree { repo, index } => {
                            app.repos[*repo].worktrees[*index].branch.clone()
                        }
                    };
                    assert_eq!(picked, label, "row {row} draws {label:?}, click picked {picked:?}");
                }
                None => assert_eq!(
                    after, before,
                    "row {row} draws no repo, but clicking it moved the cursor:\n{drawn}"
                ),
            }
        }
    }

    fn render_tasks(tasks: Vec<Task>, w: u16, h: u16, ic: &Icons) -> Vec<String> {
        let mut app = App::new(tasks, "demo".into(), crate::config::Config::default());
        app.filter = tree::Filter::All;
        app.rebuild();

        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, &mut app, ic)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// The status glyph already says "blocked", so repeating it after the name
    /// is noise -- except when the glyph says something else. A started task
    /// that is also blocked renders as in-progress (dex's own precedence), and
    /// then the trailing marker is the only thing carrying the fact.
    #[test]
    fn the_trailing_blocked_marker_appears_only_when_the_glyph_cannot_say_it() {
        let blocker = task("blocker", None, "Blocker");

        let mut idle = task("idle", None, "Idle and blocked");
        idle.blocked_by = vec!["blocker".into()];

        let mut started = task("started", None, "Started but blocked");
        started.blocked_by = vec!["blocker".into()];
        started.started_at = Some("2026-01-01T00:00:00Z".into());

        let ic = &crate::icons::UNICODE;
        let rows = render_tasks(vec![blocker, idle, started], 100, 12, ic);

        let row_for = |name: &str| -> String {
            rows.iter()
                .find(|r| r.contains(name))
                .unwrap_or_else(|| panic!("no row for {name}:\n{}", rows.join("\n")))
                .clone()
        };

        let idle_row = row_for("Idle and blocked");
        assert_eq!(
            idle_row.matches(ic.blocked).count(),
            1,
            "glyph already says blocked, so the marker should not repeat: {idle_row:?}"
        );

        let started_row = row_for("Started but blocked");
        // The marker is a spinner frame, not `ic.active`: the fixture has work in
        // progress, so the row is animating and frame 0 is what is drawn.
        assert!(
            started_row.contains(ic.spin[0]),
            "a started task reads as in progress: {started_row:?}"
        );
        assert_eq!(
            started_row.matches(ic.blocked).count(),
            1,
            "the glyph cannot say blocked here, so the marker must: {started_row:?}"
        );
    }

    /// Renders a full frame and returns it as plain text, one String per row.
    ///
    /// Disables the repos rung: this helper is about the tree/detail split,
    /// predates the sidebar, and every caller's column arithmetic (`tree_rows`
    /// included) assumes the tree pane starts at column 0.
    fn render(w: u16, h: u16, ic: &Icons) -> Vec<String> {
        let mut app = App::new(
            vec![
                task("root", None, "Parent task"),
                task("kid", Some("root"), "Child task"),
            ],
            "demo".into(),
            crate::config::Config::default(),
        );
        app.repos_pane_above = 0;

        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, ic))
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// The whole point of the colour work: dex and dextui are used on the same
    /// tasks in the same directory, so disagreeing about what colour a state is
    /// makes them contradictory rather than merely different.
    ///
    /// Source of truth is dex 0.16.0 `dist/cli/formatting.js`:
    ///   completed -> green (32), started_at -> blue (34), else -> yellow (33).
    #[test]
    fn status_colours_match_the_dex_cli() {
        assert_eq!(status_color(Status::Pending), Color::Yellow, "todo");
        assert_eq!(status_color(Status::InProgress), Color::Blue, "in progress");
        assert_eq!(status_color(Status::Completed), Color::Green, "done");
    }

    /// This terminal follows the macOS appearance and flips light/dark *under
    /// the running app*, so any fixed colour value is wrong half the time.
    /// Only ANSI-16 names and Reset are remapped by the user's theme.
    #[test]
    fn every_theme_colour_adapts_to_the_terminal() {
        for (name, c) in crate::theme::ALL {
            let ok = matches!(
                c,
                Color::Reset
                    | Color::Black
                    | Color::Red
                    | Color::Green
                    | Color::Yellow
                    | Color::Blue
                    | Color::Magenta
                    | Color::Cyan
                    | Color::Gray
                    | Color::DarkGray
                    | Color::LightRed
                    | Color::LightGreen
                    | Color::LightYellow
                    | Color::LightBlue
                    | Color::LightMagenta
                    | Color::LightCyan
                    | Color::White
            );
            assert!(ok, "{name} is {c:?}: Indexed/Rgb cannot follow the theme");
        }
    }

    /// The selection accent is a *cursor*, not a state. Every other colour in
    /// the tree means something about the task; if the gutter shared a hue with
    /// one of them, "where am I" and "what is this" would be the same signal and
    /// a selected row could read as blocked.
    #[test]
    fn the_selection_accent_is_not_a_status_colour() {
        use crate::theme::{ACCENT, ACCENT_DIM};
        for (n, c) in [("ACCENT", ACCENT), ("ACCENT_DIM", ACCENT_DIM)] {
            for (sn, s) in [
                ("TODO", TODO),
                ("ACTIVE", ACTIVE),
                            ("DONE", DONE),
                ("BLOCKED", BLOCKED),
            ] {
                assert_ne!(c, s, "{n} is the same colour as {sn}");
            }
        }
        assert_ne!(ACCENT, ACCENT_DIM, "an unfocused pane must look different");
    }

    #[test]
    fn a_frame_actually_draws_something() {
        // Regression: the app once ran happily while painting an empty screen.
        let rows = render(100, 20, &crate::icons::UNICODE);
        let text = rows.join("\n");
        assert!(
            text.contains("Parent task"),
            "nothing was drawn:\n{text}"
        );
    }

    #[test]
    fn the_header_shows_identity_context_and_counts() {
        let rows = render(100, 20, &crate::icons::UNICODE);
        assert!(rows[0].contains("dextui"), "header row: {:?}", rows[0]);
        assert!(rows[0].contains("demo"), "header row: {:?}", rows[0]);
        // "pending" was one opaque number; it is now split into what you can
        // actually pick up and what you cannot.
        assert!(rows[0].contains("ready"), "header row: {:?}", rows[0]);
    }

    /// The status strip and the help dialog advertise the same keys to the same
    /// person, so they must not drift apart. Nothing else checks this: the CLI
    /// has `every_command_in_the_usage_text_is_actually_accepted`, but the
    /// in-app bindings had no equivalent, which is how `e`/`E` could have been
    /// renamed in one surface and not the other.
    #[test]
    fn the_shortcut_strip_and_the_help_dialog_agree() {
        for (key, action) in [
            ("s", "start"),
            ("c", "done"),
            ("r", "rename"),
            ("e", "edit"),
            ("n", "new"),
            ("a", "sub"),
            ("d", "del"),
            ("f", "filter"),
            ("o", "sort"),
        ] {
            assert!(
                SHORTCUTS.contains(&format!("{key} {action}")),
                "the strip does not advertise {key} for {action}: {SHORTCUTS}"
            );
        }

        // Zoom took `z`, so collapse/expand moved and both surfaces must agree.
        assert!(HELP.contains("- / +      collapse / expand all"), "help: -/+");
        assert!(HELP.contains("w / z"), "help: z zooms");
        assert!(!HELP.contains("z Z"), "the old collapse keys are gone");

        // The pair this change exists to remove. `E` must not survive anywhere,
        // and `r` must no longer mean refresh.
        assert!(!SHORTCUTS.contains("E edit"), "`E` is gone: {SHORTCUTS}");
        assert!(!HELP.contains("E   edit"), "`E` is gone from the help");
        assert!(
            !HELP.contains("f          cycle filter      r   refresh"),
            "bare `r` no longer refreshes"
        );

        // Both surfaces name the same key for each of the two that moved.
        assert!(HELP.contains("r   rename"), "help: r renames");
        assert!(HELP.contains("e   edit description"), "help: e edits");
        assert!(HELP.contains("^R  refresh now"), "help: Ctrl-R refreshes");
    }

    /// The sidebar's keys were on neither surface: the strip still read
    /// `a sub` where `a` registers a repo, and the help mentioned none of
    /// `3`, `a` or `D`. An app that does not document its own keys is the
    /// same defect as one that documents them wrongly.
    #[test]
    fn both_surfaces_advertise_the_repo_sidebar_keys() {
        assert!(SHORTCUTS.contains("1 repos"), "the strip hides the sidebar: {SHORTCUTS}");

        // The strip swaps while the sidebar has focus, because `a` means
        // something else there.
        for key in ["enter switch", "a register", "D unregister"] {
            assert!(
                REPO_SHORTCUTS.contains(key),
                "the sidebar strip does not advertise {key}: {REPO_SHORTCUTS}"
            );
        }
        assert!(
            !REPO_SHORTCUTS.contains("a sub"),
            "the sidebar strip still claims `a` makes a subtask: {REPO_SHORTCUTS}"
        );

        // And the help names each of them too.
        assert!(HELP.contains("1          focus repos"), "help: 1 focuses the sidebar");
        assert!(HELP.contains("a   register the repo"), "help: a registers");
        assert!(HELP.contains("D          unregister"), "help: D unregisters");
        assert!(HELP.contains("enter / l  switch"), "help: enter switches store");
        assert!(HELP.contains("[1] [2] [3]"), "help: the third pane has a tab");
    }

    /// The dialog used to be a fixed 16 rows against ~28 lines of text, so
    /// two thirds of what `?` documented -- the mouse, the refresh guarantee,
    /// the sidebar keys -- was unreachable at every terminal size. Adding a
    /// line to `HELP` that nobody can read is not documenting a key.
    #[test]
    fn the_help_dialog_shows_all_of_the_help() {
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.mode = crate::app::Mode::Help;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let screen = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        // The first and last lines of the text, and one from the section that
        // used to be cut off entirely.
        for line in [
            "tab        switch pane",
            "D          unregister",
            "dialog are never disturbed.",
        ] {
            assert!(screen.contains(line), "help clipped -- missing {line:?}:\n{screen}");
        }
    }

    /// Draws `?` at the given size and returns the screen, having first let a
    /// frame publish the heights `scroll` is clamped against -- which is the
    /// same order the real app does it in, since you cannot scroll a dialog
    /// before it has been drawn to you.
    fn help_screen(width: u16, height: u16, scroll_to_end: bool) -> String {
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.open_help();

        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut render = |app: &mut App| {
            terminal
                .draw(|f| draw(f, app, &crate::icons::UNICODE))
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let first = render(&mut app);
        if !scroll_to_end {
            return first;
        }
        app.scroll_help(i32::MAX);
        render(&mut app)
    }

    /// The whole point of the scroll: the closing paragraph is off the bottom
    /// of a short terminal, and `G` has to be able to reach it. Before this it
    /// was simply gone, with nothing on screen saying so.
    #[test]
    fn the_last_line_of_the_help_is_reachable_on_a_short_terminal() {
        const LAST: &str = "dialog are never disturbed.";

        let top = help_screen(80, 24, false);
        assert!(
            !top.contains(LAST),
            "80x24 is not short enough to test scrolling:\n{top}"
        );
        assert!(top.contains("tab        switch pane"), "help starts at the top:\n{top}");

        let end = help_screen(80, 24, true);
        assert!(end.contains(LAST), "the last line stayed out of reach:\n{end}");
    }

    /// `fold` exists to give an exact row count, so the cases that matter are
    /// the ones where an off-by-one would put a `↓` under the last line.
    #[test]
    fn folding_never_returns_a_row_wider_than_the_width() {
        for width in [1u16, 3, 7, 12, 40, 79, 200] {
            let rows = fold(HELP, width);
            for r in &rows {
                assert!(
                    r.chars().count() <= width as usize,
                    "{width}: row overruns: {r:?}"
                );
            }
            assert!(rows.len() >= HELP.lines().count(), "{width}: rows went missing");
        }
    }

    #[test]
    fn folding_keeps_every_word_and_breaks_only_what_cannot_fit() {
        assert_eq!(fold("one two three", 20), ["one two three"]);
        assert_eq!(fold("one two three", 7), ["one two", "three"]);
        // Exactly the width is not a fold.
        assert_eq!(fold("abcde", 5), ["abcde"]);
        // A word with nowhere to go is broken rather than left to overrun.
        assert_eq!(fold("ab abcdefgh ij", 4), ["ab", "abcd", "efgh", "ij"]);
        // Blank lines are rows too -- the help's paragraph breaks are made of
        // them, and dropping them would misreport the height.
        assert_eq!(fold("a\n\nb", 10), ["a", "", "b"]);
        // Width 0 has no wrap to do, and must not loop forever trying.
        assert_eq!(fold("anything at all", 0), ["anything at all"]);
    }

    /// The markers must be read off the hint row alone: `HELP` documents the
    /// arrow keys, so the same glyphs appear in the body of every screen.
    fn help_hint(screen: &str) -> &str {
        screen
            .lines()
            .find(|l| l.contains("dismiss"))
            .expect("the hint row is always drawn at these widths")
    }

    /// Clipping in silence is the defect. A marker is what turns "that is all
    /// of it" into "there is more", and it must not appear when there is not --
    /// which is why the height it keys off is folded rather than estimated.
    #[test]
    fn the_help_says_when_it_is_hiding_something() {
        let short = help_screen(80, 24, false);
        let hint = help_hint(&short);
        assert!(hint.contains('\u{2193}'), "no more-below marker: {hint:?}");
        assert!(
            hint.contains("any other key dismisses"),
            "the hint never says the movement keys stopped dismissing: {hint:?}"
        );

        let end = help_screen(80, 24, true);
        let hint = help_hint(&end);
        assert!(hint.contains('\u{2191}'), "no more-above marker at the end: {hint:?}");
        assert!(!hint.contains('\u{2193}'), "still claims more below at the end: {hint:?}");

        let whole = help_screen(120, 40, false);
        let hint = help_hint(&whole);
        assert!(
            !hint.contains('\u{2193}') && !hint.contains('\u{2191}'),
            "markers drawn over a dialog that fits: {hint:?}"
        );
        assert!(
            hint.contains("any key to dismiss") && !hint.contains("scroll"),
            "a dialog that fits should still promise any key: {hint:?}"
        );
    }

    /// A terminal narrower than the widest line used to cut it off at the
    /// border mid-sentence. Folding loses the column alignment, but only where
    /// there was never room for it, and it loses no words.
    #[test]
    fn a_narrow_help_folds_its_lines_instead_of_cutting_them() {
        let narrow = help_screen(60, 40, false);
        // Tails of lines wider than a 60-column terminal, which cannot survive
        // it any way but wrapping -- one from the key table, one from prose.
        for tail in ["result)", "$EDITOR", "confirmation)"] {
            assert!(
                narrow.contains(tail),
                "{tail:?} was truncated rather than folded:\n{narrow}"
            );
        }
    }

    /// The header row alone. Every pane now draws its own `[n]` on its border,
    /// so a whole-screen `contains("[1]")` no longer says anything about the
    /// tabs -- it matches a pane corner at every width.
    fn header_row(screen: &str) -> &str {
        screen.lines().next().unwrap_or_default()
    }

    /// The tabs are the only thing on screen saying the other pane exists, so
    /// they appear exactly when one is hidden -- and never when both are up,
    /// where they would be decoration on a row that already sheds to fit.
    #[test]
    fn the_pane_tabs_appear_only_when_a_pane_is_hidden() {
        let zoom = screen(60, 80, Focus::Tree);
        let zoomed = header_row(&zoom);
        assert!(zoomed.contains("[2]"), "no tabs in zoom mode: {zoomed}");
        assert!(zoomed.contains(" 3 "), "no third tab: {zoomed}");

        let wide = screen(100, 80, Focus::Tree);
        let split = header_row(&wide);
        // Not a bare `[` test: the filter menu is spelled `[ all  pending … ]`
        // and lives on this same row.
        for (n, _) in TABS {
            assert!(
                !split.contains(&format!("[{n}]")),
                "tabs drawn beside both panes: {split}"
            );
        }
    }

    /// Numbered left to right as they are drawn: repos, tasks, detail. The
    /// panes were originally numbered in the order they were built, so the
    /// sidebar -- drawn first -- answered to `3`.
    #[test]
    fn the_current_pane_is_the_marked_tab() {
        for (focus, marked) in [
            (Focus::Repos, "[1]"),
            (Focus::Tree, "[2]"),
            (Focus::Detail, "[3]"),
        ] {
            let s = screen(60, 80, focus);
            let row = header_row(&s);
            assert!(row.contains(marked), "{focus:?} should mark {marked}: {row}");
            assert_eq!(
                row.matches('[').count(),
                1,
                "two tabs marked at once: {row}"
            );
        }
    }

    /// The number on a pane's own border and the number in the header tabs are
    /// the same key, so they are read from the one list rather than written
    /// twice -- and this is what says so.
    #[test]
    fn a_panes_own_number_matches_its_header_tab() {
        for (focus, marked) in [
            (Focus::Repos, "[1]"),
            (Focus::Tree, "[2]"),
            (Focus::Detail, "[3]"),
        ] {
            assert_eq!(pane_number(focus), marked);
            let s = screen(60, 80, focus);
            let row = header_row(&s);
            assert!(
                row.contains(&pane_number(focus)),
                "the header marks a different pane than {focus:?} draws: {row}"
            );
        }
    }

    /// Both states must be the same width, or switching tabs would shove the
    /// rest of the header sideways -- the jitter the whole header design avoids.
    #[test]
    fn switching_tabs_does_not_move_anything_else() {
        assert_eq!(
            span_width(&tab_spans(Focus::Tree)),
            span_width(&tab_spans(Focus::Detail))
        );
    }

    /// The tabs are reserved before the ladder runs, so they outlive the sort
    /// label and the filter menu. A rung that dropped them would hide the only
    /// indication that there is a way back.
    #[test]
    fn the_tabs_survive_a_terminal_too_narrow_for_anything_else() {
        for w in [60u16, 50, 40, 30, 24] {
            let s = screen(w, 80, Focus::Detail);
            assert!(
                header_row(&s).contains("[3]"),
                "{w} columns dropped the tabs, hiding the way back:\n{s}"
            );
        }
    }

    /// At an absurd width the tabs would take the whole row and the store label
    /// would vanish with nothing marking it as clipped. The ladder's rule is
    /// that the label survives everything, so the tabs are what yields.
    #[test]
    fn the_tabs_yield_to_the_store_label_at_absurd_widths() {
        for w in [4u16, 6, 8, 10] {
            let s = screen(w, 80, Focus::Tree);
            let head = s.lines().next().unwrap_or("");
            assert!(
                !head.contains("[1]") || head.trim().len() > 4,
                "{w} columns: tabs took the whole row: {head:?}"
            );
        }
    }

    /// Renders a frame and returns the header zones the renderer published.
    fn zones_for(w: u16, filter: tree::Filter, mode: Mode) -> Vec<(u16, u16, HeaderZone)> {
        let mut app = App::new(
            vec![task("root", None, "Parent task")],
            "demo".into(),
            crate::config::Config::default(),
        );
        app.filter = filter;
        app.mode = mode;
        app.rebuild();

        let mut terminal = Terminal::new(TestBackend::new(w, 12)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();
        app.header_zones.clone()
    }

    /// The mark on the active filter is weight plus its state's colour, which is
    /// what replaced UPPERCASING it. Every other option stays dim, so exactly one
    /// word can ever read as current.
    #[test]
    fn exactly_one_filter_is_marked_and_it_is_the_current_one() {
        for current in tree::Filter::MENU {
            let menu = filter_menu(current);
            let marked: Vec<&str> = menu
                .iter()
                .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(marked, vec![current.name()], "current = {current:?}");

            for f in tree::Filter::MENU {
                if f == current {
                    continue;
                }
                let span = menu.iter().find(|s| s.content == f.name()).unwrap();
                assert_eq!(span.style.fg, Some(DIM), "{f:?} should be dim");
            }
        }

        // The colours are the states', so the menu speaks the same language as
        // the rows below it. `all` is not a state and gets no colour.
        assert_eq!(filter_name(tree::Filter::Pending, true).style.fg, Some(TODO));
        assert_eq!(filter_name(tree::Filter::InProgress, true).style.fg, Some(ACTIVE));
        assert_eq!(filter_name(tree::Filter::All, true).style.fg, Some(PLAIN));
    }

    /// A click has to land on the word you can see, so the zones must match what
    /// was drawn rather than a second calculation of where it should have been.
    #[test]
    fn every_menu_word_is_clickable_where_it_is_drawn() {
        let zones = zones_for(120, tree::Filter::Pending, Mode::Normal);

        for f in tree::Filter::MENU {
            let z = zones
                .iter()
                .find(|(_, _, z)| *z == HeaderZone::Filter(f))
                .unwrap_or_else(|| panic!("no zone for {f:?} in {zones:?}"));
            assert_eq!(
                (z.1 - z.0 + 1) as usize,
                f.name().chars().count(),
                "zone for {f:?} is not the width of its word"
            );
        }

        assert!(
            zones.iter().any(|(_, _, z)| *z == HeaderZone::Sort),
            "the sort label is clickable too: {zones:?}"
        );

        // Zones must not overlap, or a click would be ambiguous.
        let mut spans: Vec<(u16, u16)> = zones.iter().map(|(a, b, _)| (*a, *b)).collect();
        spans.sort();
        for pair in spans.windows(2) {
            assert!(pair[0].1 < pair[1].0, "zones overlap: {spans:?}");
        }
    }

    /// Row 0 belongs to the search box while searching. A zone left over from the
    /// previous frame would act on a menu that is not on screen.
    #[test]
    fn the_header_offers_nothing_to_click_while_searching() {
        assert!(zones_for(120, tree::Filter::Pending, Mode::Search).is_empty());
    }

    /// Too narrow for the menu, the header names the current filter alone. With
    /// no options on screen there is nothing to pick, so that word cycles.
    #[test]
    fn the_collapsed_filter_label_cycles_instead_of_picking() {
        let zones = zones_for(46, tree::Filter::Pending, Mode::Normal);
        assert!(
            zones.iter().any(|(_, _, z)| *z == HeaderZone::FilterCycle),
            "narrow header should offer a cycling zone: {zones:?}"
        );
        assert!(
            !zones
                .iter()
                .any(|(_, _, z)| matches!(z, HeaderZone::Filter(_))),
            "nothing to pick from when only one word is drawn: {zones:?}"
        );
    }

    /// The floor is what every rung of the header's ladder reserves, so it must
    /// be exactly the narrowest layout that still says something -- reserving
    /// more would push the filter menu out early, reserving less would let the
    /// menu outbid the numbers.
    ///
    /// It stays six cells however busy the store is, because the narrowest
    /// layout is the percentage alone and a percentage is at most four
    /// characters. That independence is worth pinning: it is why the reservation
    /// can be a constant-ish cost rather than something that grows with the
    /// task count and squeezes the header on exactly the projects that need it.
    #[test]
    fn the_counts_floor_is_the_narrowest_layout_that_still_says_something() {
        let ic = &crate::icons::UNICODE;

        let small = Counts {
            total: 10,
            completed: 4,
            pending: 6,
            active: 1,
            blocked: 2,
            ready: 3,
            percent: 40,
        };
        let busy = Counts {
            total: 4000,
            completed: 1200,
            pending: 2800,
            active: 137,
            blocked: 421,
            ready: 2242,
            percent: 30,
        };

        for c in [small, busy] {
            let floor = counts_floor(c, ic);
            assert!(floor > 0, "reserved nothing for {c:?}");
            // The floor must actually be enough: at exactly that room the counts
            // draw, and one cell narrower they do not.
            assert!(
                !header_counts(c, floor, ic).is_empty(),
                "floor {floor} draws nothing for {c:?}"
            );
            assert!(
                header_counts(c, floor - 1, ic).is_empty(),
                "floor {floor} is not the narrowest for {c:?}"
            );
        }

        assert_eq!(
            counts_floor(small, ic),
            counts_floor(busy, ic),
            "the floor must not grow with the store"
        );
    }

    /// The header shares its row with the sort and filter labels drawn right-
    /// aligned over the same area, so the counts must yield rather than collide.
    /// They are dropped in order of what carries least: bar, then percentage,
    /// then the words.
    #[test]
    fn the_header_counts_give_way_as_the_terminal_narrows() {
        let c = Counts {
            total: 10,
            completed: 4,
            pending: 6,
            active: 1,
            blocked: 2,
            ready: 3,
            percent: 40,
        };
        let ic = &crate::icons::UNICODE;

        // Deliberately the production helper, not a copy of it: a re-derived
        // formula would agree with a wrong one.
        let width = parts_width;

        let mut seen: Vec<usize> = Vec::new();
        for room in (0..=60).rev() {
            let parts = header_counts(c, room, ic);
            let w = width(&parts);
            assert!(w <= room, "room={room} produced {w} cells: {parts:?}");
            seen.push(w);
        }

        // Widest at 60, and it really does shed content on the way down.
        assert!(seen[0] > 0, "nothing drawn even at 60 cells");
        assert_eq!(*seen.last().unwrap(), 0, "something drawn at zero room");
        assert!(
            seen.windows(2).all(|w| w[0] >= w[1]),
            "width must never grow as room shrinks: {seen:?}"
        );

        // The widest layout carries the bar and the percentage; the narrowest
        // non-empty one still names every non-zero state.
        let widest: String = header_counts(c, 60, ic)
            .iter()
            .flatten()
            .map(|s| s.content.to_string())
            .collect();
        assert!(widest.contains("40%"), "{widest:?}");
        assert!(widest.contains("3 ready"), "{widest:?}");
        assert!(widest.contains("2 blocked"), "{widest:?}");
    }

    /// A zero is not worth a word. dex-report omits its zero sections too.
    #[test]
    fn the_header_omits_states_with_nothing_in_them() {
        let c = Counts {
            total: 4,
            completed: 1,
            pending: 3,
            active: 0,
            blocked: 0,
            ready: 3,
            percent: 25,
        };
        let text: String = header_counts(c, 60, &crate::icons::UNICODE)
            .iter()
            .flatten()
            .map(|s| s.content.to_string())
            .collect();

        assert!(text.contains("3 ready"), "{text:?}");
        assert!(!text.contains("active"), "nothing is active: {text:?}");
        assert!(!text.contains("blocked"), "nothing is blocked: {text:?}");
    }

    /// Renders just the header row, for a given store label and width.
    fn render_header(store: &str, w: u16, ic: &Icons) -> String {
        let mut app = App::new(
            vec![task("root", None, "Parent task")],
            store.into(),
            crate::config::Config::default(),
        );
        // The ladder is the subject here, so zoom mode is switched off: its tabs
        // are reserved ahead of the ladder and would otherwise confound every
        // width below the threshold. The tabs have their own tests.
        app.single_pane_below = 0;
        let mut terminal = Terminal::new(TestBackend::new(w, 8)).unwrap();
        terminal.draw(|f| draw(f, &mut app, ic)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<String>()
    }

    /// The two halves of the header used to be two Paragraphs drawn over the
    /// *same* Rect -- identity left-aligned, sort/filter right-aligned -- with
    /// only the counts reserving any room. Below ~48 columns they overwrote each
    /// other: at 44 the store label was eaten and left a dangling " · ", and at
    /// 36 the row read `dexA-Z`. The row is now split into two Rects, so an
    /// overlap can no longer be expressed.
    #[test]
    fn the_headers_two_blocks_never_overwrite_each_other() {
        for ic in crate::icons::ALL {
            // A long label is the same bug at a wide terminal, not just a narrow one.
            for store in ["demo", "a-rather-long-project-name-here"] {
                // The row this narrow shows the store label and nothing else,
                // so it is also the width at which the guarantee below starts.
                let floor = span_width(&identity_store(store, &ic));

                for w in 4u16..=120 {
                    let head = render_header(store, w, &ic);
                    let seen = head.trim_end();
                    let why = format!(
                        "tier {} store {store:?} width {w}: {seen:?}",
                        crate::icons::name(ic.tier)
                    );

                    assert!(
                        !seen.ends_with('·'),
                        "a separator with nothing after it -- {why}"
                    );
                    // The bracketed filter menu is all-or-nothing; half of it
                    // is what being overwritten looked like.
                    assert_eq!(
                        seen.contains('['),
                        seen.contains(']'),
                        "half a filter menu -- {why}"
                    );
                    // Given room for it at all, the store label survives whole.
                    // It is what the right-hand block yields to, and it used to
                    // be the casualty: at 44 columns it was overwritten outright
                    // and at 36 it was gone, in both cases silently.
                    if w as usize >= floor {
                        assert!(
                            seen.contains(store),
                            "the store label did not survive -- {why}"
                        );
                    } else {
                        // Below that, a clipped label must say it is clipped, or
                        // `dexA-Z` reads as the name of something.
                        assert!(
                            seen.is_empty() || seen.contains('…') || seen.contains(store),
                            "clipped with nothing to say so -- {why}"
                        );
                    }
                }
            }
        }
    }

    /// "Wrong tasks" is this app's most common confusion and the store label is
    /// the only thing on the screen that answers it, so it outlives the app's
    /// own name.
    #[test]
    fn the_identity_gives_up_its_own_name_before_the_store() {
        let ic = &crate::icons::UNICODE;
        let text = |room: usize| -> String {
            header_identity("my-project", ic, room)
                .iter()
                .map(|s| s.content.to_string())
                .collect()
        };

        assert!(text(30).contains("dextui"), "{:?}", text(30));
        assert!(text(30).contains("my-project"), "{:?}", text(30));

        // Room for one of the two: it is the store.
        let tight = text(11);
        assert!(tight.contains("my-project"), "{tight:?}");
        assert!(!tight.contains("dextui"), "{tight:?}");

        let mut seen: Vec<usize> = Vec::new();
        for room in (0..=40).rev() {
            let w = span_width(&header_identity("my-project", ic, room));
            assert!(w <= room, "room={room} produced {w} cells");
            seen.push(w);
        }
        assert!(
            seen.windows(2).all(|w| w[0] >= w[1]),
            "width must never grow as room shrinks: {seen:?}"
        );
        assert_eq!(*seen.last().unwrap(), 0, "something drawn at zero room");
    }

    /// A filter silently in force with nothing on screen saying so is the most
    /// confusing state this app has, so which filter is active is the last thing
    /// the right-hand block drops -- after the menu around it, and after the
    /// sort order, which only changes the order of what you can already see.
    #[test]
    fn the_right_hand_block_keeps_the_active_filter_longest() {
        let filter = tree::Filter::Pending;
        let cs = right_candidates("priority", filter);
        let text = |i: usize| -> String {
            cs[i].iter().map(|s| s.content.to_string()).collect()
        };

        assert!(text(0).contains("[ all  pending  active ]"), "{:?}", text(0));

        assert!(!text(1).contains('['), "the menu should have gone: {:?}", text(1));
        assert!(text(1).contains("priority"), "{:?}", text(1));
        assert!(text(1).contains(filter.name()), "{:?}", text(1));

        assert!(!text(2).contains("priority"), "sort should have gone: {:?}", text(2));
        assert!(text(2).contains(filter.name()), "{:?}", text(2));

        assert!(cs[3].is_empty(), "the last rung draws nothing: {:?}", text(3));

        // Strictly descending, which is what makes the ladder in `header_sides`
        // monotone: a wider terminal can never pick a narrower rung.
        let widths: Vec<usize> = cs.iter().map(|c| span_width(c)).collect();
        assert!(
            widths.windows(2).all(|w| w[0] > w[1]),
            "candidates must strictly narrow: {widths:?}"
        );
    }

    /// The header must never bring back something it has already dropped. Sizing
    /// the two ends one after the other did exactly that: the right-hand block's
    /// steps are far larger than the identity's, so narrowing the terminal could
    /// free more room than the narrowing cost. The app name was absent at 44
    /// columns and present again at 36 -- which reads as a rendering bug, because
    /// nothing about a smaller window should reveal more.
    #[test]
    fn the_header_never_brings_back_what_it_has_already_dropped() {
        for ic in crate::icons::ALL {
            for store in ["demo", "a-rather-long-project-name-here"] {
                // Everything here is state the header is *reporting*; the counts
                // are excluded on purpose, being the one part built to shed and
                // regain content as room allows.
                let markers = ["dextui", "[", "priority", tree::Filter::Pending.name()];
                let mut last_seen = [0u16; 4];

                for w in 4u16..=140 {
                    let head = render_header(store, w, &ic);
                    for (i, m) in markers.iter().enumerate() {
                        if head.contains(m) {
                            last_seen[i] = w;
                        } else if last_seen[i] != 0 {
                            panic!(
                                "{m:?} was drawn at {} columns and is back to being \
                                 absent at {w} -- tier {}, store {store:?}: {head:?}",
                                last_seen[i],
                                crate::icons::name(ic.tier)
                            );
                        }
                    }
                }
            }
        }
    }

    /// `age` reports "now" for anything under a minute, which reads as a bug the
    /// moment something suffixes it: "started now ago". The absolute timestamp
    /// rows special-cased it from the start; the in-progress summary line did
    /// not, so the two disagreed on the same screen about the same instant.
    #[test]
    fn a_task_started_moments_ago_reads_just_now_not_now_ago() {
        let mut t = task("t", None, "Fresh task");
        t.started_at = Some(chrono::Utc::now().to_rfc3339());

        let rows = render_tasks(vec![t], 120, 16, &crate::icons::UNICODE);
        let text = rows.join("\n");
        assert!(
            !text.contains("now ago"),
            "\"now ago\" is not a duration:\n{text}"
        );

        let summary = rows
            .iter()
            .find(|r| r.contains("in progress"))
            .unwrap_or_else(|| panic!("no status line:\n{text}"));
        assert!(
            summary.contains("started just now"),
            "status line: {summary:?}"
        );
    }

    #[test]
    fn every_icon_tier_renders() {
        for ic in crate::icons::ALL {
            let text = render(100, 20, &ic).join("\n");
            assert!(
                text.contains("Parent task"),
                "tier {} drew nothing",
                crate::icons::name(ic.tier)
            );
        }
    }

    /// Renders one task whose description is `md`, and returns the frame text.
    fn render_description(md: &str, w: u16, h: u16) -> String {
        let mut app = App::new(
            vec![Task {
                id: "t".into(),
                name: "Task".into(),
                description: Some(md.to_string()),
                created_at: Some("2026-01-01T00:00:00Z".into()),
                ..Default::default()
            }],
            "demo".into(),
            crate::config::Config::default(),
        );

        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn markdown_tables_are_drawn_as_tables_not_raw_pipes() {
        // The reason tui-markdown replaced the hand-rolled parser: this used to
        // render as literal `|---|---|` rows with unaligned columns.
        let text = render_description(
            "| option | cost |\n|---|---:|\n| hand-rolled | low |\n",
            110,
            24,
        );

        assert!(text.contains('┌') && text.contains('┼'), "no table borders:\n{text}");
        assert!(
            !text.contains("|---"),
            "the delimiter row leaked through:\n{text}"
        );
    }

    #[test]
    fn a_wide_table_in_a_narrow_pane_does_not_panic() {
        // Tables are laid out at their natural width, which can exceed the pane.
        let wide = "| a very long column header here | and another one |\n                    |---|---|\n| some long cell value | another long value |\n";
        for w in [40u16, 60, 80] {
            let _ = render_description(wide, w, 20);
        }
    }

    #[test]
    fn a_very_narrow_pane_does_not_panic() {
        // The right-hand gutter must be dropped rather than overflow the row.
        // Every tier, because the nerd cap kit is new geometry in that gutter.
        for ic in crate::icons::ALL {
            for w in [20u16, 30, 40] {
                let _ = render(w, 12, &ic);
            }
        }
    }

    /// Every `Progress` a real store can produce, at both sub-cell settings.
    fn every_bar(mut f: impl FnMut(Progress, Bar, bool)) {
        for total in 1..=60usize {
            for done in 0..=total {
                for active in 0..=(total - done) {
                    let p = Progress {
                        done,
                        active,
                        total,
                    };
                    for partials in [false, true] {
                        f(p, Bar::new(p, METER_WIDTH, partials), partials);
                    }
                }
            }
        }
    }

    /// The bar is a fixed-width column in the right-hand gutter; a run that does
    /// not add up shifts everything after it. This is also the underflow guard:
    /// the cell arithmetic subtracts `usize`s, so a slip panics here rather than
    /// wrapping to a four-billion-cell `repeat` in the renderer.
    #[test]
    fn a_bar_always_fills_exactly_the_meter_width() {
        every_bar(|p, b, partials| {
            assert_eq!(
                b.done + b.active + usize::from(b.partial > 0) + b.empty,
                METER_WIDTH,
                "{p:?} partials={partials} -> {b:?}"
            );
            assert!(
                b.done + b.active <= METER_WIDTH,
                "coloured runs overflow the bar: {p:?} partials={partials} -> {b:?}"
            );
        });
    }

    /// The mirror of `a_non_zero_count_never_rounds_away_to_nothing`, and the
    /// more dangerous direction: a run that does not exist must never be drawn.
    ///
    /// In a tier without sub-cell glyphs the bar snaps to whole cells, and the
    /// snap can push the combined extent past the done run's own rounding. The
    /// leftover cell was handed to `active` unconditionally, so a task with
    /// nothing started painted an in-flight cell -- the meter and the status
    /// glyph describing the same task in contradictory terms, which is the exact
    /// failure this whole epic exists to remove.
    #[test]
    fn a_zero_count_is_never_drawn() {
        every_bar(|p, b, partials| {
            if p.active == 0 {
                assert_eq!(b.active, 0, "phantom in-flight: {p:?} partials={partials} -> {b:?}");
            }
            if p.done == 0 {
                assert_eq!(b.done, 0, "phantom done: {p:?} partials={partials} -> {b:?}");
            }
        });

        // The smallest real case, found by brute force over the arithmetic: 7 of
        // 9 done and nothing started rendered `#####+.` in ascii, with the `+`
        // in blue.
        let b = Bar::new(
            Progress {
                done: 7,
                active: 0,
                total: 9,
            },
            METER_WIDTH,
            false,
        );
        assert_eq!(b.active, 0, "7 of 9 done, none started -> {b:?}");
    }

    /// One finished subtask out of a hundred is the single most useful thing a
    /// meter can say, and rounding would erase it. The rule predates this bar;
    /// moving to eighths must not quietly downgrade it to a 1/8 sliver.
    #[test]
    fn a_non_zero_count_never_rounds_away_to_nothing() {
        every_bar(|p, b, partials| {
            if p.done > 0 {
                assert!(b.done >= 1, "{p:?} partials={partials} -> {b:?}");
            }
            if p.active > 0 {
                assert!(b.active >= 1, "{p:?} partials={partials} -> {b:?}");
            }
        });

        let one = Bar::new(Progress { done: 1, active: 0, total: 100 }, METER_WIDTH, true);
        assert_eq!((one.done, one.partial), (1, 0), "{one:?}");

        let both = Bar::new(Progress { done: 1, active: 1, total: 100 }, METER_WIDTH, true);
        assert_eq!((both.done, both.active), (1, 1), "{both:?}");
    }

    /// `partial` indexes `Meter::partial[eighths - 1]`, a seven-entry table, so
    /// an eighth of 8 would panic. It cannot arise because the remainder is
    /// taken modulo 8 rather than patched after rounding whole cells -- the
    /// naive scheme reaches 8/8 at, for instance, 16 done and 16 active of 45.
    #[test]
    fn the_partial_cell_never_exceeds_seven_eighths() {
        every_bar(|p, b, partials| {
            assert!(b.partial <= 7, "{p:?} partials={partials} -> {b:?}");
        });

        let b = Bar::new(Progress { done: 16, active: 16, total: 45 }, METER_WIDTH, true);
        assert!(b.partial <= 7, "{b:?}");
    }

    /// Nerd and ascii have no eighth-blocks, so their bars must land on whole
    /// cells -- and still round to the nearest one rather than truncating.
    #[test]
    fn a_tier_without_partial_glyphs_snaps_to_whole_cells() {
        every_bar(|p, b, partials| {
            if !partials {
                assert_eq!(b.partial, 0, "{p:?} -> {b:?}");
            }
        });

        let b = Bar::new(Progress { done: 3, active: 0, total: 8 }, METER_WIDTH, false);
        assert_eq!((b.done, b.active, b.partial, b.empty), (3, 0, 0, 4), "{b:?}");
    }

    /// A true sub-cell colour boundary would need fg=green on bg=blue, which
    /// introduces a background the colour policy forbids and which the selected
    /// row's styling would invert. So the fraction lives at the outer edge only
    /// and done->active snaps.
    ///
    /// This also pins a deliberate behaviour change: the outer edge rounds on
    /// the *combined* extent (1+1 of 3 is 4.67 cells, so 5) rather than summing
    /// two separately-rounded runs (2 + 2 = 4).
    #[test]
    fn the_partial_sits_at_the_outer_edge_not_the_done_active_boundary() {
        let b = Bar::new(Progress { done: 1, active: 1, total: 3 }, METER_WIDTH, true);
        assert_eq!((b.done, b.active, b.partial, b.empty), (2, 2, 5, 2), "{b:?}");
    }

    /// The reason the sub-cell edge exists at all: without it 13 of 14 fills
    /// every cell and reads as finished.
    #[test]
    fn the_outer_edge_carries_the_sub_cell_remainder() {
        let b = Bar::new(Progress { done: 3, active: 0, total: 8 }, METER_WIDTH, true);
        assert_eq!((b.done, b.active, b.partial, b.empty), (2, 0, 5, 4), "{b:?}");

        let nearly = Bar::new(Progress { done: 13, active: 0, total: 14 }, METER_WIDTH, true);
        assert_eq!(nearly.done, 6, "{nearly:?}");
        assert!(nearly.partial > 0, "a full bar would read as finished: {nearly:?}");
        assert_eq!(nearly.empty, 0, "{nearly:?}");
    }

    #[test]
    fn an_untouched_parent_is_all_trough_and_a_finished_one_is_all_bar() {
        let none = Bar::new(Progress { done: 0, active: 0, total: 4 }, METER_WIDTH, true);
        assert_eq!((none.done, none.active, none.partial, none.empty), (0, 0, 0, METER_WIDTH));

        let all = Bar::new(Progress { done: 7, active: 0, total: 7 }, METER_WIDTH, true);
        assert_eq!((all.done, all.partial, all.empty), (METER_WIDTH, 0, 0));
    }

    /// The bar's spans, without the trailing ` n/total`, as (text, foreground).
    fn meter_bar(p: Progress, ic: &Icons) -> Vec<(String, Option<Color>)> {
        let mut spans = meter_spans(p, ic);
        spans.pop();
        spans
            .into_iter()
            .map(|s| (s.content.into_owned(), s.style.fg))
            .collect()
    }

    /// The bar sits in a fixed column in the tree's right gutter, and the gutter
    /// is sized with `chars().count()`. A glyph the terminal measures as double
    /// width would shift every row -- the exact failure already documented for
    /// `▾ ▸ ⊘`, and the live risk with the nerd tier's Private Use Area kit.
    /// `Span::width` goes through unicode-width, so this catches both that and a
    /// plain arithmetic slip.
    #[test]
    fn the_meter_is_exactly_seven_cells_wide_in_every_tier() {
        let cases = [
            Progress { done: 0, active: 0, total: 4 },
            Progress { done: 3, active: 0, total: 8 },
            Progress { done: 1, active: 1, total: 3 },
            Progress { done: 1, active: 0, total: 100 },
            Progress { done: 13, active: 0, total: 14 },
            Progress { done: 7, active: 0, total: 7 },
        ];
        for ic in crate::icons::ALL {
            for p in cases {
                let bar = meter_bar(p, &ic);
                let cells: usize = bar.iter().map(|(t, _)| Span::raw(t.clone()).width()).sum();
                let chars: usize = bar.iter().map(|(t, _)| t.chars().count()).sum();
                assert_eq!(
                    cells,
                    METER_WIDTH,
                    "tier {} {p:?}: {bar:?}",
                    crate::icons::name(ic.tier)
                );
                assert_eq!(
                    chars,
                    METER_WIDTH,
                    "tier {} {p:?}: the gutter is measured in chars: {bar:?}",
                    crate::icons::name(ic.tier)
                );
            }
        }
    }

    /// The point of the whole change: the one place progress is quantified now
    /// speaks the same colour language as the status glyphs.
    #[test]
    fn the_meter_paints_done_in_flight_and_untouched_in_the_status_colours() {
        let p = Progress { done: 2, active: 2, total: 7 };
        for ic in crate::icons::ALL {
            let fgs: Vec<_> = meter_bar(p, &ic).into_iter().map(|(_, fg)| fg).collect();
            assert_eq!(
                fgs,
                vec![Some(DONE), Some(ACTIVE), Some(DIM)],
                "tier {}",
                crate::icons::name(ic.tier)
            );
        }

        let spans = meter_spans(p, &crate::icons::UNICODE);
        assert_eq!(spans.last().unwrap().style.fg, Some(DIM), "the fraction is secondary");
    }

    /// The fraction is a sliver of *more of the same state*, not a state of its
    /// own, so it takes the colour of whichever run reaches the outer edge.
    #[test]
    fn the_partial_cell_takes_the_colour_of_the_run_it_extends() {
        let ic = &crate::icons::UNICODE;

        let done_only = meter_bar(Progress { done: 3, active: 0, total: 8 }, ic);
        assert_eq!(
            done_only.iter().map(|(_, fg)| *fg).collect::<Vec<_>>(),
            vec![Some(DONE), Some(DONE), Some(DIM)],
            "{done_only:?}"
        );
        assert_eq!(done_only[1].0, "\u{258b}", "5/8 of a cell: {done_only:?}");

        let mixed = meter_bar(Progress { done: 1, active: 1, total: 3 }, ic);
        assert_eq!(
            mixed.iter().map(|(_, fg)| *fg).collect::<Vec<_>>(),
            vec![Some(DONE), Some(ACTIVE), Some(ACTIVE), Some(DIM)],
            "{mixed:?}"
        );
    }

    /// Position, not just fill, chooses the glyph: the nerd kit's caps are what
    /// make seven cells read as one bar rather than seven stamps.
    #[test]
    fn the_nerd_meter_is_capped_at_both_ends() {
        let bar: String = meter_bar(Progress { done: 2, active: 0, total: 7 }, &crate::icons::NERD)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(
            bar,
            "\u{ee03}\u{ee04}\u{ee01}\u{ee01}\u{ee01}\u{ee01}\u{ee02}",
            "{bar:?}"
        );
    }

    /// At seven cells a bar cannot distinguish 2/7 from 3/7, so the exact count
    /// is the part that is actually useful for triage.
    #[test]
    fn the_fraction_stays_beside_the_bar() {
        let spans = meter_spans(Progress { done: 3, active: 0, total: 8 }, &crate::icons::UNICODE);
        assert_eq!(spans.last().unwrap().content.as_ref(), " 3/8");
    }

    fn started(id: &str, name: &str) -> Task {
        Task {
            started_at: Some("2026-01-01T00:00:00Z".into()),
            ..task(id, None, name)
        }
    }

    /// A whole frame as a `Buffer`, which keeps the per-cell styling the plain
    /// `render` helper throws away -- and styling is half the subject here.
    fn render_frame(tasks: Vec<Task>, frame: usize, ic: &Icons) -> ratatui::buffer::Buffer {
        let mut app = App::new(tasks, "demo".into(), crate::config::Config::default());
        app.filter = tree::Filter::All;
        app.rebuild();
        app.spin_frame = frame;

        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|f| draw(f, &mut app, ic)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Renders at `width` and returns the whole screen as text.
    fn screen(width: u16, single_pane_below: u16, focus: Focus) -> String {
        let mut app = App::new(
            vec![
                Task {
                    // Only the detail pane renders a description, so this is an
                    // unambiguous marker for it. "priority" is not: that is the
                    // sort label, and it sits in the header in both layouts.
                    description: Some("DETAIL-ONLY-MARKER".into()),
                    ..task("a", None, "A task in the tree")
                },
                task("b", None, "Another one"),
            ],
            "demo".into(),
            crate::config::Config::default(),
        );
        app.filter = tree::Filter::All;
        app.single_pane_below = single_pane_below;
        app.focus = focus;
        app.selected = Some("a".into());
        app.rebuild();

        let mut terminal = Terminal::new(TestBackend::new(width, 14)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Below the threshold `focus` stops meaning "which border is brighter" and
    /// starts meaning "which pane you are looking at". Both panes carry content
    /// the other does not, so each is identifiable in the output.
    #[test]
    fn below_the_threshold_focus_decides_which_pane_is_drawn() {
        let tree_view = screen(60, 80, Focus::Tree);
        assert!(tree_view.contains("Another one"), "no tree: {tree_view}");
        assert!(
            !tree_view.contains("DETAIL-ONLY-MARKER"),
            "the detail pane leaked in: {tree_view}"
        );

        let detail_view = screen(60, 80, Focus::Detail);
        assert!(
            detail_view.contains("DETAIL-ONLY-MARKER"),
            "no detail pane: {detail_view}"
        );
        assert!(
            !detail_view.contains("Another one"),
            "the tree leaked in: {detail_view}"
        );
    }

    /// Above it, focus goes back to meaning emphasis and both are on screen.
    #[test]
    fn above_the_threshold_both_panes_are_drawn_whichever_has_focus() {
        for focus in [Focus::Tree, Focus::Detail] {
            let s = screen(100, 80, focus);
            assert!(s.contains("Another one"), "{focus:?}: no tree: {s}");
            assert!(
                s.contains("DETAIL-ONLY-MARKER"),
                "{focus:?}: no detail: {s}"
            );
        }
    }

    /// A dialog has to survive the layout it is drawn over, and the single-pane
    /// path returns early -- so this is the assertion that stops it returning
    /// before the overlays.
    #[test]
    fn dialogs_still_draw_over_a_single_pane() {
        let mut app = App::new(
            vec![task("a", None, "A task")],
            "demo".into(),
            crate::config::Config::default(),
        );
        app.single_pane_below = 80;
        app.mode = Mode::Help;
        app.rebuild();

        let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol())
            .collect();

        assert!(text.contains("switch pane"), "the help dialog is missing");
    }

    /// The glyph changes every frame now, so the assertion that matters is that
    /// its **column** does not. That is the whole risk of a spinner whose frames
    /// are font-fallbacked, and the reason this was rejected twice before.
    #[test]
    fn the_spinner_turns_without_moving_the_column() {
        for ic in [
            &crate::icons::NERD,
            &crate::icons::UNICODE,
            &crate::icons::ASCII,
        ] {
            let tasks = || vec![task("a", None, "Idle task"), started("b", "Running task")];

            let mut columns = std::collections::HashSet::new();
            let mut seen = std::collections::HashSet::new();

            for f in 0..ic.spin.len() {
                let buf = render_frame(tasks(), f, ic);

                // Scoped to the running task's own row, then located by colour.
                //
                // Neither half is optional. Searching the whole buffer finds the
                // header's own `1 active`, which is ACTIVE-coloured too; and
                // searching by symbol matches tree scaffolding that shares a
                // character, which is how the ascii tier's `|` frame was caught
                // colliding with the selection gutter.
                let row = (0..buf.area.height)
                    .find(|&y| {
                        (0..buf.area.width)
                            .map(|x| buf[(x, y)].symbol())
                            .collect::<String>()
                            .contains("Running task")
                    })
                    .unwrap_or_else(|| panic!("{:?}: no row for the running task", ic.tier));

                let (at, style) = (0..buf.area.width)
                    .map(|x| ((x, row), buf[(x, row)].style()))
                    .find(|(_, s)| s.fg == Some(ACTIVE))
                    .unwrap_or_else(|| panic!("{:?}: no in-progress marker drawn", ic.tier));

                assert_eq!(
                    buf[at].symbol(),
                    ic.spin[f],
                    "{:?}: frame {f} drew the wrong glyph",
                    ic.tier
                );
                columns.insert(at.0);
                seen.insert(ic.spin[f]);

                // Colour no longer carries the motion, so it must stay put.
                assert!(
                    !style.add_modifier.contains(Modifier::BOLD),
                    "{:?}: frame {f} changed weight",
                    ic.tier
                );
            }

            assert_eq!(
                columns.len(),
                1,
                "{:?}: the marker moved between frames: {columns:?}",
                ic.tier
            );
            assert_eq!(
                seen.len(),
                ic.spin.len(),
                "{:?}: frames repeated within one cycle",
                ic.tier
            );
        }
    }

    /// With animation off the marker must be the still glyph -- the same one the
    /// header counts and help legend show -- not whichever spinner frame happens
    /// to sit at index 0. A lone braille dot does not read as "in progress"
    /// without motion behind it; a play triangle does.
    #[test]
    fn with_animation_off_the_marker_is_the_still_glyph() {
        for ic in [
            &crate::icons::NERD,
            &crate::icons::UNICODE,
            &crate::icons::ASCII,
        ] {
            assert_eq!(row_glyph(Status::InProgress, ic, None), ic.active);
            // And it is genuinely a different glyph from the rotation, or this
            // assertion would pass by coincidence.
            assert!(
                !ic.spin.contains(&ic.active),
                "{:?}: the still glyph is also a spinner frame",
                ic.tier
            );
        }
    }

    /// Every frame must be exactly one character wide. A multi-character frame
    /// would shift the column outright, before any font question arises.
    #[test]
    fn every_spinner_frame_is_a_single_character() {
        for ic in [
            &crate::icons::NERD,
            &crate::icons::UNICODE,
            &crate::icons::ASCII,
        ] {
            for f in ic.spin {
                assert_eq!(
                    f.chars().count(),
                    1,
                    "{:?}: frame {f:?} is not one character",
                    ic.tier
                );
            }
            assert!(ic.spin.len() >= 2, "{:?}: nothing to animate", ic.tier);
        }
    }

    /// Without this the pulse could quietly become a whole-screen flicker rather
    /// than a signal about one row.
    #[test]
    fn only_the_in_progress_glyph_pulses() {
        let mut done = task("done", None, "Finished task");
        done.completed = true;
        let mut blocked = task("blocked", None, "Blocked task");
        blocked.blocked_by = vec!["pending".into()];

        let tasks = || {
            vec![
                task("pending", None, "Pending task"),
                done.clone(),
                blocked.clone(),
            ]
        };

        for ic in crate::icons::ALL {
            assert_eq!(
                render_frame(tasks(), 0, &ic),
                render_frame(tasks(), 3, &ic),
                "tier {} repaints with nothing running",
                crate::icons::name(ic.tier)
            );
        }
    }

    /// A pulse repaint can now land while a dialog is open, which never happened
    /// before. Immediate-mode rendering makes it safe by construction -- the
    /// prompt is redrawn from `app.mode` with the same value -- but "safe by
    /// construction" is an argument, and this is a check.
    #[test]
    fn a_pulse_repaint_does_not_disturb_an_open_prompt() {
        let ic = &crate::icons::UNICODE;

        let frame = |pulse_on: bool| {
            let mut app = App::new(
                vec![started("b", "Running task")],
                "demo".into(),
                crate::config::Config::default(),
            );
            app.mode = Mode::Prompt(crate::app::Prompt {
                title: "Rename: Running task".into(),
                label: "Name".into(),
                input: crate::app::TextInput::new("half-typed"),
                pending: crate::app::Pending::EditName { id: "b".into() },
            });
            app.spin_frame = if pulse_on { 1 } else { 0 };

            let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
            terminal.draw(|f| draw(f, &mut app, ic)).unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            (terminal.backend().buffer().clone(), cursor)
        };

        let (off, off_cursor) = frame(false);
        let (on, on_cursor) = frame(true);

        assert_eq!(off_cursor, on_cursor, "the cursor moved mid-typing");
        let text = |b: &ratatui::buffer::Buffer| {
            (0..b.area.height)
                .map(|y| {
                    (0..b.area.width)
                        .map(|x| b[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(text(&off), text(&on), "the prompt redrew differently");
    }

    /// Draws a real frame with `select` selected and `focus` focused, and hands
    /// back both the styled buffer and the `App` the renderer wrote its geometry
    /// into. Styling is the entire subject of the selection tests, and the plain
    /// `render` helper throws it away.
    fn render_selection(
        tasks: Vec<Task>,
        select: &str,
        focus: Focus,
        ic: &Icons,
        w: u16,
        h: u16,
    ) -> (ratatui::buffer::Buffer, App) {
        let mut app = App::new(tasks, "demo".into(), crate::config::Config::default());
        app.filter = tree::Filter::All;
        // This helper is about the tree/detail split and predates the sidebar;
        // callers' column arithmetic assumes the tree pane starts at column 0.
        app.repos_pane_above = 0;
        app.rebuild();
        app.selected = Some(select.to_string());
        app.focus = focus;

        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, &mut app, ic)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (buf, app)
    }

    /// The buffer row `name` was drawn on.
    fn row_of(buf: &ratatui::buffer::Buffer, name: &str) -> u16 {
        for y in 0..buf.area.height {
            let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            if line.contains(name) {
                return y;
            }
        }
        panic!("{name:?} was never drawn");
    }

    /// The column `name` starts at on row `y`.
    ///
    /// Counted in cells, not bytes: the tree draws multibyte box characters, and
    /// a byte offset would differ between two rows whose names line up perfectly
    /// on screen.
    fn col_of(buf: &ratatui::buffer::Buffer, y: u16, name: &str) -> usize {
        let cells: Vec<&str> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        let line: String = cells.concat();
        let byte = line
            .find(name)
            .unwrap_or_else(|| panic!("{name:?} is not on row {y}: {line:?}"));
        let mut at = 0;
        for (i, c) in cells.iter().enumerate() {
            if at == byte {
                return i;
            }
            at += c.len();
        }
        panic!("{name:?} does not start on a cell boundary on row {y}")
    }

    /// Every cell of row `y` that lies inside the tree pane.
    fn tree_cells<'a>(
        buf: &'a ratatui::buffer::Buffer,
        app: &App,
        y: u16,
    ) -> Vec<&'a ratatui::buffer::Cell> {
        (1..app.divider_x).map(|x| &buf[(x, y)]).collect()
    }

    /// Selection is carried by a rail in the left margin and a bold name, not by
    /// inverting the row. Inversion is the thing being replaced, so its absence
    /// is asserted rather than assumed -- and the gutter is a *reserved* column
    /// on every row, so selecting one must not shift its name relative to its
    /// siblings.
    #[test]
    fn the_selected_row_is_marked_by_a_gutter_not_by_inverting_it() {
        let ic = &crate::icons::UNICODE;
        let tasks = vec![
            task("alpha", None, "Alpha task"),
            task("beta", None, "Beta task"),
        ];

        let (buf, app) = render_selection(tasks, "alpha", Focus::Tree, ic, 100, 20);
        let sel = row_of(&buf, "Alpha task");
        let other = row_of(&buf, "Beta task");

        // x = 0 is the pane border, so the gutter is the first cell inside it.
        assert_eq!(buf[(1, sel)].symbol(), ic.gutter, "no gutter on the selection");
        assert_eq!(buf[(1, other)].symbol(), " ", "an unselected row drew a gutter");

        for cell in tree_cells(&buf, &app, sel) {
            assert!(
                !cell.style().add_modifier.contains(Modifier::REVERSED),
                "the selected row still inverts: {cell:?}"
            );
        }

        assert_eq!(
            col_of(&buf, sel, "Alpha task"),
            col_of(&buf, other, "Beta task"),
            "selecting a row moved its name out of the column"
        );

        let name_x = col_of(&buf, sel, "Alpha task") as u16;
        assert!(
            buf[(name_x, sel)]
                .style()
                .add_modifier
                .contains(Modifier::BOLD),
            "the selected name is not bold"
        );
        assert!(
            !buf[(col_of(&buf, other, "Beta task") as u16, other)]
                .style()
                .add_modifier
                .contains(Modifier::BOLD),
            "an unselected name is bold"
        );
    }

    /// Focus follows the border, and the gutter has to agree with it -- otherwise
    /// two panes both claim a cursor. Easy to forget, because the tree is focused
    /// by default and every other test would pass without this.
    #[test]
    fn the_selection_gutter_dims_when_the_tree_is_unfocused() {
        let ic = &crate::icons::UNICODE;
        let tasks = || vec![task("alpha", None, "Alpha task")];

        let (focused, _) = render_selection(tasks(), "alpha", Focus::Tree, ic, 100, 20);
        let y = row_of(&focused, "Alpha task");
        assert_eq!(focused[(1, y)].style().fg, Some(crate::theme::ACCENT));

        let (unfocused, _) = render_selection(tasks(), "alpha", Focus::Detail, ic, 100, 20);
        let y = row_of(&unfocused, "Alpha task");
        assert_eq!(unfocused[(1, y)].style().fg, Some(crate::theme::ACCENT_DIM));
        assert_eq!(
            unfocused[(1, y)].symbol(),
            ic.gutter,
            "an unfocused pane still knows where the cursor is"
        );
    }

    /// The reason inversion had to go. The status glyph and all three meter runs
    /// set explicit foregrounds, and `REVERSED` swapped every one of them with
    /// the background -- so on the selected row the colour language inverted
    /// exactly where progress is quantified. This is the row that has both.
    #[test]
    fn selection_does_not_recolour_the_meter_or_the_status_glyph() {
        let ic = &crate::icons::UNICODE;

        let mut finished = task("done", Some("root"), "Finished child");
        finished.completed = true;
        let mut running = task("run", Some("root"), "Running child");
        running.started_at = Some("2026-01-01T00:00:00Z".into());

        let tasks = vec![
            task("root", None, "Parent task"),
            finished,
            running,
            task("todo", Some("root"), "Pending child"),
        ];

        let (buf, app) = render_selection(tasks, "root", Focus::Tree, ic, 120, 20);
        let y = row_of(&buf, "Parent task");
        let cells = tree_cells(&buf, &app, y);

        assert_eq!(buf[(1, y)].symbol(), ic.gutter, "fixture: the parent is selected");

        let fg_of = |sym: &str| -> Vec<Option<Color>> {
            cells
                .iter()
                .filter(|c| c.symbol() == sym)
                .map(|c| c.style().fg)
                .collect()
        };

        // 1 done + 1 in flight of 3 gives two green cells, two blue, a blue
        // partial and a dim remainder -- every colour the meter can speak.
        assert!(
            fg_of("\u{2588}").contains(&Some(DONE)),
            "no green in the meter: {:?}",
            fg_of("\u{2588}")
        );
        assert!(
            fg_of("\u{2588}").contains(&Some(ACTIVE)),
            "no blue in the meter: {:?}",
            fg_of("\u{2588}")
        );
        assert_eq!(
            fg_of("\u{2591}"),
            vec![Some(DIM); 2],
            "the untouched remainder lost its colour"
        );

        // The parent is neither started nor completed nor blocked, so its own
        // marker is the yellow todo glyph.
        assert_eq!(
            fg_of(ic.pending),
            vec![Some(status_color(Status::Pending))],
            "the status glyph was recoloured by the selection"
        );

        for cell in &cells {
            assert!(
                !cell.style().add_modifier.contains(Modifier::REVERSED),
                "inversion would swap every one of those foregrounds: {cell:?}"
            );
        }
    }

    /// Nothing else ties the renderer's actual output to the hit test: the
    /// `app.rs` click tests use a hand-written geometry stand-in. Written before
    /// the selection gutter existed, so a layout change that broke click
    /// mapping would show up here as a flip from green to red.
    #[test]
    fn a_click_still_selects_the_row_that_was_drawn() {
        let ic = &crate::icons::UNICODE;
        let tasks = vec![
            task("alpha", None, "Alpha task"),
            task("beta", None, "Beta task"),
        ];

        let (buf, mut app) = render_selection(tasks, "alpha", Focus::Tree, ic, 100, 20);
        let y = row_of(&buf, "Beta task");

        app.select_at_row(y);
        assert_eq!(
            app.selected.as_deref(),
            Some("beta"),
            "clicking row {y} selected {:?}",
            app.selected
        );
    }

    /// The tree pane's content for each row, without the two pane borders.
    fn tree_rows(rows: &[String]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let mut it = r.match_indices('│');
                match (it.next(), it.next()) {
                    (Some((a, _)), Some((b, _))) => r[a + '│'.len_utf8()..b].to_string(),
                    _ => String::new(),
                }
            })
            .collect()
    }

    /// A rollup over nothing is meaningless, so a leaf gets no meter at all.
    /// `░` is safe to look for: ratatui's scrollbar draws `█` and `│`, never it.
    #[test]
    fn a_leaf_gets_no_meter() {
        let rows = tree_rows(&render(120, 20, &crate::icons::UNICODE));
        let row = |name: &str| {
            rows.iter()
                .find(|r| r.contains(name))
                .unwrap_or_else(|| panic!("no row for {name}:\n{}", rows.join("\n")))
                .clone()
        };
        assert!(row("Parent task").contains('░'), "{:?}", row("Parent task"));
        assert!(!row("Child task").contains('░'), "{:?}", row("Child task"));
    }

    /// Rollups come from the *unfiltered* task list: hiding completed subtasks
    /// is exactly when you most want to see how many there were.
    #[test]
    fn a_meter_counts_the_unfiltered_tree() {
        let mut finished = task("done", Some("root"), "Finished child");
        finished.completed = true;

        let app_tasks = vec![
            task("root", None, "Parent task"),
            finished,
            task("kid", Some("root"), "Pending child"),
        ];
        let mut app = App::new(app_tasks, "demo".into(), crate::config::Config::default());
        assert_eq!(app.filter, tree::Filter::Pending, "fixture assumes the default filter");
        // About the tree pane's rollup, not the sidebar; `tree_rows` below
        // assumes the tree pane starts at column 0.
        app.repos_pane_above = 0;

        let mut terminal = Terminal::new(TestBackend::new(120, 20)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let rows = tree_rows(&rows);
        let text = rows.join("\n");

        assert!(!text.contains("Finished child"), "the filter should hide it:\n{text}");
        let parent = rows.iter().find(|r| r.contains("Parent task")).unwrap();
        assert!(
            parent.contains("1/2"),
            "the rollup must count the hidden child: {parent:?}"
        );
    }

    /// The sidebar draws, and shows both levels.
    #[test]
    fn the_sidebar_shows_repos_with_their_worktrees() {
        let mut app = App::new(
            vec![task("a", None, "A task")],
            "demo".into(),
            crate::config::Config::default(),
        );
        app.terminal_width = 140;
        app.repos_pane_above = 110;
        app.repos = vec![crate::repos::Repo {
            name: "dextui".into(),
            path: "/x/dextui".into(),
            worktrees: vec![crate::worktree::Worktree {
                path: "/x/dextui".into(),
                branch: "main".into(),
                is_main: true,
                is_locked: false,
                is_detached: false,
            }],
            open: true,
            registered: true,
            is_global: false,
        }];
        app.rebuild();

        let mut terminal = Terminal::new(TestBackend::new(140, 14)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("dextui"), "no repo row: {text}");
        assert!(text.contains("main"), "no worktree row: {text}");
    }

    /// The bug this closes: `render_widget` (no `ListState`) always draws the
    /// list from its very first row, so `G`/`PageDown` could move
    /// `selected_repo_row` to a row below the visible area with nothing on
    /// screen ever scrolling to show it -- and `enter` would then switch to a
    /// store the user could not see was selected. A `ListState`, mirroring
    /// `draw_tree`'s exactly, is what makes the pane follow the selection.
    #[test]
    fn selecting_a_row_below_the_fold_scrolls_the_sidebar_to_show_it() {
        let mut app = App::new(
            vec![task("a", None, "A task")],
            "demo".into(),
            crate::config::Config::default(),
        );
        app.terminal_width = 140;
        app.repos_pane_above = 110;
        app.repos = (0..20)
            .map(|i| crate::repos::Repo {
                name: format!("repo{i}"),
                path: format!("/x/repo{i}"),
                worktrees: vec![],
                open: false,
                registered: true,
                is_global: false,
            })
            .collect();
        app.rebuild();

        let render = |app: &mut App| -> String {
            let mut terminal = Terminal::new(TestBackend::new(140, 14)).unwrap();
            terminal.draw(|f| draw(f, app, &crate::icons::UNICODE)).unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };

        let first_frame = render(&mut app);
        assert!(first_frame.contains("repo0"), "the top row should be visible initially");
        assert!(
            !first_frame.contains("repo19"),
            "the fixture should not already fit everything: {first_frame}"
        );

        app.select_last_repo_row();
        let scrolled = render(&mut app);

        assert!(
            scrolled.contains("repo19"),
            "the selected row must have scrolled into view: {scrolled}"
        );
        assert!(
            !scrolled.contains("repo0"),
            "the old top row should have scrolled out of view: {scrolled}"
        );
    }

    /// `Focus::Repos` used to fall through to `draw_tree` as a placeholder --
    /// harmless while nothing could set that focus, wrong the moment something
    /// can. In single-pane mode the repos pane must actually be the one drawn.
    #[test]
    fn single_pane_repos_focus_draws_the_repos_pane_not_the_tree() {
        let mut app = App::new(
            vec![task("a", None, "Only Task")],
            "demo".into(),
            crate::config::Config::default(),
        );
        app.single_pane_below = 9999; // force one pane regardless of width
        app.focus = Focus::Repos;
        app.rebuild();

        let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
        terminal
            .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
            .unwrap();

        let buf = terminal.backend().buffer();
        let text: String = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("repos"), "repos pane title missing:\n{text}");
        assert!(
            !text.contains("Only Task"),
            "the tree, not the repos pane, was drawn:\n{text}"
        );
    }

    /// Every rung of the ladder draws without panicking, including the boundaries.
    #[test]
    fn every_width_draws_without_panicking() {
        for w in [40u16, 79, 80, 109, 110, 160] {
            let mut app = App::new(
                vec![task("a", None, "A task")],
                "demo".into(),
                crate::config::Config::default(),
            );
            app.terminal_width = w;
            app.rebuild();
            let mut terminal = Terminal::new(TestBackend::new(w, 14)).unwrap();
            terminal
                .draw(|f| draw(f, &mut app, &crate::icons::UNICODE))
                .unwrap();
        }
    }
}

