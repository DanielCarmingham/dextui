//! Immediate-mode rendering. Everything is redrawn from `App` each frame.

use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Mode};
use crate::dex::{local_time, Status, Task};
use crate::tree;

const SHORTCUTS: &str =
    " s start  c complete  e edit  n new  a subtask  d delete  f filter  / find  ? help  q quit";

pub fn draw(frame: &mut Frame, app: &App) {
    let [top, body, bottom] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_filter_bar(frame, app, top);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Fill(1)]).areas(body);

    draw_tree(frame, app, left);
    draw_detail(frame, app, right);
    draw_status(frame, app, bottom);

    match &app.mode {
        Mode::Prompt(p) => draw_prompt(frame, p),
        Mode::Confirm { message, .. } => {
            draw_message(frame, "Delete task", message, "enter delete    esc cancel", Color::Red)
        }
        Mode::ForceComplete { message, .. } => draw_message(
            frame,
            "Incomplete subtasks",
            message,
            "enter force    esc cancel",
            Color::Yellow,
        ),
        Mode::Error(e) => draw_message(frame, "dex error", e, "any key to dismiss", Color::Red),
        Mode::Help => draw_help(frame),
        _ => {}
    }
}

fn draw_filter_bar(frame: &mut Frame, app: &App, area: Rect) {
    let label = app.filter.label();
    let searching = matches!(app.mode, Mode::Search);

    let query_style = if searching {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let left = Line::from(vec![
        Span::styled(" / ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.query.value.clone(), query_style),
    ]);

    frame.render_widget(Paragraph::new(left), area);
    frame.render_widget(
        Paragraph::new(Line::from(label).right_aligned()).style(Style::default().fg(Color::Cyan)),
        area,
    );

    if searching {
        // Sits after " / ".
        let x = area.x + 3 + app.query.cursor as u16;
        frame.set_cursor_position(Position {
            x: x.min(area.right().saturating_sub(1)),
            y: area.y,
        });
    }
}

fn draw_tree(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!(" {} ", app.store_label);
    let block = Block::bordered().title(title).border_style(
        if matches!(app.mode, Mode::Normal) {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    );

    let rows = tree::visible_rows(&app.tree, &app.expanded);

    let items: Vec<ListItem> = rows
        .iter()
        .map(|row| {
            let t = &row.node.task;
            let mut spans = vec![
                Span::styled(row.prefix.clone(), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} ", t.status().glyph()),
                    Style::default().fg(status_color(t.status())),
                ),
            ];

            let name_style = if !row.node.is_match {
                // Scaffolding: kept only because a descendant matched.
                Style::default().fg(Color::DarkGray)
            } else if t.completed {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::default()
            };

            spans.push(Span::styled(t.name.clone(), name_style));

            if t.is_blocked() {
                spans.push(Span::styled(" ⊘", Style::default().fg(Color::Red)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut state = ListState::default();
    state.select(app.selected_row());

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::Indexed(238))
            .add_modifier(Modifier::BOLD),
    );

    frame.render_stateful_widget(list, area, &mut state);
}

fn status_color(s: Status) -> Color {
    match s {
        Status::Completed => Color::Green,
        Status::InProgress => Color::Yellow,
        Status::Pending => Color::DarkGray,
    }
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().border_style(Style::default().fg(Color::DarkGray));

    let Some(task) = app.selected_task() else {
        let msg = if app.tasks.is_empty() {
            "No tasks yet.\n\nPress n to create one."
        } else {
            "No tasks match the current filter.\n\nPress f to change it, or clear the search."
        };
        frame.render_widget(
            Paragraph::new(msg)
                .block(block)
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    };

    frame.render_widget(
        Paragraph::new(detail_lines(task, app))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Built entirely from the already-fetched list. `dex show` is never called,
/// because selection changes on every arrow key and a ~180ms process spawn per
/// keypress would make navigation unusable.
fn detail_lines<'a>(t: &'a Task, app: &'a App) -> Vec<Line<'a>> {
    let mut lines = vec![
        Line::from(Span::styled(
            t.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "─".repeat(t.name.chars().count().clamp(8, 60)),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
    ];

    let mut field = |k: &str, v: String, style: Style| {
        lines.push(Line::from(vec![
            Span::styled(format!("{k:<10}"), Style::default().fg(Color::DarkGray)),
            Span::styled(v, style),
        ]));
    };

    field("id", t.id.clone(), Style::default().fg(Color::DarkGray));
    field(
        "status",
        format!("{} {}", t.status().glyph(), t.status().label()),
        Style::default().fg(status_color(t.status())),
    );
    field("priority", t.priority.to_string(), Style::default());

    if let Some(parent) = t.parent_id.as_ref().and_then(|p| app.by_id.get(p)) {
        field("parent", parent.name.clone(), Style::default());
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
        field("blocked", names.join(", "), Style::default().fg(Color::Red));
    }

    field("created", local_time(&t.created_at), Style::default());
    if t.started_at.is_some() {
        field("started", local_time(&t.started_at), Style::default());
    }
    if t.completed_at.is_some() {
        field("done", local_time(&t.completed_at), Style::default());
    }

    // Full text, never truncated -- the whole point of a detail pane.
    if let Some(d) = t.description.as_ref().filter(|d| !d.trim().is_empty()) {
        lines.push(Line::from(""));
        for l in d.lines() {
            lines.push(Line::from(l));
        }
    }

    if let Some(r) = t.result.as_ref().filter(|r| !r.trim().is_empty()) {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "result",
            Style::default().fg(Color::DarkGray),
        )));
        for l in r.lines() {
            lines.push(Line::from(Span::styled(
                l,
                Style::default().fg(Color::Green),
            )));
        }
    }

    lines
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (text, style) = if app.status.is_empty() {
        (SHORTCUTS.to_string(), Style::default().fg(Color::DarkGray))
    } else {
        (
            format!(" {}", app.status),
            Style::default().fg(Color::Yellow),
        )
    };

    frame.render_widget(Paragraph::new(text).style(style), area);
}

fn draw_prompt(frame: &mut Frame, p: &crate::app::Prompt) {
    let area = centered(frame.area(), 70, 7);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(format!(" {} ", p.title))
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [label_area, input_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(p.label.clone()).style(Style::default().fg(Color::DarkGray)),
        label_area,
    );
    frame.render_widget(Paragraph::new(p.input.value.clone()), input_area);
    frame.render_widget(
        Paragraph::new("enter confirm    esc cancel").style(Style::default().fg(Color::DarkGray)),
        hint_area,
    );

    frame.set_cursor_position(Position {
        x: (input_area.x + p.input.cursor as u16).min(input_area.right().saturating_sub(1)),
        y: input_area.y,
    });
}

fn draw_message(frame: &mut Frame, title: &str, body: &str, hint: &str, colour: Color) {
    let area = centered(frame.area(), 66, 9);
    frame.render_widget(Clear, area);

    let block = Block::bordered()
        .title(format!(" {title} "))
        .border_style(Style::default().fg(colour));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [body_area, hint_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    frame.render_widget(
        Paragraph::new(body.to_string()).wrap(Wrap { trim: false }),
        body_area,
    );
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        hint_area,
    );
}

fn draw_help(frame: &mut Frame) {
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
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [body, hint] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    frame.render_widget(Paragraph::new(HELP), body);
    frame.render_widget(
        Paragraph::new("any key to dismiss").style(Style::default().fg(Color::DarkGray)),
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

    let _ = writeln!(out, "label   {}", app.store_label);
    let _ = writeln!(out, "tasks   {}\n", app.tasks.len());

    for filter in [
        crate::tree::Filter::All,
        crate::tree::Filter::Pending,
        crate::tree::Filter::InProgress,
    ] {
        let forest = tree::build(&app.tasks, "", filter);
        let count = tree::flatten(&forest).len();
        let _ = writeln!(out, "--- filter: {filter:?} ({count} visible) ---");
        for node in &forest {
            print_node(node, 0, &mut out);
        }
        let _ = writeln!(out);
    }

    if let Some(first) = app.tasks.first() {
        let _ = writeln!(out, "--- detail pane for {} ---", first.name);
        for line in detail_lines(first, app) {
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

fn print_node(node: &tree::Node, depth: usize, out: &mut String) {
    use std::fmt::Write;
    let scaffold = if node.is_match { "" } else { "  (scaffold)" };
    let _ = writeln!(
        out,
        "{}{} {}{}",
        "  ".repeat(depth),
        node.task.status().glyph(),
        node.task.name,
        scaffold
    );
    for c in &node.children {
        print_node(c, depth + 1, out);
    }
}
