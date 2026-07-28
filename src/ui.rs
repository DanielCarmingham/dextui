//! Immediate-mode rendering. Everything is redrawn from `App` each frame.
//!
//! This is the only module that knows about colour: `markdown` and `tree` emit
//! neutral descriptions of what things *are*, and the `Palette` decides how they
//! look.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Clear, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Wrap,
};
use ratatui::Frame;

use crate::app::{App, Mode};
use crate::icons::Icons;
use crate::dex::{age, local_time, Status, Task};
use crate::markdown::{self, Emphasis};
use crate::theme::Palette;
use crate::tree::{self, Progress};

const SHORTCUTS: &str =
    " s start  c complete  e edit  n new  a subtask  d delete  f filter  / find  ? help  q quit";

/// Width of the inline progress meter, in cells.
const METER_WIDTH: usize = 7;

pub fn draw(frame: &mut Frame, app: &App, p: &Palette, ic: &Icons) {
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, app, p, ic, top);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Fill(1)]).areas(body);

    draw_tree(frame, app, p, ic, left);
    draw_detail(frame, app, p, ic, right);
    draw_status(frame, app, p, bottom);

    match &app.mode {
        Mode::Prompt(prompt) => draw_prompt(frame, prompt, p),
        Mode::Confirm { message, .. } => draw_message(
            frame,
            "Delete task",
            message,
            "enter delete    esc cancel",
            p.blocked,
            p,
        ),
        Mode::ForceComplete { message, .. } => draw_message(
            frame,
            "Incomplete subtasks",
            message,
            "enter force    esc cancel",
            p.active,
            p,
        ),
        Mode::Error(e) => draw_message(frame, "dex error", e, "any key to dismiss", p.blocked, p),
        Mode::Help => draw_help(frame, p),
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

fn status_color(s: Status, p: &Palette) -> Color {
    match s {
        Status::Completed => p.done,
        Status::InProgress => p.active,
        Status::Pending => p.pending,
    }
}

/// A compact meter plus the raw fraction, e.g. `▓▓▓░░░░ 2/7`.
///
/// The number is shown alongside the bar on purpose: at seven cells a bar cannot
/// distinguish 2/7 from 3/7, and for triage the exact count is the useful part.
fn meter_spans(progress: Progress, p: &Palette, ic: &Icons) -> Vec<Span<'static>> {
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
        Span::styled(ic.meter_done.repeat(done), Style::default().fg(p.meter_full)),
        Span::styled(ic.meter_active.repeat(active), Style::default().fg(p.active)),
        Span::styled(
            ic.meter_empty.repeat(METER_WIDTH - done - active),
            Style::default().fg(p.meter_empty),
        ),
        Span::styled(
            format!(" {}/{}", progress.done, progress.total),
            Style::default().fg(p.label),
        ),
    ]
}

/// A segment of the header: text plus its own background.
struct Seg {
    text: String,
    fg: Color,
    bg: Color,
    bold: bool,
}

fn seg_text(icon: &str, text: &str) -> String {
    if icon.is_empty() {
        format!(" {text} ")
    } else {
        format!(" {icon} {text} ")
    }
}

