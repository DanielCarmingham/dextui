//! dextui — a two-pane terminal browser for dex tasks.

mod app;
mod config;
mod dex;
mod editor;
mod icons;
mod markdown;
mod pulse;
mod registry;
mod theme;
mod tree;
mod ui;
mod watch;
mod worktree;

use std::sync::mpsc::{channel, Sender};
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

const USAGE: &str = "\
dextui — browse and triage dex tasks

USAGE:
    dextui [COMMAND]

With no command, runs the TUI against the dex store for the current directory.

COMMANDS:
    config              Show the config paths and a commented template
    config init         Write a config template
    config edit         Open a config in $EDITOR, creating it if needed
    icons               List the glyph tiers
    selftest            Print the data pipeline as text and exit (no TUI)

OPTIONS:
    -h, --help          Show this help
    -V, --version       Show the version

CONFIG OPTIONS:
    -g, --global        Act on ~/.config/dextui/config.toml (the default)
    -l, --local         Act on .dextui.toml at the git root
        --project       Alias for --local
        --force         With `config init`, overwrite an existing file

    Settings layer defaults < global < project < environment. Inside the app,
    `,` opens the global config in $EDITOR and reloads it when you save.
";

#[derive(Debug)]
enum Command {
    Run,
    Help,
    Version,
    ShowConfig,
    InitConfig { force: bool, scope: config::Scope },
    EditConfig { scope: config::Scope },
    Icons,
    SelfTest,
}

/// Subcommands rather than flags, and `-l`/`-g` rather than invented spellings,
/// so the vocabulary matches `dex` itself -- these two are always used together.
///
/// Hand-rolled: the surface is five commands, and the point is that an
/// unrecognised argument is an error rather than a silent fall-through into
/// launching the TUI.
fn parse_args() -> Result<Command, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse(&args)
}

/// What to print when there is no terminal to draw into.
///
/// `ratatui::init` **panics** rather than returning an error when the terminal
/// cannot be put into raw mode, so without this check piping dextui anywhere --
/// or running it from a script, a CI job, or an editor's task runner -- ends in
/// a Rust backtrace. The docs promised a blank screen; a panic looks like a bug
/// in the app rather than a misuse of it.
///
/// `selftest` exists precisely so the data path can be inspected without a
/// terminal, so it is worth pointing at here.
fn requires_a_terminal() -> String {
    "dextui: this needs a real terminal, and stdout is not one.\n\n\
     It draws a full-screen interface, so it cannot render into a pipe, a file,\n\
     or a job with no terminal attached.\n\n\
     To inspect the data without a terminal, run `dextui selftest`."
        .to_string()
}

fn parse(args: &[String]) -> Result<Command, String> {
    let mut words: Vec<&str> = Vec::new();
    let mut force = false;
    let mut scope = config::Scope::Global;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--force" => force = true,
            "-l" | "--local" | "--project" => scope = config::Scope::Project,
            "-g" | "--global" => scope = config::Scope::Global,
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other:?}"));
            }
            other => words.push(other),
        }
    }

    match words.as_slice() {
        [] => Ok(Command::Run),
        ["config"] => Ok(Command::ShowConfig),
        ["config", "init"] => Ok(Command::InitConfig { force, scope }),
        ["config", "edit"] => Ok(Command::EditConfig { scope }),
        ["config", other] => Err(format!(
            "unknown config command {other:?}; expected `init` or `edit`"
        )),
        ["icons"] => Ok(Command::Icons),
        ["selftest"] => Ok(Command::SelfTest),
        [other, ..] => Err(format!("unknown command {other:?}")),
    }
}

