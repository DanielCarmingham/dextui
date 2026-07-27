//! dex-tui — a two-pane terminal browser for dex tasks.

mod app;
mod dex;
mod tree;
mod ui;
mod watch;

use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread;

use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use app::{App, Mode, Pending, Prompt, TextInput};
use dex::{store_label, Dex, Task};

/// Everything the main loop reacts to, from every thread, on one channel.
enum Msg {
    Input(CtEvent),
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

    let mut app = App::new(tasks, store_label(&store_dir));

    if std::env::args().any(|a| a == "--selftest") {
        println!("store   {store_dir}");
        print!("{}", ui::selftest(&app));
        return Ok(());
    }

    let (tx, rx) = channel::<Msg>();

    // Input thread: blocking reads forwarded onto the single event channel.
    {
        let tx = tx.clone();
        thread::spawn(move || {
            while let Ok(ev) = event::read() {
                if tx.send(Msg::Input(ev)).is_err() {
                    return;
                }
            }
        });
    }

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

    let mut terminal = ratatui::init();

    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, &app))?;

        let Ok(msg) = rx.recv() else { break };
        match msg {
            Msg::Input(CtEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                handle_key(&mut app, key, &dex, &tx);
            }
            // Resize and everything else just falls through to a redraw.
            Msg::Input(_) => {}

            Msg::StoreChanged => {
                if app.is_modal() {
                    // Deferred rather than dropped; applied when the dialog closes.
                    app.pending_refresh = true;
                } else {
                    refresh(&dex, &tx);
                }
            }

            Msg::Tasks(Ok(tasks)) => app.apply_tasks(tasks),
            // Keep the last good model rather than blanking the view.
            Msg::Tasks(Err(e)) => app.status = format!("refresh failed: {}", flatten(&e)),

            Msg::Ok(message) => {
                app.status = message;
                refresh(&dex, &tx);
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

    ratatui::restore();
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

        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
        KeyCode::PageDown => app.move_selection(10),
        KeyCode::PageUp => app.move_selection(-10),
        KeyCode::Right | KeyCode::Char('l') => app.expand_selected(),
        KeyCode::Left | KeyCode::Char('h') => app.collapse_selected(),
        KeyCode::Char('g') => app.select_first(),
        KeyCode::Char('G') => app.select_last(),
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
                    title: format!("Edit: {}", t.name),
                    label: "Name".into(),
                    input: TextInput::new(&t.name),
                    pending: Pending::EditName { id: t.id.clone() },
                });
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
            let current = app
                .by_id
                .get(&id)
                .and_then(|t| t.description.clone())
                .unwrap_or_default();
            app.mode = Mode::Prompt(Prompt {
                title: p.title,
                label: "Description".into(),
                input: TextInput::new(&current),
                pending: Pending::EditDescription { id, name: value },
            });
        }
        Pending::EditDescription { id, name } => {
            let shown = name.clone();
            act(dex, tx, format!("updated {shown}"), move |d| {
                d.edit(&id, Some(&name), Some(&value))
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
