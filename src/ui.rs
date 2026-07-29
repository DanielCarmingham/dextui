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

use crate::app::{App, Focus, Mode};

/// Colour is used only where it carries meaning. Everything else is left to the
/// terminal, so the app inherits whatever scheme the user runs -- including a
/// light/dark switch at runtime -- instead of imposing its own. The values live
/// in `theme`; this module decides where they go.
use crate::theme::{
    ACCENT, ACCENT_DIM, ACTIVE, ACTIVE_PULSE, BLOCKED, CODE, DIM, DONE, PLAIN, TODO,
};

use crate::icons::Icons;
use crate::dex::{self, age, local_time, Status, Task};
use crate::tree::{self, Progress};

const SHORTCUTS: &str =
    " s start  c done  e rename  E edit  n new  a sub  d del  f filter  o sort  , config  ? help";

/// Width of the inline progress meter, in cells.
const METER_WIDTH: usize = 7;

pub fn draw(frame: &mut Frame, app: &mut App, ic: &Icons) {
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, ic, top);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(app.split_percent), Constraint::Fill(1)])
            .areas(body);

    // Published for mouse handling: the divider sits where the two borders meet.
    app.divider_x = right.x;
    app.terminal_width = frame.area().width;
    app.body_top = body.y;
    app.body_bottom = body.y + body.height;

    draw_tree(frame, app, ic, left);
    draw_detail(frame, app, ic, right);
    draw_status(frame, app, bottom);

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
        Mode::Help => draw_help(frame),
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

fn status_color(s: Status) -> Color {
    match s {
        Status::Completed => DONE,
        Status::InProgress => ACTIVE,
        Status::Blocked => BLOCKED,
        Status::Pending => TODO,
    }
}