fn main() -> std::io::Result<()> {
    let command = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dextui: {e}\n");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };

    // Handled before touching dex or the terminal, so they work anywhere.
    match command {
        Command::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        Command::Version => {
            println!("dextui {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Command::ShowConfig => {
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
        Command::InitConfig { force, scope } => match config::init(scope, force) {
            Ok(p) => {
                println!("wrote {}", p.display());
                return Ok(());
            }
            Err(e) => {
                eprintln!("dextui: {e}");
                std::process::exit(1);
            }
        },
        // No terminal to hand over here, unlike `,` inside the app, so the
        // editor can simply be run.
        Command::EditConfig { scope } => {
            let path = match config::path_for_editing(scope) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("dextui: {e}");
                    std::process::exit(1);
                }
            };
            let current = std::fs::read_to_string(&path).unwrap_or_default();

            match editor::edit("config", &current) {
                Ok(Some(text)) => {
                    if let Err(e) = std::fs::write(&path, format!("{text}\n")) {
                        eprintln!("dextui: {}: {e}", path.display());
                        std::process::exit(1);
                    }
                    // Parse it back so a mistake is reported now rather than at
                    // the next launch, where it would be easy to miss.
                    let (_, problem) = config::load();
                    match problem {
                        Some(p) => {
                            // Saved, so the edit is not lost -- but a non-zero
                            // exit so this is not mistaken for success.
                            eprintln!("dextui: saved {}, but: {p}", path.display());
                            std::process::exit(1);
                        }
                        None => println!("saved {}", path.display()),
                    }
                }
                Ok(None) => println!("{} unchanged", path.display()),
                Err(e) => {
                    eprintln!("dextui: {e}");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
        Command::Icons => {
            println!("Set with icons = \"...\" in the config, or DEXTUI_ICONS\n");
            for i in icons::ALL {
                println!(
                    "  {:<9} {}  {}{}{}{}",
                    icons::name(i.tier),
                    icons::about(i.tier),
                    i.pending,
                    i.active,
                    i.done,
                    i.blocked,
                );
            }
            return Ok(());
        }
        Command::Run | Command::SelfTest => {}
    }

    // Ahead of the dex preflight, and ahead of `ratatui::init` which panics
    // rather than erroring when there is no terminal to put into raw mode.
    // Nothing below can succeed without one, so spending a ~180ms dex call to
    // report a different problem first would only send someone the wrong way.
    //
    // `selftest` is exempt: printing the data path without a terminal is the
    // entire reason it exists.
    if matches!(command, Command::Run)
        && !std::io::IsTerminal::is_terminal(&std::io::stdout())
    {
        eprintln!("{}", requires_a_terminal());
        std::process::exit(1);
    }

    let dex = Arc::new(Dex::real());

    // Preflight before taking over the terminal, so a failure prints plainly
    // instead of leaving a half-initialised TUI behind.
    let store_dir = match dex.store_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}", dex::requires_dex(&e));
            std::process::exit(1);
        }
    };

    // Same treatment: `dex dir` can succeed against a dex that then fails on a
    // real command, and reaching here having already said "dex is fine" would
    // make the second failure baffling.
    let tasks = match dex.list() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}", dex::requires_dex(&e));
            std::process::exit(1);
        }
    };

    let (cfg, config_problem) = config::load();

    let mut app = App::new(tasks, store_label(&store_dir), cfg);
    if let Some(msg) = config_problem {
        app.status = format!("config: {msg}");
    }

    if matches!(command, Command::SelfTest) {
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

    let mut glyphs = cfg.icons;
    let mut terminal = ratatui::init();
    // ratatui::init only sets raw mode and the alternate screen; mouse
    // reporting is opt-in. While captured, the terminal stops doing its own
    // text selection -- hold Shift to bypass it, as most terminals allow.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);

    // Polled rather than run from a reader thread. A thread blocked in
    // `event::read()` would swallow the first keystroke intended for $EDITOR,
    // because both it and the child would be reading the same terminal. The
    // pulse deliberately does not change that: it is arithmetic on this thread,
    // not a timer thread, so nothing else can ever reach for the terminal.
    //
    // `Instant`, not `SystemTime`: monotonic, so an NTP step or a laptop waking
    // from sleep cannot jump the animation's phase.
    let epoch = std::time::Instant::now();
    let mut dirty = true;
    while !app.should_quit {
        // The only redraw animation ever causes, and only while something is
        // actually in progress.
        if app.pulse_tick(epoch.elapsed(), glyphs.spin.len()) {
            dirty = true;
        }

        if dirty {
            terminal.draw(|f| ui::draw(f, &mut app, &glyphs))?;
            dirty = false;
        }

        // The timeout bounds how long a store change waits to be noticed; it is
        // not a redraw interval, since nothing is drawn unless something changed.
        // While a task is running it is additionally clamped to the next phase
        // flip, which can only ever shorten it -- when nothing is running this
        // is the same 100ms it has always been, and the idle cost stays zero.
        if event::poll(pulse::poll_timeout(app.is_animating(), epoch.elapsed()))? {
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

        if std::mem::take(&mut app.pending_config_edit) {
            edit_config(&mut terminal, &mut app, &mut glyphs)?;
            dirty = true;
        }
    }

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    Ok(())
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    match m.kind {
        // The header row, before anything else: it is above the body, so the
        // divider and pane tests below never see it.
        MouseEventKind::Down(MouseButton::Right) if m.row == 0 => {
            app.click_header(m.column, true);
        }

        MouseEventKind::Down(MouseButton::Left) if m.row == 0 => {
            app.click_header(m.column, false);
        }

        MouseEventKind::Down(MouseButton::Left) => {
            if app.on_divider(m.column) {
                app.dragging_split = true;
            } else if app.in_body(m.row) {
                // Click to focus, and in the tree also to select the row.
                match app.pane_at(m.column) {
                    Focus::Tree => {
                        app.focus = Focus::Tree;
                        app.select_at_row(m.row);
                    }
                    Focus::Detail => app.focus = Focus::Detail,
                }
            }
        }

        MouseEventKind::Drag(MouseButton::Left) if app.dragging_split => {
            app.set_split(m.column, app.terminal_width);
        }

        MouseEventKind::Up(_) => app.dragging_split = false,

        // The wheel acts on whichever pane is under the pointer, which is what
        // people expect regardless of where focus happens to be. Both panes slide
        // their *content* with the gesture -- see `App::scroll_tree` for why the
        // tree cannot just move its selection.
        MouseEventKind::ScrollDown => {
            match app.pane_at(m.column) {
                Focus::Tree => app.scroll_tree(1),
                Focus::Detail => app.scroll_detail(1, 0),
            }
        }
        MouseEventKind::ScrollUp => {
            match app.pane_at(m.column) {
                Focus::Tree => app.scroll_tree(-1),
                Focus::Detail => app.scroll_detail(-1, 0),
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

/// Opens the config file in $EDITOR and reloads it, creating it from the
/// template first if it does not exist yet.
fn edit_config(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    glyphs: &mut icons::Icons,
) -> std::io::Result<()> {
    let path = match config::path_for_editing(config::Scope::Global) {
        Ok(p) => p,
        Err(e) => {
            app.mode = Mode::Error(e);
            return Ok(());
        }
    };

    let current = std::fs::read_to_string(&path).unwrap_or_default();

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    let outcome = editor::edit("config", &current);
    *terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    terminal.clear()?;

    match outcome {
        Ok(Some(text)) => {
            if let Err(e) = std::fs::write(&path, format!("{text}\n")) {
                app.mode = Mode::Error(format!("{}: {e}", path.display()));
                return Ok(());
            }
            let (cfg, problem) = config::load();
            *glyphs = cfg.icons;
            app.apply_config(cfg);
            app.status = match problem {
                Some(p) => format!("config reloaded, but: {p}"),
                None => "config reloaded".into(),
            };
        }
        Ok(None) => app.status = "config unchanged".into(),
        Err(e) => app.mode = Mode::Error(flatten(&e.to_string())),
    }

    Ok(())
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
        // Enter always opens the detail. It is the deliberate way across, and it
        // was unbound in Normal mode, so nothing changes meaning.
        KeyCode::Enter => app.show_detail(),

        // Straight to a pane by number, the way LazyGit and gitui do it. The
        // numbers are only *drawn* in zoom mode, where there is a hidden pane to
        // reach, but the keys work at any width -- they were unbound, and
        // refusing them when both panes are visible would be a rule to remember
        // for no benefit.
        KeyCode::Char('1') => app.show_tree(),
        KeyCode::Char('2') => app.show_detail(),

        KeyCode::Right | KeyCode::Char('l') => match app.focus {
            // Falls through to the detail only when there was nothing to open --
            // a leaf, where this key did nothing at all before. Gated on
            // single-pane: with both panes visible, a focus jump while walking
            // the tree would silently redirect j/k to the other pane.
            Focus::Tree => {
                if !app.expand_selected() && app.single_pane() {
                    app.show_detail();
                }
            }
            Focus::Detail => app.scroll_detail(0, 4),
        },
        KeyCode::Left | KeyCode::Char('h') => match app.focus {
            Focus::Tree => app.collapse_selected(),
            // Sideways first while there is anywhere to go -- a wide table with
            // wrap off -- and back only when there is not. The same
            // "nothing to scroll to" rule the wrap toggle already follows.
            Focus::Detail => {
                let can_scroll = !app.wrap && app.detail_scroll.1 > 0;
                if !can_scroll && app.single_pane() {
                    app.show_tree();
                } else {
                    app.scroll_detail(0, -4);
                }
            }
        },
        KeyCode::Char('g') => match app.focus {
            Focus::Tree => app.select_first(),
            Focus::Detail => app.detail_to_top(),
        },
        KeyCode::Char('G') => match app.focus {
            Focus::Tree => app.select_last(),
            Focus::Detail => app.detail_to_bottom(),
        },
        // `z` is tmux's zoom-pane, which is where the reflex comes from. That
        // cost collapse/expand their old keys -- and `-`/`+` is the better
        // mnemonic anyway: minus closes, plus opens. `=` is accepted for `+`
        // because it is the same physical key without the shift.
        KeyCode::Char('z') => app.toggle_zoom(),
        KeyCode::Char('-') => app.collapse_all(),
        KeyCode::Char('+') | KeyCode::Char('=') => app.expand_all(),

        KeyCode::Char('/') => app.mode = Mode::Search,
        KeyCode::Char('f') => {
            app.filter = app.filter.next();
            app.rebuild();
        }
        // Ctrl-R, because `r` is worth more as rename. The store is watched and
        // a 10s safety poll backstops it, so this is an escape hatch for the
        // events macOS drops rather than something you should need.
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            refresh(dex, tx)
        }
        KeyCode::Char('?') => app.mode = Mode::Help,
        // Reuses the $EDITOR machinery rather than building a settings form.
        KeyCode::Char(',') => app.pending_config_edit = true,

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
        // `r` for rename and `e` for edit, rather than `e`/`E` for both. The
        // app's other case pairs are one action and its variant -- `o`/`O` sorts
        // and reverses, `z`/`Z` collapses and expands -- whereas these were two
        // different editors sharing a letter, and which case did which was
        // something you had to remember rather than work out.
        KeyCode::Char('r') => {
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
        KeyCode::Char('e') => {
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

#[cfg(test)]
mod tests {
    /// The message has to leave someone with something to do. `selftest` is the
    /// answer for the case that produces this -- a script or CI job wanting the
    /// data without a terminal -- so it must be named, and must stay a real
    /// command: `every_command_in_the_usage_text_is_actually_accepted` covers
    /// the usage text, not this string.
    #[test]
    fn the_no_terminal_message_offers_a_way_forward() {
        let m = super::requires_a_terminal();
        assert!(m.contains("real terminal"), "does not say what is wrong: {m}");
        assert!(m.contains("dextui selftest"), "offers no alternative: {m}");
        assert!(
            matches!(super::parse(&["selftest".to_string()]), Ok(super::Command::SelfTest)),
            "the message names a command that is not accepted"
        );
    }

    use super::*;

    fn parsed(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn no_arguments_runs_the_tui() {
        assert!(matches!(parsed(&[]), Ok(Command::Run)));
    }

    #[test]
    fn an_unknown_command_or_option_is_an_error_not_a_silent_launch() {
        // Unknown arguments used to fall through into starting the TUI, so
        // `--help` did nothing in a terminal and panicked outside one.
        assert!(parsed(&["--nonsense"]).unwrap_err().contains("--nonsense"));
        assert!(parsed(&["wibble"]).unwrap_err().contains("wibble"));
        assert!(parsed(&["config", "wibble"]).unwrap_err().contains("wibble"));
    }

    #[test]
    fn help_and_version_have_short_forms() {
        assert!(matches!(parsed(&["-h"]), Ok(Command::Help)));
        assert!(matches!(parsed(&["--help"]), Ok(Command::Help)));
        assert!(matches!(parsed(&["-V"]), Ok(Command::Version)));
        assert!(matches!(parsed(&["--version"]), Ok(Command::Version)));
    }

    #[test]
    fn help_wins_even_after_a_command() {
        // Asking for help should never run something instead.
        assert!(matches!(parsed(&["config", "init", "--help"]), Ok(Command::Help)));
    }

    #[test]
    fn config_subcommands_parse() {
        assert!(matches!(parsed(&["config"]), Ok(Command::ShowConfig)));
        assert!(matches!(parsed(&["config", "init"]), Ok(Command::InitConfig { .. })));
        assert!(matches!(parsed(&["config", "edit"]), Ok(Command::EditConfig { .. })));
    }

    #[test]
    fn local_and_project_select_the_project_file() {
        // -l matches dex's own spelling; --project says what it means.
        for flag in ["-l", "--local", "--project"] {
            assert!(
                matches!(
                    parsed(&["config", "edit", flag]),
                    Ok(Command::EditConfig { scope: config::Scope::Project })
                ),
                "{flag} did not select the project scope"
            );
        }
    }

    #[test]
    fn global_is_the_default_and_can_be_stated_explicitly() {
        assert!(matches!(
            parsed(&["config", "edit"]),
            Ok(Command::EditConfig { scope: config::Scope::Global })
        ));
        for flag in ["-g", "--global"] {
            assert!(matches!(
                parsed(&["config", "edit", flag]),
                Ok(Command::EditConfig { scope: config::Scope::Global })
            ));
        }
    }

    #[test]
    fn options_may_appear_before_the_command() {
        assert!(matches!(
            parsed(&["--local", "--force", "config", "init"]),
            Ok(Command::InitConfig { force: true, scope: config::Scope::Project })
        ));
    }

    #[test]
    fn init_does_not_overwrite_unless_asked() {
        assert!(matches!(
            parsed(&["config", "init"]),
            Ok(Command::InitConfig { force: false, .. })
        ));
    }

    #[test]
    fn every_command_in_the_usage_text_is_actually_accepted() {
        // The usage block is the only place these are advertised, so anything
        // listed but unparseable would be a silent lie.
        for line in USAGE.lines() {
            let line = line.trim_end();
            let Some(rest) = line.strip_prefix("    ") else {
                continue;
            };
            if rest.starts_with(' ') || rest.is_empty() {
                continue;
            }
            // "config init         Write a config template" -> ["config", "init"]
            let words: Vec<&str> = rest
                .split_whitespace()
                .take_while(|w| !w.starts_with('-') && w.chars().all(|c| c.is_ascii_lowercase()))
                .collect();
            if words.is_empty() || words[0] == "dextui" {
                continue;
            }
            assert!(
                parse(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>()).is_ok(),
                "usage advertises {words:?} but the parser rejects it"
            );
        }
    }
}
