//! dex-tui — a two-pane terminal browser for dex tasks.

mod app;
mod config;
mod dex;
mod editor;
mod icons;
mod markdown;
mod tree;
mod ui;
mod watch;

use std::sync::mpsc::{channel, Sender};
use std::time::Duration;
use std::sync::Arc;
use std::thread;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;

use app::{App, Focus, Mode, Pending, Prompt, TextInput};
use dex::{store_label, Dex, Task};

/// Everything the main loop reacts to, from every thread, on one channel.
enum Msg {
    StoreChanged,
    Tasks(Result<Vec<Task>, String>),
    Ok(String),
    Failed(String),
    /// `dex complete` was rejected; carries what is needed to retry with --force.
    CompleteRejected {
        id: String,
        result: String,
        error: String,
    },
}

fn main() -> std::io::Result<()> {
    let dex = Arc::new(Dex::real());

    // Preflight before taking over the terminal, so a failure prints plainly
    // instead of leaving a half-initialised TUI behind.
    let store_dir = match dex.store_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("dex-tui: {e}");
            eprintln!("dex-tui: is `dex` installed and on your PATH?");
            std::process::exit(1);
        }
    };

    let tasks = match dex.list() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("dex-tui: {e}");
            std::process::exit(1);
        }
    };

    let (cfg, config_problem) = config::load();

    if std::env::args().any(|a| a == "--config") {
        let mark = |p: Option<std::path::PathBuf>| match p {
            Some(p) => {
                let state = if p.exists() { "present" } else { "not present" };
                format!("{}  ({state})", p.display())
            }
            None => "(could not resolve)".to_string(),
        };
        println!("# global   {}", mark(config::path()));
        println!("# project  {}\n", mark(config::project_path()));
        print!("{}", config::EXAMPLE);
        return Ok(());
    }

    let mut app = App::new(tasks, store_label(&store_dir), cfg);
    if let Some(msg) = config_problem {
        app.status = format!("config: {msg}");
    }

    if std::env::args().any(|a| a == "--icons") {
        println!("Set with DEXTUI_ICONS=<tier>\n");
        for i in icons::ALL {
            println!(
                "  {:<9} {}  {}{}{}{}  {}",
                icons::name(i.tier),
                icons::about(i.tier),
                i.pending,
                i.active,
                i.done,
                i.blocked,
                i.expanded,
            );
        }
        return Ok(());
    }

    if std::env::args().any(|a| a == "--selftest") {
        println!("store   {store_dir}");
        print!("{}", ui::selftest(&app));
        return Ok(());
    }

    let (tx, rx) = channel::<Msg>();

    // Watcher. Kept alive for the whole run; dropping it stops notifications.
    let (watch_tx, watch_rx) = channel::<()>();
    let _watcher = watch::spawn(&store_dir, watch_tx);
    {
        let tx = tx.clone();
        thread::spawn(move || {
            while watch_rx.recv().is_ok() {
                if tx.send(Msg::StoreChanged).is_err() {
                    return;
                }
            }
        });
    }

    let glyphs = cfg.icons;
    let mut terminal = ratatui::init();
    // ratatui::init only sets raw mode and the alternate screen; mouse
    // reporting is opt-in. While captured, the terminal stops doing its own
    // text selection -- hold Shift to bypass it, as most terminals allow.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);

    // Polled rather than run from a reader thread. A thread blocked in
    // `event::read()` would swallow the first keystroke intended for $EDITOR,
    // because both it and the child would be reading the same terminal.
    let mut dirty = true;
    while !app.should_quit {
        if dirty {
            terminal.draw(|f| ui::draw(f, &mut app, &glyphs))?;
            dirty = false;
        }

        // The timeout bounds how long a store change waits to be noticed; it is
        // not a redraw interval, since nothing is drawn unless something changed.
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                CtEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(&mut app, key, &dex, &tx);
                    dirty = true;
                }
                CtEvent::Mouse(m) => {
                    handle_mouse(&mut app, m);
                    dirty = true;
                }
                CtEvent::Resize(..) => dirty = true,
                _ => {}
            }
        }

        while let Ok(msg) = rx.try_recv() {
            handle_msg(&mut app, msg, &dex, &tx);
            dirty = true;
        }

        // Requested by `E`. Runs here, outside the draw, so the terminal can be
        // handed over cleanly and restored afterwards.
        if let Some(id) = app.pending_editor.take() {
            run_editor(&mut terminal, &mut app, &id, &dex, &tx)?;
            dirty = true;
        }
    }

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    Ok(())
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if app.on_divider(m.column) {
                app.dragging_split = true;
            } else if app.in_body(m.row) {
                // Click to focus, and in the tree also to select the row.
                if m.column < app.divider_x {
                    app.focus = Focus::Tree;
                    app.select_at_row(m.row);
                } else {
                    app.focus = Focus::Detail;
                }
            }
        }

        MouseEventKind::Drag(MouseButton::Left) if app.dragging_split => {
            app.set_split(m.column, app.terminal_width);
        }

        MouseEventKind::Up(_) => app.dragging_split = false,

        // The wheel acts on whichever pane is under the pointer, which is what
        // people expect regardless of where focus happens to be.
        MouseEventKind::ScrollDown => {
            if m.column < app.divider_x {
                app.move_selection(1);
            } else {
                app.scroll_detail(1, 0);
            }
        }
        MouseEventKind::ScrollUp => {
            if m.column < app.divider_x {
                app.move_selection(-1);
            } else {
                app.scroll_detail(-1, 0);
            }
        }
        MouseEventKind::ScrollLeft => app.scroll_detail(0, -4),
        MouseEventKind::ScrollRight => app.scroll_detail(0, 4),

        _ => {}
    }
}