/// The header: app identity, which store you are in, and what is outstanding.
///
/// With a Nerd Font the segments are joined by powerline separators; otherwise
/// they simply abut, which still reads as distinct blocks because each carries
/// its own background. Nothing here degrades to tofu.
///
/// While searching, the search box takes the whole line over. That is why the
/// header costs no vertical space: the two are never needed at once.
fn draw_header(frame: &mut Frame, app: &App, p: &Palette, ic: &Icons, area: Rect) {
    if matches!(app.mode, Mode::Search) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " search ",
                    Style::default()
                        .bg(p.head_app_bg)
                        .fg(p.head_app_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if ic.has_powerline() { ic.sep } else { "" },
                    Style::default().fg(p.head_app_bg),
                ),
                Span::raw(" "),
                Span::styled(app.query.value.clone(), Style::default().fg(p.active)),
            ])),
            area,
        );

        let lead = if ic.has_powerline() { 10 } else { 9 };
        frame.set_cursor_position(Position {
            x: (area.x + lead + app.query.cursor as u16).min(area.right().saturating_sub(1)),
            y: area.y,
        });
        return;
    }

    let (pending, active) = app.counts();

    let segments = [
        Seg {
            text: seg_text(ic.app, "dex-tui"),
            fg: p.head_app_fg,
            bg: p.head_app_bg,
            bold: true,
        },
        Seg {
            text: seg_text(ic.project, &app.store_label),
            fg: p.head_ctx_fg,
            bg: p.head_ctx_bg,
            bold: false,
        },
        Seg {
            text: seg_text("", &format!("{pending} pending · {active} active")),
            fg: p.head_info_fg,
            bg: p.head_info_bg,
            bold: false,
        },
    ];

    let mut spans: Vec<Span> = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let mut style = Style::default().fg(seg.fg).bg(seg.bg);
        if seg.bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(seg.text.clone(), style));

        if ic.has_powerline() {
            // The separator is drawn in this segment's colour on the *next*
            // segment's background, which is what makes the join look solid.
            let next_bg = segments.get(i + 1).map(|n| n.bg).unwrap_or(Color::Reset);
            spans.push(Span::styled(
                ic.sep,
                Style::default().fg(seg.bg).bg(next_bg),
            ));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    // Filter state stays right-aligned, rendered over the same row.
    frame.render_widget(
        Paragraph::new(
            Line::from(vec![
                Span::styled(app.filter.label(), Style::default().fg(p.label)),
                Span::raw(" "),
            ])
            .right_aligned(),
        ),
        area,
    );
}

fn draw_tree(frame: &mut Frame, app: &App, p: &Palette, ic: &Icons, area: Rect) {
    // No title: the header already says which store this is, and repeating it
    // on the pane border was the same fact twice.
    let block = Block::bordered().border_style(Style::default().fg(p.chrome));

    let inner_width = area.width.saturating_sub(2) as usize;
    let rows = tree::visible_rows(&app.tree, &app.expanded);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let t = &row.node.task;

            let mut spans = vec![
                Span::styled(row.prefix.clone(), Style::default().fg(p.chrome)),
                Span::styled(
                    format!("{} ", ic.marker(row.has_children, row.is_open)),
                    Style::default().fg(p.chrome),
                ),
                Span::styled(
                    format!("{} ", glyph(t.status(), ic)),
                    Style::default().fg(status_color(t.status(), p)),
                ),
            ];

            let name_style = if !row.node.is_match {
                // Scaffolding: kept only because a descendant matched.
                Style::default().fg(p.label)
            } else if t.completed {
                Style::default()
                    .fg(p.label)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default().fg(p.pending)
            };

            spans.push(Span::styled(t.name.clone(), name_style));
            if t.is_blocked() {
                spans.push(Span::styled(
                    format!(" {}", ic.blocked),
                    Style::default().fg(p.blocked),
                ));
            }

            // Right gutter: a rollup for parents, otherwise how long this has been
            // in flight. Only in-progress tasks get an age -- putting one on every
            // row would bury the signal it exists to give.
            let trailing: Vec<Span> = match app.progress.get(&t.id) {
                Some(progress) => meter_spans(*progress, p, ic),
                None if t.status() == Status::InProgress => match age(&t.started_at) {
                    Some(a) => vec![Span::styled(a, Style::default().fg(p.active))],
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

    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_style(Style::default().bg(p.selection).add_modifier(Modifier::BOLD)),
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
                .track_style(Style::default().fg(p.chrome))
                .thumb_style(Style::default().fg(p.label)),
            area,
            &mut sb,
        );
    }
}

