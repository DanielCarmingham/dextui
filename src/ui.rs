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
/// light/dark switch at runtime -- instead of imposing its own.
///
/// Only ANSI-16 names and `Reset` appear here: they are remapped by the user's
/// terminal theme. Fixed `Indexed`/`Rgb` values, and `White`/`Black` for text,
/// can only ever suit one background.
const PLAIN: Color = Color::Reset;
const DIM: Color = Color::DarkGray;
const ACTIVE: Color = Color::Yellow;
const DONE: Color = Color::Green;
const BLOCKED: Color = Color::Red;
const CODE: Color = Color::Cyan;
use crate::icons::Icons;
use crate::dex::{age, local_time, Status, Task};
use crate::tree::{self, Progress};

const SHORTCUTS: &str =
    " s start  c done  e rename  E edit  n new  a sub  d del  f filter  o sort  / find  ? help";

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
        Layout::horizontal([Constraint::Percentage(45), Constraint::Fill(1)]).areas(body);

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
        Status::Pending => ic.pending,
    }
}

fn status_color(s: Status) -> Color {
    match s {
        Status::Completed => DONE,
        Status::InProgress => ACTIVE,
        Status::Pending => PLAIN,
    }
}

/// A compact meter plus the raw fraction, e.g. `▓▓▓░░░░ 2/7`.
///
/// The number is shown alongside the bar on purpose: at seven cells a bar cannot
/// distinguish 2/7 from 3/7, and for triage the exact count is the useful part.
fn meter_spans(progress: Progress, ic: &Icons) -> Vec<Span<'static>> {
    let cells = |n: usize| -> usize {
        if n == 0 {
            0
        } else {
            // Anything non-zero gets at least one cell, so a single finished or
            // in-flight subtask is never rounded away to an empty bar.
            ((n as f64 / progress.total as f64) * METER_WIDTH as f64).round().max(1.0) as usize
        }
    };

    let done = cells(progress.done).min(METER_WIDTH);
    let active = cells(progress.active).min(METER_WIDTH - done);

    vec![
        Span::styled(ic.meter_done.repeat(done), Style::default().fg(DONE)),
        Span::styled(ic.meter_active.repeat(active), Style::default().fg(ACTIVE)),
        Span::styled(
            ic.meter_empty.repeat(METER_WIDTH - done - active),
            Style::default().fg(DIM),
        ),
        Span::styled(
            format!(" {}/{}", progress.done, progress.total),
            Style::default().fg(DIM),
        ),
    ]
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
        "dex-tui",
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

fn draw_tree(frame: &mut Frame, app: &App, ic: &Icons, area: Rect) {
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

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let t = &row.node.task;

            let mut spans = vec![
                Span::styled(row.prefix.clone(), Style::default().fg(DIM)),
                Span::styled(
                    format!("{} ", ic.marker(row.has_children, row.is_open)),
                    Style::default().fg(DIM),
                ),
                Span::styled(
                    format!("{} ", glyph(t.status(), ic)),
                    Style::default().fg(status_color(t.status())),
                ),
            ];

            let name_style = if !row.node.is_match {
                // Scaffolding: kept only because a descendant matched.
                Style::default().fg(DIM)
            } else if t.completed {
                Style::default()
                    .fg(DIM)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default().fg(PLAIN)
            };

            spans.push(Span::styled(t.name.clone(), name_style));
            if t.is_blocked() {
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
                None if t.status() == Status::InProgress => match age(&t.started_at) {
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

    let mut state = ListState::default();
    state.select(app.selected_row());

    // REVERSED inverts whatever the terminal's current colours are, so the
    // selected row stays readable in light and dark alike. A fixed background
    // can only ever be right for one of them.
    // REVERSED inverts whatever the terminal's current colours are, so the
    // selected row is readable on a light and a dark background alike. A fixed
    // background can only ever be right for one of them.
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );

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
    let mut summary = vec![Span::styled(
        format!("{} {}", glyph(t.status(), ic), t.status().label()),
        Style::default().fg(status_color(t.status())),
    )];

    if t.status() == Status::InProgress
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

    if t.is_blocked() {
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
→ ← h l    expand / scroll   e   edit name, then description
g / G      first / last      n   new top-level task
w          toggle wrap       E   edit description in $EDITOR
o / O      sort / reverse    a   new subtask of selection
/          search            d   delete (with confirmation)
f          cycle filter      r   refresh now
z Z        collapse/expand all

Movement follows the focused pane, shown by its brighter border. Turn wrap
off (w) to scroll a wide table sideways -- wrapping removes the overflow
there would otherwise be to scroll to.

The view refreshes itself whenever the dex store changes, including when
another process or agent edits it. Your selection, expansion and any open
dialog are never disturbed.";

    let area = centered(frame.area(), 74, 16);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(" dex-tui ")
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

/// Plain-text render of the whole pipeline, for `--selftest`.
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
        glyph(node.task.status(), ic),
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
        assert!(rows[0].contains("dex-tui"), "header row: {:?}", rows[0]);
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
        for w in [20u16, 30, 40] {
            let _ = render(w, 12, &crate::icons::UNICODE);
        }
    }
}