/// The status marker's style for the current animation frame.
///
/// **The glyph never changes shape; only its intensity breathes.** A marker that
/// changed shape between frames would shift the column every task name lines up
/// in -- the same failure that rules out a braille spinner, which macOS
/// substitutes at 1.11 cells.
///
/// Only in progress pulses. It is the one state that is *happening*, and making
/// the rest move too would turn a signal into screen flicker.
fn status_style(s: Status, pulse: bool) -> Style {
    if s == Status::InProgress && pulse {
        Style::default()
            .fg(ACTIVE_PULSE)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(status_color(s))
    }
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
    let m = &ic.meter;
    let bar = Bar::new(progress, METER_WIDTH, !m.partial.is_empty());

    let run = |glyphs: [&'static str; 3], from: usize, len: usize| -> String {
        (from..from + len).map(|i| glyphs[cap(i, METER_WIDTH)]).collect()
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

    spans.push(Span::styled(
        format!(" {}/{}", progress.done, progress.total),
        Style::default().fg(DIM),
    ));
    spans
}

/// The header: app identity, which store you are in, and what is outstanding.
///
/// Plain text with dim separators, no coloured bands. The app does not impose a
/// look; the terminal's own scheme shows through.
///
/// While searching, the search box takes the whole line over. That is why the
/// header costs no vertical space: the two are never needed at once.
fn draw_header(frame: &mut Frame, app: &App, ic: &Icons, area: Rect) {
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

    let (pending, active) = app.counts();
    let sep = || Span::styled(" · ", Style::default().fg(DIM));

    let mut spans = vec![Span::raw(" ")];
    if !ic.app.is_empty() {
        spans.push(Span::styled(format!("{} ", ic.app), Style::default().fg(DIM)));
    }
    spans.push(Span::styled(
        "dextui",
        Style::default().add_modifier(Modifier::BOLD),
    ));
    spans.push(sep());

    if !ic.project.is_empty() {
        spans.push(Span::styled(format!("{} ", ic.project), Style::default().fg(DIM)));
    }
    spans.push(Span::styled(app.store_label.clone(), Style::default().fg(PLAIN)));
    spans.push(sep());

    spans.push(Span::styled(
        format!("{pending} pending"),
        Style::default().fg(DIM),
    ));
    spans.push(sep());
    spans.push(Span::styled(
        format!("{active} active"),
        Style::default().fg(ACTIVE),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    frame.render_widget(
        Paragraph::new(
            Line::from(vec![
                Span::styled(
                    app.sort.label(app.sort_reversed),
                    Style::default().fg(DIM),
                ),
                Span::styled("  ", Style::default()),
                Span::styled(app.filter.label(), Style::default().fg(DIM)),
                Span::raw(" "),
            ])
            .right_aligned(),
        ),
        area,
    );
}

fn draw_tree(frame: &mut Frame, app: &mut App, ic: &Icons, area: Rect) {
    // No title: the header already says which store this is, and repeating it
    // on the pane border was the same fact twice.
    let block = Block::bordered().border_style(Style::default().fg(if app.focus
        == Focus::Tree
    {
        PLAIN
    } else {
        DIM
    }));

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

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let t = &row.node.task;
            let is_selected = selected == Some(i);

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
                    format!("{} ", glyph(dex::status(t, &app.by_id), ic)),
                    status_style(dex::status(t, &app.by_id), app.pulse_on),
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
            if dex::is_blocked(t, &app.by_id) && dex::status(t, &app.by_id) != Status::Blocked {
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
                let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                let tail: usize = trailing.iter().map(|s| s.content.chars().count()).sum();
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

fn draw_detail(frame: &mut Frame, app: &mut App, ic: &Icons, area: Rect) {
    let focused = app.focus == Focus::Detail;
    let block = Block::bordered()
        .title(if app.wrap { "" } else { " no wrap " })
        .title_style(Style::default().fg(DIM))
        .border_style(Style::default().fg(if focused { PLAIN } else { DIM }));

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
            summary.push(Span::styled(" · ", Style::default().fg(DIM)));
            summary.push(Span::styled(
                format!("started {a} ago"),
                Style::default().fg(ACTIVE),
            ));
        }

    // How long it actually took, which reads better than two raw timestamps.
    if let Some(took) = t.worked_duration() {
        summary.push(Span::styled(" · ", Style::default().fg(DIM)));
        summary.push(Span::styled(
            format!("took {took}"),
            Style::default().fg(DONE),
        ));
    }

    summary.push(Span::styled(" · ", Style::default().fg(DIM)));
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
    let stamp = |iso: &Option<String>| match age(iso).as_deref() {
        // "now ago" reads as a bug; anything under a minute is just now.
        Some("now") => format!("{}  (just now)", local_time(iso)),
        Some(a) => format!("{}  ({a} ago)", local_time(iso)),
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
        (SHORTCUTS.to_string(), Style::default().fg(DIM))
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

fn draw_help(frame: &mut Frame) {
    // Left-aligned on purpose: centring would destroy the column alignment.
    const HELP: &str = "\
tab        switch pane       s   start task
↑ ↓ j k    move / scroll     c   complete (prompts for result)
→ ← h l    expand / scroll   e   rename
g / G      first / last      E   edit description in $EDITOR
w          toggle wrap       n   new top-level task
o / O      sort / reverse    a   new subtask of selection
/          search            d   delete (with confirmation)
f          cycle filter      r   refresh now
,          edit config       q   quit
z Z        collapse/expand all

Movement follows the focused pane, shown by its brighter border. Turn wrap
off (w) to scroll a wide table sideways -- wrapping removes the overflow
there would otherwise be to scroll to.

Mouse: drag the divider to resize, wheel scrolls the pane under the pointer,
click selects. Hold Shift to select text, as capture is enabled.

The view refreshes itself whenever the dex store changes, including when
another process or agent edits it. Your selection, expansion and any open
dialog are never disturbed.";

    let area = centered(frame.area(), 74, 16);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(" dextui ")
        .border_style(Style::default().fg(ACTIVE));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [body, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    frame.render_widget(
        Paragraph::new(HELP).style(Style::default().fg(PLAIN)),
        body,
    );
    frame.render_widget(
        Paragraph::new("any key to dismiss").style(Style::default().fg(DIM)),
        hint,
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

    let (pending, active) = app.counts();
    let _ = writeln!(out, "label   {}", app.store_label);
    let _ = writeln!(
        out,
        "tasks   {} ({pending} pending, {active} active)\n",
        app.tasks.len()
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
        assert!(
            started_row.contains(ic.active),
            "a started task reads as in progress: {started_row:?}"
        );
        assert_eq!(
            started_row.matches(ic.blocked).count(),
            1,
            "the glyph cannot say blocked here, so the marker must: {started_row:?}"
        );
    }

    /// Renders a full frame and returns it as plain text, one String per row.
    fn render(w: u16, h: u16, ic: &Icons) -> Vec<String> {
        let mut app = App::new(
            vec![
                task("root", None, "Parent task"),
                task("kid", Some("root"), "Child task"),
            ],
            "demo".into(),
            crate::config::Config::default(),
        );

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
                ("ACTIVE_PULSE", ACTIVE_PULSE),
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
        assert!(rows[0].contains("pending"), "header row: {:?}", rows[0]);
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
    /// `render` helper throws away -- and styling is the entire subject here.
    fn render_frame(tasks: Vec<Task>, pulse_on: bool, ic: &Icons) -> ratatui::buffer::Buffer {
        let mut app = App::new(tasks, "demo".into(), crate::config::Config::default());
        app.filter = tree::Filter::All;
        app.rebuild();
        app.pulse_on = pulse_on;

        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal.draw(|f| draw(f, &mut app, ic)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// Where `symbol` is drawn, and how, in the tree pane.
    fn find_cell(buf: &ratatui::buffer::Buffer, symbol: &str) -> ((u16, u16), Style) {
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if buf[(x, y)].symbol() == symbol {
                    return ((x, y), buf[(x, y)].style());
                }
            }
        }
        panic!("{symbol:?} was never drawn");
    }

    /// The shape assertion is the point. A glyph that changed between frames
    /// would shift the column every task name lines up in, which is the exact
    /// failure that rules out a braille spinner here.
    #[test]
    fn an_in_progress_row_renders_in_both_phases_without_changing_shape() {
        let ic = &crate::icons::UNICODE;
        let tasks = || vec![task("a", None, "Idle task"), started("b", "Running task")];

        let (off_at, off_style) = find_cell(&render_frame(tasks(), false, ic), ic.active);
        let (on_at, on_style) = find_cell(&render_frame(tasks(), true, ic), ic.active);

        assert_eq!(off_at, on_at, "the marker moved between frames");

        assert_eq!(off_style.fg, Some(ACTIVE));
        assert!(!off_style.add_modifier.contains(Modifier::BOLD));

        assert_eq!(on_style.fg, Some(crate::theme::ACTIVE_PULSE));
        assert!(on_style.add_modifier.contains(Modifier::BOLD));
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
                render_frame(tasks(), false, &ic),
                render_frame(tasks(), true, &ic),
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
            app.pulse_on = pulse_on;

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
}