fn draw_detail(frame: &mut Frame, app: &App, p: &Palette, ic: &Icons, area: Rect) {
    let block = Block::bordered().border_style(Style::default().fg(p.chrome));

    let Some(task) = app.selected_task() else {
        let msg = if app.tasks.is_empty() {
            "No tasks yet.\n\nPress n to create one."
        } else {
            "No tasks match the current filter.\n\nPress f to change it, or clear the search."
        };
        frame.render_widget(
            Paragraph::new(msg)
                .block(block)
                .style(Style::default().fg(p.label))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    };

    frame.render_widget(
        Paragraph::new(detail_lines(task, app, p, ic))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Built entirely from the already-fetched list. `dex show` is never called,
/// because selection changes on every arrow key and a ~180ms process spawn per
/// keypress would make navigation unusable.
fn detail_lines<'a>(t: &'a Task, app: &'a App, p: &'a Palette, ic: &Icons) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(Span::styled(
            t.name.clone(),
            Style::default().fg(p.title).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "─".repeat(t.name.chars().count().clamp(8, 60)),
            Style::default().fg(p.chrome),
        )),
    ];

    // One status line reads faster than three separate label/value rows.
    let mut summary = vec![Span::styled(
        format!("{} {}", glyph(t.status(), ic), t.status().label()),
        Style::default().fg(status_color(t.status(), p)),
    )];

    if t.status() == Status::InProgress
        && let Some(a) = age(&t.started_at) {
            summary.push(Span::styled(" · ", Style::default().fg(p.chrome)));
            summary.push(Span::styled(
                format!("started {a} ago"),
                Style::default().fg(p.active),
            ));
        }

    summary.push(Span::styled(" · ", Style::default().fg(p.chrome)));
    summary.push(Span::styled(
        format!("priority {}", t.priority),
        Style::default().fg(p.label),
    ));
    lines.push(Line::from(summary));

    if let Some(progress) = app.progress.get(&t.id) {
        lines.push(Line::from(""));
        let mut row = meter_spans(*progress, p, ic);
        row.push(Span::styled(
            format!(
                "  subtask{} done",
                if progress.total == 1 { "" } else { "s" }
            ),
            Style::default().fg(p.label),
        ));
        lines.push(Line::from(row));
    }

    lines.push(Line::from(""));

    let mut field = |k: &str, v: String, style: Style| {
        lines.push(Line::from(vec![
            Span::styled(format!("{k:<10}"), Style::default().fg(p.label)),
            Span::styled(v, style),
        ]));
    };

    field("id", t.id.clone(), Style::default().fg(p.label));

    if let Some(parent) = t.parent_id.as_ref().and_then(|id| app.by_id.get(id)) {
        field("parent", parent.name.clone(), Style::default().fg(p.pending));
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
        field("blocked", names.join(", "), Style::default().fg(p.blocked));
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
        Style::default().fg(p.pending),
    );
    if t.completed_at.is_some() {
        field("done", stamp(&t.completed_at), Style::default().fg(p.done));
    }

    if let Some(d) = t.description.as_ref().filter(|d| !d.trim().is_empty()) {
        lines.push(Line::from(""));
        lines.extend(markdown_lines(d, p));
    }

    if let Some(r) = t.result.as_ref().filter(|r| !r.trim().is_empty()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "result",
            Style::default().fg(p.label),
        )));
        for line in r.lines() {
            lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(p.done),
            )));
        }
    }

    lines
}