fn handle_msg(app: &mut App, msg: Msg, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    match msg {
        Msg::StoreChanged => {
            if app.is_modal() {
                // Deferred rather than dropped; applied when the dialog closes.
                app.pending_refresh = true;
            } else {
                refresh(dex, tx);
            }
        }

        Msg::Tasks(Ok(tasks)) => app.apply_tasks(tasks),
        // Keep the last good model rather than blanking the view.
        Msg::Tasks(Err(e)) => app.status = format!("refresh failed: {}", flatten(&e)),

        Msg::Ok(message) => {
            app.status = message;
            refresh(dex, tx);
        }
        Msg::Failed(e) => app.mode = Mode::Error(flatten(&e)),

        Msg::CompleteRejected { id, result, error } => {
            app.mode = Mode::ForceComplete {
                id,
                result,
                message: flatten(&error),
            };
        }
    }
}

/// Leaves the TUI, runs the editor, restores the TUI, and writes back only if
/// the text actually changed.
fn run_editor(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    id: &str,
    dex: &Arc<Dex>,
    tx: &Sender<Msg>,
) -> std::io::Result<()> {
    let current = app
        .by_id
        .get(id)
        .and_then(|t| t.description.clone())
        .unwrap_or_default();

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    let outcome = editor::edit(id, &current);
    *terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    terminal.clear()?;

    match outcome {
        Ok(Some(new_text)) => {
            let id = id.to_string();
            act(dex, tx, "description updated".into(), move |d| {
                d.edit(&id, None, Some(&new_text))
            });
        }
        Ok(None) => app.status = "description unchanged".into(),
        Err(e) => app.mode = Mode::Error(flatten(&e.to_string())),
    }

    Ok(())
}

fn flatten(s: &str) -> String {
    s.replace(['\n', '\r'], " ").trim().to_string()
}

fn refresh(dex: &Arc<Dex>, tx: &Sender<Msg>) {
    let dex = Arc::clone(dex);
    let tx = tx.clone();
    thread::spawn(move || {
        let _ = tx.send(Msg::Tasks(dex.list()));
    });
}

/// Runs a dex write off the UI thread and reports the outcome.
fn act<F>(dex: &Arc<Dex>, tx: &Sender<Msg>, success: String, f: F)
where
    F: FnOnce(&Dex) -> Result<(), String> + Send + 'static,
{
    let dex = Arc::clone(dex);
    let tx = tx.clone();
    thread::spawn(move || {
        let msg = match f(&dex) {
            Ok(()) => Msg::Ok(success),
            Err(e) => Msg::Failed(e),
        };
        let _ = tx.send(msg);
    });
}

fn close_modal(app: &mut App, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    app.mode = Mode::Normal;
    if app.pending_refresh {
        app.pending_refresh = false;
        refresh(dex, tx);
    }
}