/// Applies the palette to what `markdown` identified.
fn markdown_lines(text: &str, p: &Palette) -> Vec<Line<'static>> {
    markdown::parse(text)
        .into_iter()
        .map(|segments| {
            Line::from(
                segments
                    .into_iter()
                    .map(|s| {
                        let style = match s.emphasis {
                            Emphasis::Plain => Style::default().fg(p.pending),
                            Emphasis::Marker => Style::default().fg(p.md_marker),
                            Emphasis::Heading => Style::default()
                                .fg(p.md_heading)
                                .add_modifier(Modifier::BOLD),
                            Emphasis::Bold => Style::default().add_modifier(Modifier::BOLD),
                            Emphasis::Code | Emphasis::CodeBlock => Style::default().fg(p.md_code),
                            Emphasis::Quote => Style::default()
                                .fg(p.md_quote)
                                .add_modifier(Modifier::ITALIC),
                        };
                        Span::styled(s.text, style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn draw_status(frame: &mut Frame, app: &App, p: &Palette, area: Rect) {
    let (text, style) = if app.status.is_empty() {
        (SHORTCUTS.to_string(), Style::default().fg(p.label))
    } else {
        (format!(" {}", app.status), Style::default().fg(p.active))
    };

    frame.render_widget(Paragraph::new(text).style(style), area);
}

fn draw_prompt(frame: &mut Frame, prompt: &crate::app::Prompt, p: &Palette) {
    let area = centered(frame.area(), 70, 7);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(format!(" {} ", prompt.title))
        .border_style(Style::default().fg(p.active));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [label_area, input_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(prompt.label.clone()).style(Style::default().fg(p.label)),
        label_area,
    );
    frame.render_widget(
        Paragraph::new(prompt.input.value.clone()).style(Style::default().fg(p.title)),
        input_area,
    );
    frame.render_widget(
        Paragraph::new("enter confirm    esc cancel").style(Style::default().fg(p.label)),
        hint_area,
    );

    frame.set_cursor_position(Position {
        x: (input_area.x + prompt.input.cursor as u16).min(input_area.right().saturating_sub(1)),
        y: input_area.y,
    });
}

fn draw_message(frame: &mut Frame, title: &str, body: &str, hint: &str, accent: Color, p: &Palette) {
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
            .style(Style::default().fg(p.pending))
            .wrap(Wrap { trim: false }),
        body_area,
    );
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(p.label)),
        hint_area,
    );
}

fn draw_help(frame: &mut Frame, p: &Palette) {
    // Left-aligned on purpose: centring would destroy the column alignment.
    const HELP: &str = "\
↑ ↓ j k    move              s   start task
→ ← h l    expand/collapse   c   complete (prompts for result)
g / G      first / last      e   edit name, then description
/          search            n   new top-level task
f          cycle filter      a   new subtask of selection
z          collapse all      d   delete (with confirmation)
Z          expand all        r   refresh now

The view refreshes itself whenever the dex store changes, including when
another process or agent edits it. Your selection, expansion and any open
dialog are never disturbed.";

    let area = centered(frame.area(), 74, 16);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(" dex-tui ")
        .border_style(Style::default().fg(p.active));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [body, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    frame.render_widget(
        Paragraph::new(HELP).style(Style::default().fg(p.pending)),
        body,
    );
    frame.render_widget(
        Paragraph::new("any key to dismiss").style(Style::default().fg(p.label)),
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
    let p = &crate::theme::CALM;
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
        let forest = tree::build(&app.tasks, "", filter);
        let count = tree::flatten(&forest).len();
        let _ = writeln!(out, "--- filter: {filter:?} ({count} visible) ---");
        for node in &forest {
            print_node(node, 0, app, ic, &mut out);
        }
        let _ = writeln!(out);
    }

    if let Some(first) = app.tasks.first() {
        let _ = writeln!(out, "--- detail pane for {} ---", first.name);
        for line in detail_lines(first, app, p, ic) {
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
        Some(p) => format!("  {}/{}", p.done, p.total),
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
            priority: 1,
            completed: false,
            result: None,
            created_at: Some("2026-01-01T00:00:00Z".into()),
            started_at: None,
            completed_at: None,
            blocked_by: vec![],
            children: vec![],
        }
    }

    /// Renders a full frame and returns it as plain text, one String per row.
    fn render(w: u16, h: u16, ic: &Icons) -> Vec<String> {
        let app = App::new(
            vec![
                task("root", None, "Parent task"),
                task("kid", Some("root"), "Child task"),
            ],
            "demo".into(),
        );

        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| draw(f, &app, &crate::theme::CALM, ic))
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

    #[test]
    fn a_very_narrow_pane_does_not_panic() {
        // The right-hand gutter must be dropped rather than overflow the row.
        for w in [20u16, 30, 40] {
            let _ = render(w, 12, &crate::icons::UNICODE);
        }
    }
}