fn handle_key(app: &mut App, key: KeyEvent, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    match app.mode.clone() {
        Mode::Normal => handle_normal(app, key, dex, tx),
        Mode::Search => handle_search(app, key),
        Mode::Prompt(p) => handle_prompt(app, key, p, dex, tx),

        Mode::Confirm { id, .. } => {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char('y')) {
                let name = app.by_id.get(&id).map(|t| t.name.clone()).unwrap_or_default();
                act(dex, tx, format!("deleted {name}"), move |d| d.delete(&id));
            }
            close_modal(app, dex, tx);
        }

        Mode::ForceComplete { id, result, .. } => {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char('y')) {
                act(dex, tx, "completed".to_string(), move |d| {
                    d.complete(&id, &result, true)
                });
            }
            close_modal(app, dex, tx);
        }

        Mode::Error(_) | Mode::Help => close_modal(app, dex, tx),
    }
}

fn handle_normal(app: &mut App, key: KeyEvent, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    let selected = app.selected_task().cloned();

    // Clear any transient status as soon as the user does something else.
    app.status.clear();

    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true
        }
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,

        KeyCode::Tab | KeyCode::BackTab => app.toggle_focus(),
        // Wrapping and horizontal scrolling are mutually exclusive, so this is
        // the switch between reading prose and reading a wide table.
        KeyCode::Char('w') => app.toggle_wrap(),
        KeyCode::Char('o') => app.cycle_sort(),
        KeyCode::Char('O') => app.toggle_sort_direction(),

        // Movement drives whichever pane has focus. Action keys below stay
        // global, because they always act on the selected task.
        KeyCode::Down | KeyCode::Char('j') => match app.focus {
            Focus::Tree => app.move_selection(1),
            Focus::Detail => app.scroll_detail(1, 0),
        },
        KeyCode::Up | KeyCode::Char('k') => match app.focus {
            Focus::Tree => app.move_selection(-1),
            Focus::Detail => app.scroll_detail(-1, 0),
        },
        KeyCode::PageDown => match app.focus {
            Focus::Tree => app.move_selection(10),
            Focus::Detail => app.scroll_detail(10, 0),
        },
        KeyCode::PageUp => match app.focus {
            Focus::Tree => app.move_selection(-10),
            Focus::Detail => app.scroll_detail(-10, 0),
        },
        KeyCode::Right | KeyCode::Char('l') => match app.focus {
            Focus::Tree => app.expand_selected(),
            Focus::Detail => app.scroll_detail(0, 4),
        },
        KeyCode::Left | KeyCode::Char('h') => match app.focus {
            Focus::Tree => app.collapse_selected(),
            Focus::Detail => app.scroll_detail(0, -4),
        },
        KeyCode::Char('g') => match app.focus {
            Focus::Tree => app.select_first(),
            Focus::Detail => app.detail_to_top(),
        },
        KeyCode::Char('G') => match app.focus {
            Focus::Tree => app.select_last(),
            Focus::Detail => app.detail_to_bottom(),
        },
        KeyCode::Char('z') => app.collapse_all(),
        KeyCode::Char('Z') => app.expand_all(),

        KeyCode::Char('/') => app.mode = Mode::Search,
        KeyCode::Char('f') => {
            app.filter = app.filter.next();
            app.rebuild();
        }
        KeyCode::Char('r') => refresh(dex, tx),
        KeyCode::Char('?') => app.mode = Mode::Help,

        KeyCode::Char('n') => {
            app.mode = Mode::Prompt(Prompt {
                title: "New task".into(),
                label: "Name".into(),
                input: TextInput::default(),
                pending: Pending::CreateName { parent: None },
            })
        }

        KeyCode::Char('s') => {
            if let Some(t) = selected {
                let id = t.id.clone();
                act(dex, tx, format!("started {}", t.name), move |d| d.start(&id));
            }
        }
        KeyCode::Char('a') => {
            if let Some(t) = selected {
                app.mode = Mode::Prompt(Prompt {
                    title: format!("New subtask of: {}", t.name),
                    label: "Name".into(),
                    input: TextInput::default(),
                    pending: Pending::CreateName {
                        parent: Some(t.id.clone()),
                    },
                });
            }
        }
        KeyCode::Char('c') => {
            if let Some(t) = selected {
                app.mode = Mode::Prompt(Prompt {
                    title: format!("Complete: {}", t.name),
                    label: "Result".into(),
                    input: TextInput::default(),
                    pending: Pending::Complete { id: t.id.clone() },
                });
            }
        }
        KeyCode::Char('e') => {
            if let Some(t) = selected {
                app.mode = Mode::Prompt(Prompt {
                    title: format!("Rename: {}", t.name),
                    label: "Name".into(),
                    input: TextInput::new(&t.name),
                    pending: Pending::EditName { id: t.id.clone() },
                });
            }
        }
        // A single-line field cannot honestly edit a multi-line description.
        KeyCode::Char('E') => {
            if let Some(t) = selected {
                app.pending_editor = Some(t.id.clone());
            }
        }
        KeyCode::Char('d') => {
            if let Some(t) = selected {
                let kids = app
                    .tasks
                    .iter()
                    .filter(|x| x.parent_id.as_deref() == Some(t.id.as_str()))
                    .count();
                let message = if kids > 0 {
                    format!("\"{}\" and its {kids} subtask(s) will be deleted.", t.name)
                } else {
                    format!("\"{}\" will be deleted.", t.name)
                };
                app.mode = Mode::Confirm {
                    id: t.id.clone(),
                    message,
                };
            }
        }
        _ => {}
    }
}

fn handle_search(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Enter => app.mode = Mode::Normal,
        KeyCode::Backspace => {
            app.query.backspace();
            app.rebuild();
        }
        KeyCode::Left => app.query.left(),
        KeyCode::Right => app.query.right(),
        KeyCode::Char(c) => {
            app.query.insert(c);
            app.rebuild();
        }
        _ => {}
    }
}

fn handle_prompt(app: &mut App, key: KeyEvent, mut p: Prompt, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    match key.code {
        KeyCode::Esc => close_modal(app, dex, tx),
        KeyCode::Enter => submit(app, p, dex, tx),
        KeyCode::Backspace => {
            p.input.backspace();
            app.mode = Mode::Prompt(p);
        }
        KeyCode::Left => {
            p.input.left();
            app.mode = Mode::Prompt(p);
        }
        KeyCode::Right => {
            p.input.right();
            app.mode = Mode::Prompt(p);
        }
        KeyCode::Char(c) => {
            p.input.insert(c);
            app.mode = Mode::Prompt(p);
        }
        _ => {}
    }
}

fn submit(app: &mut App, p: Prompt, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    let value = p.input.value.clone();

    match p.pending {
        // Two-step flows chain into a second prompt rather than acting yet.
        Pending::CreateName { parent } => {
            if value.trim().is_empty() {
                close_modal(app, dex, tx);
                return;
            }
            app.mode = Mode::Prompt(Prompt {
                title: p.title,
                label: "Description (may be left empty)".into(),
                input: TextInput::default(),
                pending: Pending::CreateDescription {
                    parent,
                    name: value,
                },
            });
        }
        Pending::CreateDescription { parent, name } => {
            let shown = name.clone();
            act(dex, tx, format!("created {shown}"), move |d| {
                d.create(&name, &value, parent.as_deref())
            });
            close_modal(app, dex, tx);
        }

        Pending::EditName { id } => {
            if value.trim().is_empty() {
                close_modal(app, dex, tx);
                return;
            }
            let shown = value.clone();
            act(dex, tx, format!("renamed to {shown}"), move |d| {
                d.edit(&id, Some(&value), None)
            });
            close_modal(app, dex, tx);
        }

        Pending::Complete { id } => {
            // dex refuses to complete a task with unfinished subtasks unless
            // forced. Detect that specific rejection and offer the retry, rather
            // than making the user retype the result.
            let dex2 = Arc::clone(dex);
            let tx2 = tx.clone();
            let result = value;
            thread::spawn(move || {
                let msg = match dex2.complete(&id, &result, false) {
                    Ok(()) => Msg::Ok("completed".to_string()),
                    Err(e) if e.to_lowercase().contains("subtask") => Msg::CompleteRejected {
                        id,
                        result,
                        error: e,
                    },
                    Err(e) => Msg::Failed(e),
                };
                let _ = tx2.send(msg);
            });

            close_modal(app, dex, tx);
            app.status = "completing…".into();
        }
    }
}
