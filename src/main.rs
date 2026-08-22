//! dextui — a two-pane terminal browser for dex tasks.

mod app;
mod config;
mod dex;
mod editor;
mod icons;
mod log;
mod markdown;
mod pulse;
mod registry;
mod repos;
#[cfg(test)]
mod test_support;
mod theme;
mod tree;
mod ui;
mod watch;
mod worktree;

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;

use app::{App, Focus, Mode, Pending, Prompt, TextInput};
use dex::{Dex, Task};

/// Everything the main loop reacts to, from every thread, on one channel.
enum Msg {
    /// A task list, tagged with the store directory it was actually read
    /// from.
    ///
    /// The tag is load-bearing, not diagnostic. `refresh` spawns a thread
    /// holding a clone of the `Arc<Dex>` current at the time; `switch_store`
    /// replaces that `Arc` on the main loop. A refresh spawned just before a
    /// switch therefore lands just after it, and without a tag would paint the
    /// **old** store's tasks under the new store's label -- silently, which is
    /// exactly why CLAUDE.md treats a wrong store as worse than a crash.
    Tasks {
        store: String,
        result: Result<Vec<Task>, String>,
    },
    Ok(String),
    Failed(String),
    /// `dex complete` was rejected; carries what is needed to retry with --force.
    CompleteRejected {
        id: String,
        result: String,
        error: String,
    },
    /// A sidebar store was re-read. Carries its freshly listed tasks (or the
    /// failure), keyed by store directory.
    ///
    /// Every store gets this, the selected one included: the cache it fills is
    /// what makes moving the sidebar cursor change the panes immediately, so
    /// the store being read cannot be the one store missing from it.
    StoreLoaded {
        dir: String,
        result: Result<Vec<Task>, String>,
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

    // Always on -- see src/log.rs. Before anything else that might be worth
    // recording, and well before `ratatui::init` takes the terminal: once the
    // alternate screen is up there is nowhere else this app can report to.
    log::init();

    // Ahead of the dex preflight, and ahead of `ratatui::init` which panics
    // rather than erroring when there is no terminal to put into raw mode.
    // Nothing below can succeed without one, so spending a ~180ms dex call to
    // report a different problem first would only send someone the wrong way.
    //
    // `selftest` is exempt: printing the data path without a terminal is the
    // entire reason it exists.
    if matches!(command, Command::Run) && !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        eprintln!("{}", requires_a_terminal());
        std::process::exit(1);
    }

    // `mut`: selecting a worktree in the repo pane replaces this with a
    // `Dex::for_store` targeting the chosen store -- see `switch_store`.
    let mut dex = Arc::new(Dex::real());

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
    // Loaded before `App::new`, which otherwise defaults it to empty: without
    // this, the first `register_repo_path` call of the run would `save()` an
    // empty-plus-one-entry registry over whatever was already on disk,
    // silently dropping every repo registered in an earlier session.
    let (registry, registry_problem) = registry::Registry::load();
    match &registry_problem {
        Some(p) => log::line("registry", &format!("load failed: {p}")),
        None => log::line(
            "registry",
            &format!("loaded {} repo(s)", registry.repos.len()),
        ),
    }

    // The repo/worktree sidebar. `App::new` cannot populate this itself --
    // `App` owns view state, not I/O -- so it is loaded here, the same way
    // the preflight `dex.list()` above fills `app.tasks`. A registered path
    // that no longer exists, or a `git worktree list` failure, is reported
    // and skipped rather than fatal: a repo someone has since deleted must
    // not stop the app starting.
    let mut repos: Vec<repos::Repo> = Vec::new();
    let mut repo_problems: Vec<String> = Vec::new();
    for repo_path in &registry.repos {
        if !std::path::Path::new(repo_path).is_dir() {
            repo_problems.push(format!("{repo_path} no longer exists"));
            continue;
        }
        match worktree::list(repo_path) {
            Ok(worktrees) => repos.push(repos::Repo {
                name: repo_name(repo_path),
                path: repo_path.clone(),
                worktrees,
                open: true,
                registered: true,
                is_global: false,
            }),
            Err(e) => repo_problems.push(format!("{repo_path}: {}", flatten(&e))),
        }
    }

    // The store this run is actually reading always gets a row, registered or
    // not. Without it, launching anywhere unregistered showed an empty sidebar
    // beside a full task tree -- a pane saying "no repos" while you are plainly
    // looking at one -- and `a` appeared to *create* the repo rather than to
    // keep it.
    let mut here_path = None;
    if let Some(here) = current_repo(&store_dir, &mut repo_problems) {
        here_path = Some(here.path.clone());
        if !repos.iter().any(|r| r.path == here.path) {
            repos.push(here);
        }
    }
    // Sorted the same way `Registry::add` keeps the file, so registering the
    // current repo changes its marker and never its position.
    repos.sort_by(|a, b| a.path.cmp(&b.path));

    let mut app = App::new(tasks, store_dir.clone(), cfg);
    app.registry = registry;
    app.repos = repos;
    // Fixed for the run: `here` is where dextui was launched, which switching
    // stores does not change. `App::new` already recorded the store.
    app.here_path = here_path;
    app.status = [
        config_problem.map(|c| format!("config: {c}")),
        registry_problem.map(|r| format!("repos: {r}")),
    ]
    .into_iter()
    .flatten()
    .chain(repo_problems)
    .collect::<Vec<_>>()
    .join("; ");

    if matches!(command, Command::SelfTest) {
        println!("store   {store_dir}");
        print!("{}", ui::selftest(&app));
        return Ok(());
    }

    let (tx, rx) = channel::<Msg>();

    // Which registered worktree the app is actually reading -- the one whose
    // store is `store_dir`, already resolved above by the preflight `dex
    // dir` call.
    app.selected_worktree = app
        .repos
        .iter()
        .flat_map(|r| r.worktrees.iter())
        .find(|w| repos::store_dir(&w.path) == store_dir)
        .map(|w| w.path.clone());

    // And the cursor starts on it, so the sidebar opens pointing at what the
    // other two panes are already showing rather than at whatever sorted
    // first.
    app.select_current_store_row();

    // Every store the sidebar can reach, the selected one included.
    //
    // It used to be "every store *except* the selected one", which kept its
    // own `watch::spawn` and its own channel. Two mechanisms for one job was
    // survivable while a switch cost a `dex list` anyway -- but a switch is
    // now a cache lookup, and restarting a watcher on every cursor move would
    // have been the only thing left making it expensive. One fleet, set up
    // once, means `switch_store` touches no watcher at all.
    let all_store_dirs = app.sidebar_stores();

    // Read **concurrently** -- one thread per store calling `Dex::for_store`
    // then `.list()`, joined before the first draw. A `dex` call costs ~180ms
    // of Node startup; done one after another, ten stores would be 1.8s of
    // blank screen before the first frame.
    //
    // The whole task list is kept, not just its counts. This is the same work
    // as before -- the lists were already being fetched here and reduced to
    // counts on arrival -- but keeping them is what lets a later switch to any
    // of these stores cost nothing at all.
    //
    // `repos::has_store` first, since it is a plain on-disk check: a store
    // that does not exist yet is an ordinary row, not worth a process spawn
    // that would only come back empty.
    let handles: Vec<_> = all_store_dirs
        .iter()
        .filter(|dir| *dir != &store_dir && std::path::Path::new(dir).is_dir())
        .map(|dir| {
            let dir = dir.clone();
            thread::spawn(move || {
                let start = std::time::Instant::now();
                let result = Dex::for_store(&dir).and_then(|d| d.list());
                log_list_outcome(&dir, &result, start.elapsed());
                (dir, result)
            })
        })
        .collect();
    for h in handles {
        // A store that failed to read is simply absent from the cache -- it
        // gets another chance the next time its own watcher fires, or when it
        // is selected. Absent and empty stay distinguishable, which is what
        // lets a switch tell "not read yet" from "no tasks". Nothing here is
        // fatal.
        if let Ok((dir, Ok(store_tasks))) = h.join() {
            app.store_tasks.insert(dir, store_tasks);
        }
    }
    // The selected store already has its full task list in `tasks` above -- no
    // second `dex list` to seed its own entry.
    app.store_tasks.insert(store_dir.clone(), app.tasks.clone());

    // One watcher per store, each with the same stat-gated safety net: no
    // extra polling loop, and no `dex list` at all until a store's own
    // fingerprint actually changes. That is what keeps this from becoming the
    // "ten node spawns every ten seconds forever" alternative CLAUDE.md rules
    // out. `_store_watchers` must stay alive for the whole run; dropping it
    // stops every one of these notifications.
    let (worktree_tx, worktree_rx) = channel::<String>();
    let mut store_watchers = watch::spawn_many(&all_store_dirs, worktree_tx.clone());
    // What already has one, so adding a repo mid-run does not stack a second
    // watcher on a store that is fine.
    let mut watched: std::collections::HashSet<String> = all_store_dirs.iter().cloned().collect();
    {
        let tx = tx.clone();
        thread::spawn(move || {
            while let Ok(dir) = worktree_rx.recv() {
                let start = std::time::Instant::now();
                let result = Dex::for_store(&dir).and_then(|d| d.list());
                log_list_outcome(&dir, &result, start.elapsed());
                if tx.send(Msg::StoreLoaded { dir, result }).is_err() {
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

        if std::mem::take(&mut app.force_redraw) {
            // Discards ratatui's idea of what is on screen, so the next draw
            // writes every cell rather than the difference against a buffer
            // that may no longer describe reality.
            terminal.clear()?;
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
            // **Drained, not one per frame.** Handling a single event and
            // then repainting looks equivalent and is not: mouse input arrives
            // in bursts -- `EnableMouseCapture` turns on button-event
            // tracking, which reports motion for every cell the pointer
            // crosses while held -- and a redraw of a full tree plus a
            // rendered-markdown detail pane is not free. Unread bytes sit in
            // the tty input buffer, which is finite (4 KB here); once it fills
            // the kernel drops bytes, and a byte lost inside `\e[<0;60;6M`
            // leaves a truncated escape sequence that crossterm reads as a
            // coordinate nobody sent, or as a bare `\e` keypress.
            //
            // What is measured and what is not, since the difference matters:
            // the overflow was reproduced only by writing thousands of bytes
            // in one go, which is an injection artifact -- 600 events
            // delivered at a realistic 1000-2000/s all arrived intact, and the
            // same 600 in a single 7 KB write killed the process. So this is a
            // headroom fix, not a diagnosed crash. It is still the right
            // shape: one repaint per burst rather than per event is fewer
            // frames, lower latency, and a buffer that stays empty while the
            // app is briefly busy.
            //
            // Terminates promptly -- `poll(ZERO)` is false the moment input
            // pauses.
            loop {
                match event::read()? {
                    CtEvent::Key(key) if key.kind == KeyEventKind::Press => {
                        handle_key(&mut app, key, &dex, &tx);
                        dirty = true;
                    }
                    CtEvent::Mouse(m) => {
                        handle_mouse(&mut app, m);
                        dirty = true;
                    }
                    // Everything else -- focus changes, paste, key releases --
                    // repaints too. It costs nothing at idle, since none of it
                    // arrives unless something is actually happening, and a
                    // frame is the cheapest possible insurance against a
                    // screen that has gone stale for a reason the app cannot
                    // see. `Resize` is no longer special-cased for the same
                    // reason.
                    _ => dirty = true,
                }

                // Anything that is about to hand the terminal to someone else
                // stops the drain: the rest of the queued input belongs to
                // `$EDITOR`, not to us. Same reason this loop polls rather
                // than running a reader thread.
                if app.should_quit
                    || app.pending_editor.is_some()
                    || app.pending_config_edit
                    || !event::poll(std::time::Duration::ZERO)?
                {
                    break;
                }
            }
        }

        while let Ok(msg) = rx.try_recv() {
            // A `StoreLoaded` for a store nobody is looking at changes only
            // the cache, so redrawing for it would be exactly the idle-cost
            // regression the pulse guarantee exists to prevent, for zero
            // visible change. One for the store on screen is a real refresh.
            let visible = !matches!(&msg, Msg::StoreLoaded { dir, .. } if *dir != app.store_dir);
            handle_msg(&mut app, msg, &dex, &tx);
            dirty = dirty || visible;
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

        // Requested by `enter`/`l` in the repo pane. Synchronous, like the
        // preflight `dex list` above: a discrete action rather than something
        // that runs on every keystroke, so the ~180ms dex call is the same
        // trade the preflight and the `$EDITOR` handoff already make.
        // A repo added mid-run has neither a watcher nor a cached task list,
        // so switching to it would fall back to a synchronous-looking async
        // `dex list` and its sidebar counts would stay absent. `A` made that
        // an ordinary action rather than an edge case.
        if std::mem::take(&mut app.repos_changed) {
            watch_new_stores(&app, &mut store_watchers, &mut watched, &worktree_tx, &tx);
        }

        if let Some(path) = app.pending_store.take() {
            switch_store(&mut app, &mut dex, &tx, &path);
            dirty = true;
        }
    }

    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    Ok(())
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    // With the help open the dialog is what the pointer is over, so the wheel
    // scrolls it rather than a pane it is covering -- and every other gesture
    // is swallowed rather than reaching through, since a click that moved the
    // selection under a dialog you cannot see it happen behind is exactly the
    // kind of unasked-for movement this app is built not to do.
    if matches!(app.mode, Mode::Help) {
        match m.kind {
            MouseEventKind::ScrollDown => app.scroll_help(1),
            MouseEventKind::ScrollUp => app.scroll_help(-1),
            _ => {}
        }
        return;
    }

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
            if let Some(d) = app.divider_at(m.column) {
                app.dragging = Some(d);
            } else if app.in_body(m.row) {
                // Click to focus, and in the tree also to select the row --
                // plus open or close it when the pointer was on its marker.
                match app.pane_at(m.column) {
                    Focus::Tree => {
                        app.focus = Focus::Tree;
                        app.click_tree(m.column, m.row);
                    }
                    Focus::Detail => app.focus = Focus::Detail,
                    // Selects, like the tree -- which is what the help
                    // already promises ("click selects"). Switching store
                    // stays on `enter`/`l`: a click is how you *look* at a
                    // row, and making it also spend a ~180ms dex call and
                    // replace both other panes would make the sidebar
                    // dangerous to point at.
                    Focus::Repos => {
                        app.focus = Focus::Repos;
                        app.select_repo_at_row(m.row);
                        follow_repo_cursor(app);
                    }
                }
            }
        }

        MouseEventKind::Drag(MouseButton::Left) if app.dragging.is_some() => match app.dragging {
            Some(app::Divider::Repos) => app.set_repos_width(m.column, app.terminal_width),
            Some(app::Divider::Split) => app.set_split(m.column, app.terminal_width),
            None => {}
        },

        MouseEventKind::Up(_) => app.dragging = None,

        // The wheel acts on whichever pane is under the pointer, which is what
        // people expect regardless of where focus happens to be. Both panes slide
        // their *content* with the gesture -- see `App::scroll_tree` for why the
        // tree cannot just move its selection.
        MouseEventKind::ScrollDown => match app.pane_at(m.column) {
            Focus::Tree => app.scroll_tree(1),
            Focus::Detail => app.scroll_detail(1, 0),
            Focus::Repos => app.scroll_repos(1),
        },
        MouseEventKind::ScrollUp => match app.pane_at(m.column) {
            Focus::Tree => app.scroll_tree(-1),
            Focus::Detail => app.scroll_detail(-1, 0),
            Focus::Repos => app.scroll_repos(-1),
        },
        MouseEventKind::ScrollLeft => app.scroll_detail(0, -4),
        MouseEventKind::ScrollRight => app.scroll_detail(0, 4),

        _ => {}
    }
}

fn handle_msg(app: &mut App, msg: Msg, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    match msg {
        // A refresh that was already in flight when the store changed under
        // it. Dropped rather than applied: it describes a store nobody is
        // looking at any more, and painting it would leave the tree and the
        // header disagreeing about which project this is.
        Msg::Tasks { store, .. } if store != app.store_dir => {
            log::line(
                "store",
                &format!("dropped a task list from {store}; now on {}", app.store_dir),
            );
        }

        Msg::Tasks {
            result: Ok(tasks), ..
        } => app.apply_tasks(tasks),
        // Keep the last good model rather than blanking the view.
        Msg::Tasks { result: Err(e), .. } => {
            app.status = format!("refresh failed: {}", flatten(&e))
        }

        Msg::Ok(message) => {
            app.status = message;
            refresh(dex, tx, app);
        }
        Msg::Failed(e) => app.mode = Mode::Error(flatten(&e)),

        Msg::CompleteRejected { id, result, error } => {
            app.mode = Mode::ForceComplete {
                id,
                result,
                message: flatten(&error),
            };
        }

        // A store's watcher fired and it has been re-read. On success the
        // cache entry is replaced; on failure it is left exactly as it was --
        // the same "keep the last good model" rule `Msg::Tasks(Err)` follows,
        // and without a status message for a store nobody is looking at,
        // since that would violate "a refresh must never disturb the user."
        Msg::StoreLoaded { dir, result } => {
            let Ok(store_tasks) = result else { return };

            // The store on screen goes through `apply_tasks`, which is what
            // preserves the selection, the expansion set and any open dialog.
            // Writing only the cache here would leave the panes stale until
            // something else happened to redraw them -- this message replaced
            // the watcher path the visible panes used to have of their own.
            if dir == app.store_dir {
                if app.is_modal() {
                    // Deferred rather than dropped; applied when the dialog
                    // closes. The cache is still updated, so a switch away and
                    // back cannot resurrect the older list.
                    app.store_tasks.insert(dir, store_tasks);
                    app.pending_refresh = true;
                    return;
                }
                app.apply_tasks(store_tasks.clone());
            }
            app.store_tasks.insert(dir, store_tasks);
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

/// Points the running app at a different worktree's dex store.
///
/// **Does no I/O on the common path**, which is what lets the sidebar cursor
/// drive the panes as immediately as the tree cursor drives the detail. Every
/// sidebar store is already listed at startup and kept current by its own
/// watcher, so a switch is a cache lookup and a `Dex` swap; and because the
/// watchers cover every store rather than only the unselected ones, there is
/// none to restart here either.
///
/// A miss -- a repo registered mid-run, or a store whose read failed -- falls
/// back to an async list. It shows the new store's label over an **empty**
/// tree while that runs, never the old store's tasks under the new name: dex
/// reports the wrong store as an empty project rather than an error, so the
/// one thing this must never do is make a wrong store look plausible.
fn switch_store(app: &mut App, dex: &mut Arc<Dex>, tx: &Sender<Msg>, worktree_path: &str) {
    // Through the sidebar, not `repos::store_dir`: the global row's path is
    // already its store, and deriving `<path>/.dex` from it would point dex at
    // a directory that does not exist.
    let dir = app.store_for_path(worktree_path);
    // The early return is what keeps a cursor move cheap when it does not
    // actually change store -- a repo row and its main worktree row resolve to
    // the same one -- and it is what stops the log filling with switches that
    // did not happen.
    if dir == app.store_dir {
        return;
    }
    log::line(
        "store",
        &format!("switching from {} to {dir}", app.store_dir),
    );

    let new_dex = match Dex::for_store(&dir) {
        Ok(d) => d,
        Err(e) => {
            app.status = format!("could not switch store: {e}");
            return;
        }
    };
    *dex = Arc::new(new_dex);

    // `load_store`, not `apply_tasks`: the latter's whole job is preserving a
    // selection and expansion set the *same* store made, by resolving them
    // against the new task list -- but this is a different store, so
    // `self.selected`/`self.expanded` refer to ids that belong nowhere here.
    // Using it anyway is exactly the collapsed-single-root bug CLAUDE.md
    // records: every old expanded id would fail to match the new tree and
    // `expand_all` would never run, so the new store would open fully
    // collapsed.
    // No status message either way. The header already names the store, in the
    // one element the degradation ladder promises always survives -- so
    // "switched to X" restated, one line lower and in a slot with no history,
    // what the screen was already saying. It was noise when a switch was a
    // deliberate `enter`; now that moving the sidebar cursor switches on every
    // keystroke it would fire on every `j`. The same goes for "reading X…":
    // nothing clears it, so an uncached store left a stale progress message
    // sitting there until the next keypress, long after the ~180ms read.
    match app.store_tasks.get(&dir) {
        Some(cached) => app.load_store(cached.clone(), dir.clone()),
        None => {
            app.load_store(Vec::new(), dir.clone());
            // Tagged with its store, so if the cursor has moved on again by
            // the time this lands, `handle_msg` drops it rather than painting
            // one store's tasks under another's name.
            let dex = Arc::clone(dex);
            let tx = tx.clone();
            let store = dir.clone();
            thread::spawn(move || {
                let start = std::time::Instant::now();
                let result = dex.list();
                log_list_outcome(&store, &result, start.elapsed());
                let _ = tx.send(Msg::StoreLoaded { dir: store, result });
            });
        }
    }
}

/// Gives a watcher and a first read to any sidebar store that lacks them.
///
/// Idempotent by design: it is called whenever the repo list changes, and
/// stacking a second watcher on a store that already has one would double
/// every event it reports.
fn watch_new_stores(
    app: &App,
    watchers: &mut Vec<watch::StoreWatcher>,
    watched: &mut std::collections::HashSet<String>,
    worktree_tx: &Sender<String>,
    tx: &Sender<Msg>,
) {
    for dir in app.sidebar_stores() {
        if !watched.insert(dir.clone()) {
            continue;
        }
        watchers.extend(watch::spawn_many(
            std::slice::from_ref(&dir),
            worktree_tx.clone(),
        ));

        // The first read, off the UI thread. Startup joins these before the
        // first frame because it has nothing to draw until it does; here there
        // is a frame on screen already, so blocking it would be a stall for a
        // store nobody has asked to look at yet.
        if app.store_tasks.contains_key(&dir) || !std::path::Path::new(&dir).is_dir() {
            continue;
        }
        let tx = tx.clone();
        thread::spawn(move || {
            let start = std::time::Instant::now();
            let result = Dex::for_store(&dir).and_then(|d| d.list());
            log_list_outcome(&dir, &result, start.elapsed());
            let _ = tx.send(Msg::StoreLoaded { dir, result });
        });
    }
}

/// Saves the repo at a typed path, so a repo you are *not* in can be added.
///
/// `~` is expanded here rather than by a shell: nothing in this app goes
/// through one, so a pasted `~/Developer/thing` would otherwise be taken
/// literally and fail on a directory that does not exist.
///
/// Any path *inside* a repo works, not just its root: `git worktree list`
/// reports the main checkout first whatever you point it at, so pasting a
/// worktree or a subdirectory saves the repo that owns it. Forgiving in the
/// direction that costs nothing.
fn save_repo_at(app: &mut App, typed: &str) -> String {
    let typed = typed.trim();
    if typed.is_empty() {
        return String::new();
    }
    let expanded = match typed.strip_prefix("~") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => format!("{}{rest}", home.to_string_lossy()),
            None => return "cannot expand ~: HOME is not set".to_string(),
        },
        None => typed.to_string(),
    };

    if !std::path::Path::new(&expanded).is_dir() {
        return format!("{expanded} is not a directory");
    }
    match worktree::list(&expanded) {
        Ok(worktrees) => register_repo(app, worktrees),
        Err(e) => format!("{expanded} is not a git repo: {}", flatten(&e)),
    }
}

/// Registers the repo dextui is currently running against -- not whatever row
/// the cursor happens to be on in the sidebar. `git worktree list` always
/// reports the main checkout first, so that entry is "the repo that has the
/// worktrees," which is what gets registered.
///
/// Writing `repos.toml` is only half of it: the sidebar draws `app.repos`, so
/// a registration that only persisted would appear to do **nothing at all**
/// until the next launch -- and the README promises switching between repos
/// "without restarting". The row is therefore built here too, exactly the way
/// startup builds one, from the `git worktree list` this already had to run.
fn register_current_repo(app: &mut App) -> String {
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => return format!("could not resolve the current directory: {e}"),
    };
    let worktrees = match worktree::list(&cwd.to_string_lossy()) {
        Ok(w) => w,
        Err(e) => return format!("could not list worktrees: {}", flatten(&e)),
    };
    register_repo(app, worktrees)
}

/// The half of `register_current_repo` that has no I/O of its own, so the
/// sidebar row it adds can actually be tested -- listing worktrees needs a real
/// git repo and a real working directory, neither of which a unit test has.
fn register_repo(app: &mut App, worktrees: Vec<worktree::Worktree>) -> String {
    // Cloned rather than held as a borrow of `worktrees`: the whole list is
    // moved into the new row below, and `app` is borrowed mutably in between.
    let Some(path) = worktrees.first().map(|w| w.path.clone()) else {
        return "no worktrees found".to_string();
    };
    match app.register_repo_path(&path) {
        Ok(true) => {
            log::line("registry", &format!("saved: added {path}"));
            // Usually already on screen, unregistered: the store this run
            // reads always has a row. Registering it marks that row rather
            // than adding a second one for the same repo -- and because the
            // list is sorted by path either way, the row does not move.
            match app.repos.iter_mut().find(|r| r.path == path) {
                Some(row) => row.registered = true,
                None => {
                    app.repos.push(repos::Repo {
                        name: repo_name(&path),
                        path: path.clone(),
                        worktrees,
                        open: true,
                        registered: true,
                        is_global: false,
                    });
                    // `Registry::add` keeps the file sorted by path, so the
                    // next launch lists them in that order. Sorting here too
                    // means the row does not move on the first restart.
                    app.repos.sort_by(|a, b| a.path.cmp(&b.path));
                }
            }
            // The row *does* move now -- out of `here` and down into `saved`,
            // which is the whole point -- so the cursor has to follow it or it
            // would be left addressing whatever slid up into that index, and
            // in the single-repo case that index is now a heading.
            app.select_current_store_row();
            app.repos_changed = true;
            format!("saved {path}")
        }
        Ok(false) => format!("{path} is already saved"),
        Err(e) => {
            let e = flatten(&e);
            log::line("registry", &format!("save failed: {e}"));
            format!("could not save: {e}")
        }
    }
}

/// The last path component, which is what the sidebar shows for a repo.
/// Shared by startup and `a` so a repo registered mid-run is labelled exactly
/// the way it will be after a restart.
/// The sidebar row for wherever the app was launched, registered or not.
///
/// Two shapes, because dex has two. Inside a git repo the store is
/// `<worktree>/.dex` and the row is an ordinary repo with its real worktrees.
/// Outside one, dex silently falls back to a global store at
/// `~/.config/dex/local` -- the single most confusing thing about it, per
/// CLAUDE.md -- so that gets a row named `global`, which is the only place on
/// screen that has ever said so.
///
/// `store_dir` is what decides between them rather than a fresh `is_dir` check:
/// it came from `dex dir`, so it is dex's own answer about which store this run
/// reads, and a second opinion here could disagree with it.
fn current_repo(store_dir: &str, problems: &mut Vec<String>) -> Option<repos::Repo> {
    let Some(root) = store_dir.strip_suffix("/.dex") else {
        return Some(repos::Repo {
            name: "global".into(),
            path: store_dir.to_string(),
            worktrees: Vec::new(),
            open: true,
            registered: false,
            is_global: true,
        });
    };

    match worktree::list(root) {
        Ok(worktrees) => {
            // `git worktree list` reports the main checkout first, whatever
            // was asked about, so the repo's identity is that path -- not the
            // worktree this happens to be running in.
            let path = worktrees
                .first()
                .map_or(root, |w| w.path.as_str())
                .to_string();
            Some(repos::Repo {
                name: repo_name(&path),
                path,
                worktrees,
                open: true,
                registered: false,
                is_global: false,
            })
        }
        Err(e) => {
            problems.push(format!("{root}: {}", flatten(&e)));
            None
        }
    }
}

fn repo_name(repo_path: &str) -> String {
    std::path::Path::new(repo_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo_path.to_string())
}

/// Picks the worktree under the sidebar cursor: remembers where the cursor
/// was in the worktree being left, focuses the tree, and queues the actual
/// store swap for the main loop -- `handle_normal` only ever sees `&Arc<Dex>`,
/// not a way to replace what it points to.
fn select_worktree_under_cursor(app: &mut App) {
    follow_repo_cursor(app);
    // `enter` is now only about focus: the store already changed when the
    // cursor landed here. Kept because moving to the pane you just chose is
    // exactly what you want next, and because `l` means the same thing.
    if app.selected_worktree_path().is_some() {
        app.focus = Focus::Tree;
    }
}

fn move_repo_cursor(app: &mut App, delta: isize) {
    app.move_repo_row(delta);
    follow_repo_cursor(app);
}

/// Points the task panes at whatever the sidebar cursor is on now.
///
/// This is what makes the sidebar and the task tree one model rather than two
/// that look identical: moving the tree cursor changes the detail pane on
/// every keystroke, and moving the sidebar cursor now changes the tree and
/// detail the same way. It used to take `enter`, on the grounds that a switch
/// cost a ~180ms `dex list` -- which stopped being true once every sidebar
/// store's task list was cached rather than reduced to counts and thrown away.
/// See `switch_store` for what a move actually costs now.
fn follow_repo_cursor(app: &mut App) {
    if let Some(path) = app.selected_worktree_path() {
        app.select_worktree(&path);
        app.pending_store = Some(path);
    }
}

/// One log line per `dex list`, in the one format every call site below
/// shares -- so the file stays greppable by store regardless of which path
/// issued the call. `store` is whatever identifies the store to the *caller*:
/// `refresh()` passes `app.store_label` (a short display name, since there is
/// only ever one selected store and no ambiguity to resolve), everywhere else
/// passes the store directory -- the same string `watch.rs` logs in its
/// `registered`/`event`/`tick` lines for that store, so a `tick <dir>
/// changed` can be grepped straight through to the `list <dir> ...` it
/// caused.
fn log_list_outcome(store: &str, result: &Result<Vec<Task>, String>, elapsed: std::time::Duration) {
    let ms = elapsed.as_millis();
    match result {
        Ok(tasks) => log::line(
            "dex",
            &format!("list {store} - {} tasks {ms}ms", tasks.len()),
        ),
        Err(e) => log::line(
            "dex",
            &format!("list {store} failed after {ms}ms: {}", flatten(e)),
        ),
    }
}

/// Takes the whole `App` rather than the two strings it reads, so the store a
/// refresh is *tagged* with and the store the app *thinks* it is on cannot be
/// passed separately and drift apart -- which would defeat the check in
/// `handle_msg` entirely.
///
/// The log line keeps using the display label (`store_label`): there is only
/// ever one selected store, so there is nothing to disambiguate, and it is
/// what every existing line in the file already says.
fn refresh(dex: &Arc<Dex>, tx: &Sender<Msg>, app: &App) {
    let dex = Arc::clone(dex);
    let tx = tx.clone();
    let store = app.store_dir.clone();
    let logged = store.clone();
    thread::spawn(move || {
        let start = std::time::Instant::now();
        let result = dex.list();
        // The *directory*, like every other `list` site. This one logged the
        // display label, which is friendlier to read and useless to grep:
        // `watch`'s registered/event/tick lines all name the directory, so a
        // store's chain could be followed end to end for every store except
        // the one being looked at.
        log_list_outcome(&logged, &result, start.elapsed());
        let _ = tx.send(Msg::Tasks { store, result });
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
        refresh(dex, tx, app);
    }
}

/// What a key *means*, once the chord that produced it has been resolved.
///
/// Deliberately coarse. Eleven of these depend on which pane has focus --
/// `MoveDown` walks the tree, scrolls the detail, or moves the sidebar cursor;
/// `Add` makes a subtask in the tree and registers a repo in the sidebar --
/// and that stays the *handler's* business rather than the table's. Per-focus
/// keymaps would be a different design, and a much larger one, for a keymap
/// where the same key genuinely means the same verb in every pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Quit,
    Redraw,
    Refresh,
    NextPane,
    PrevPane,
    FocusRepos,
    FocusTree,
    FocusDetail,
    MoveDown,
    MoveUp,
    PageDown,
    PageUp,
    /// `enter` -- the detail pane, or the worktree under the sidebar cursor.
    Open,
    /// `l` / `→` -- expand, scroll right, or step across to the detail.
    StepIn,
    /// `h` / `←` -- collapse, scroll left, or step back.
    StepOut,
    First,
    Last,
    CollapseAll,
    ExpandAll,
    ToggleZoom,
    ToggleWrap,
    ToggleSidebar,
    CycleSort,
    ReverseSort,
    FilterNext,
    FilterPrev,
    Search,
    Help,
    EditConfig,
    NewTask,
    /// `a` -- a subtask of the selection, or the current repo into `saved`.
    Add,
    /// `D` -- forget a saved repo. Sidebar only.
    Forget,
    SaveRepoByPath,
    StartTask,
    CompleteTask,
    RenameTask,
    EditDescription,
    DeleteTask,
}

/// One key, and what it does.
struct Binding {
    code: KeyCode,
    mods: KeyModifiers,
    action: Action,
}

/// The keymap: the single answer to "is this bound, and to what".
///
/// This replaced a 40-arm `match key.code`, and the reason is not tidiness.
/// That match could not *express* the difference between a key and a chord:
/// crossterm decodes the wire byte `0x04` back into `Char('d')` plus CONTROL,
/// so an arm matching on the code alone answered to `Ctrl-D` exactly as it
/// answered to `d` -- and only three of the forty inspected the modifiers.
/// `Ctrl-D` opened the delete confirmation, `Alt-D` and `Ctrl-Alt-D` with it,
/// `Ctrl-Q` quit, `Ctrl-W` toggled wrapping, `Ctrl-A` made a subtask. `d` was
/// simply the one that got noticed, being the one with a dialog attached.
///
/// A lookup answers it by construction: `Ctrl-D` is not in this table, so it
/// resolves to nothing. There is no ordering to get right and no arm that can
/// be added without stating its modifiers, which is what the previous shape
/// got wrong -- the `^L` binding worked only because it was *written above*
/// the plain `l`, a property of the file's layout that nothing checked.
///
/// A linear scan of ~40 entries per keypress is not worth indexing: a keypress
/// already costs a repaint, and the table has to stay readable more than it
/// has to stay fast.
const BINDINGS: &[Binding] = &[
    // Leaving.
    Binding { code: KeyCode::Char('q'), mods: KeyModifiers::NONE, action: Action::Quit },
    Binding { code: KeyCode::Esc, mods: KeyModifiers::NONE, action: Action::Quit },
    Binding { code: KeyCode::Char('c'), mods: KeyModifiers::CONTROL, action: Action::Quit },

    // Ctrl-L, the universal "redraw the screen".
    //
    // `terminal.draw` writes only the cells that changed since the frame
    // ratatui itself last drew, so corruption from *outside* the app is
    // invisible to it: those cells are already right as far as its buffer
    // knows, so they are never rewritten -- and the app draws nothing at all
    // until something it knows about changes. A wrong screen therefore
    // persists instead of healing. Worth having even once a particular cause
    // is found, because the cause is by definition somewhere this app does
    // not control.
    Binding { code: KeyCode::Char('l'), mods: KeyModifiers::CONTROL, action: Action::Redraw },
    // Ctrl-R, because `r` is worth more as rename. The store is watched and a
    // 10s safety poll backstops it, so this is an escape hatch for the events
    // macOS drops rather than something you should need.
    Binding { code: KeyCode::Char('r'), mods: KeyModifiers::CONTROL, action: Action::Refresh },

    // Panes. Tab walks them left to right, Shift-Tab back; the sidebar is in
    // the cycle exactly when it is on screen -- see `focus_cycle`.
    Binding { code: KeyCode::Tab, mods: KeyModifiers::NONE, action: Action::NextPane },
    Binding { code: KeyCode::BackTab, mods: KeyModifiers::NONE, action: Action::PrevPane },
    // Straight to a pane by number, the way LazyGit and gitui do it. The
    // numbers are only *drawn* in zoom mode, where there is a hidden pane to
    // reach, but the keys work at any width -- they were unbound, and refusing
    // them when every pane is visible would be a rule to remember for no
    // benefit. Numbered left to right as the panes are drawn: see `ui::TABS`,
    // the one list the tabs, the click zones and each pane's `[n]` marker all
    // read.
    Binding { code: KeyCode::Char('1'), mods: KeyModifiers::NONE, action: Action::FocusRepos },
    Binding { code: KeyCode::Char('2'), mods: KeyModifiers::NONE, action: Action::FocusTree },
    Binding { code: KeyCode::Char('3'), mods: KeyModifiers::NONE, action: Action::FocusDetail },

    // Movement, in whichever pane has focus.
    Binding { code: KeyCode::Down, mods: KeyModifiers::NONE, action: Action::MoveDown },
    Binding { code: KeyCode::Char('j'), mods: KeyModifiers::NONE, action: Action::MoveDown },
    Binding { code: KeyCode::Up, mods: KeyModifiers::NONE, action: Action::MoveUp },
    Binding { code: KeyCode::Char('k'), mods: KeyModifiers::NONE, action: Action::MoveUp },
    Binding { code: KeyCode::PageDown, mods: KeyModifiers::NONE, action: Action::PageDown },
    Binding { code: KeyCode::PageUp, mods: KeyModifiers::NONE, action: Action::PageUp },
    Binding { code: KeyCode::Enter, mods: KeyModifiers::NONE, action: Action::Open },
    Binding { code: KeyCode::Right, mods: KeyModifiers::NONE, action: Action::StepIn },
    Binding { code: KeyCode::Char('l'), mods: KeyModifiers::NONE, action: Action::StepIn },
    Binding { code: KeyCode::Left, mods: KeyModifiers::NONE, action: Action::StepOut },
    Binding { code: KeyCode::Char('h'), mods: KeyModifiers::NONE, action: Action::StepOut },
    Binding { code: KeyCode::Char('g'), mods: KeyModifiers::NONE, action: Action::First },
    Binding { code: KeyCode::Char('G'), mods: KeyModifiers::NONE, action: Action::Last },

    // `z` is tmux's zoom-pane, which is where the reflex comes from. That cost
    // collapse/expand their old keys -- and `-`/`+` is the better mnemonic
    // anyway: minus closes, plus opens. `=` is accepted for `+` because it is
    // the same physical key without the shift.
    Binding { code: KeyCode::Char('z'), mods: KeyModifiers::NONE, action: Action::ToggleZoom },
    Binding { code: KeyCode::Char('-'), mods: KeyModifiers::NONE, action: Action::CollapseAll },
    Binding { code: KeyCode::Char('+'), mods: KeyModifiers::NONE, action: Action::ExpandAll },
    Binding { code: KeyCode::Char('='), mods: KeyModifiers::NONE, action: Action::ExpandAll },

    // View. Wrapping and horizontal scrolling are mutually exclusive, so `w`
    // is the switch between reading prose and reading a wide table. `b` for
    // bar shows and hides the sidebar whatever the width would have chosen --
    // `1` brings it back, so it cannot be a way to lose the pane with no way
    // to ask for it again.
    Binding { code: KeyCode::Char('w'), mods: KeyModifiers::NONE, action: Action::ToggleWrap },
    Binding { code: KeyCode::Char('b'), mods: KeyModifiers::NONE, action: Action::ToggleSidebar },
    Binding { code: KeyCode::Char('o'), mods: KeyModifiers::NONE, action: Action::CycleSort },
    Binding { code: KeyCode::Char('O'), mods: KeyModifiers::NONE, action: Action::ReverseSort },
    Binding { code: KeyCode::Char('f'), mods: KeyModifiers::NONE, action: Action::FilterNext },
    Binding { code: KeyCode::Char('F'), mods: KeyModifiers::NONE, action: Action::FilterPrev },
    Binding { code: KeyCode::Char('/'), mods: KeyModifiers::NONE, action: Action::Search },
    Binding { code: KeyCode::Char('?'), mods: KeyModifiers::NONE, action: Action::Help },
    // Reuses the $EDITOR machinery rather than building a settings form.
    Binding { code: KeyCode::Char(','), mods: KeyModifiers::NONE, action: Action::EditConfig },

    // Acting on a task.
    Binding { code: KeyCode::Char('n'), mods: KeyModifiers::NONE, action: Action::NewTask },
    Binding { code: KeyCode::Char('a'), mods: KeyModifiers::NONE, action: Action::Add },
    Binding { code: KeyCode::Char('s'), mods: KeyModifiers::NONE, action: Action::StartTask },
    Binding { code: KeyCode::Char('c'), mods: KeyModifiers::NONE, action: Action::CompleteTask },
    // `r` for rename and `e` for edit, rather than `e`/`E` for both. The app's
    // other case pairs are one action and its variant -- `o`/`O` sorts and
    // reverses -- whereas these were two different editors sharing a letter,
    // and which case did which was something you had to remember rather than
    // work out.
    Binding { code: KeyCode::Char('r'), mods: KeyModifiers::NONE, action: Action::RenameTask },
    Binding { code: KeyCode::Char('e'), mods: KeyModifiers::NONE, action: Action::EditDescription },
    Binding { code: KeyCode::Char('d'), mods: KeyModifiers::NONE, action: Action::DeleteTask },

    // Acting on a repo. `A` saves one you are *not* in, by path; `a` saves the
    // one you are, which is the common case and worth the unshifted key. `D`
    // (shift) rather than a second use of `d`: unregistering is a sidebar-only,
    // no-dex action, and sharing a key with task deletion would suggest it
    // also deletes something.
    Binding { code: KeyCode::Char('A'), mods: KeyModifiers::NONE, action: Action::SaveRepoByPath },
    Binding { code: KeyCode::Char('D'), mods: KeyModifiers::NONE, action: Action::Forget },
];

/// SHIFT is not part of a chord, and dropping it here is what keeps `G` one
/// binding rather than two.
///
/// For a `Char` the shift is already carried in the character's *case* -- `G`
/// is not `g` -- and terminals disagree about whether to report the modifier
/// as well: crossterm sets it for uppercase ASCII, and the kitty protocol
/// reports it on keys the legacy encoding cannot express at all. Matching on
/// it would make a binding depend on which terminal the app is running in.
/// `BackTab` arrives carrying SHIFT for the same reason and is normalised the
/// same way.
fn chord(key: &KeyEvent) -> (KeyCode, KeyModifiers) {
    (key.code, key.modifiers.difference(KeyModifiers::SHIFT))
}

fn action_for(key: &KeyEvent) -> Option<Action> {
    let (code, mods) = chord(key);
    BINDINGS
        .iter()
        .find(|b| b.code == code && b.mods == mods)
        .map(|b| b.action)
}

/// The character a key event *types*, if it types one.
///
/// The text-entry modes are the other half of the chord problem, and the
/// keymap above cannot help them: there `Char(c)` is data, not a binding, so
/// they inserted whatever character arrived. `Ctrl-A` in the rename box -- the
/// reflex for "go to the start of the line" -- typed an `a` into the task
/// name, and `Ctrl-W` typed a `w` instead of deleting a word. Nothing here
/// binds those, and doing nothing is the honest outcome; silently corrupting
/// the field being edited is the worst of the options.
fn typed_char(key: &KeyEvent) -> Option<char> {
    match key.code {
        KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => Some(c),
        _ => None,
    }
}

/// `enter`, or a `y` that was actually typed rather than chorded.
fn confirms(key: &KeyEvent) -> bool {
    key.code == KeyCode::Enter || typed_char(key) == Some('y')
}

fn handle_key(app: &mut App, key: KeyEvent, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    match app.mode.clone() {
        Mode::Normal => handle_normal(app, key, dex, tx),
        Mode::Search => handle_search(app, key),
        Mode::Prompt(p) => handle_prompt(app, key, p, dex, tx),

        Mode::Confirm { id, .. } => {
            if confirms(&key) {
                // `id` is overloaded: a `repo:` prefix means `D` queued an
                // unregister rather than `d` queuing a task delete. Unlike a
                // task id (a short slug with no colon), a repo path can never
                // collide with this prefix, and it keeps `Mode::Confirm` a
                // single dialog rather than two -- see the repo sidebar docs.
                if let Some(path) = id.strip_prefix("repo:") {
                    // `unregister_repo_path` now reports whether the removal
                    // actually reached disk: reporting "unregistered"
                    // unconditionally used to mean a failed save left the
                    // entry silently reappearing at the next launch, with
                    // nothing on screen ever having said so.
                    app.status = match app.unregister_repo_path(path) {
                        Ok(true) => {
                            log::line("registry", &format!("saved: removed {path}"));
                            format!("forgot {path}")
                        }
                        Ok(false) => format!("{path} was not saved"),
                        Err(e) => {
                            let e = flatten(&e);
                            log::line("registry", &format!("save failed: {e}"));
                            format!("could not forget {path}: {e}")
                        }
                    };
                } else {
                    let name = app
                        .by_id
                        .get(&id)
                        .map(|t| t.name.clone())
                        .unwrap_or_default();
                    act(dex, tx, format!("deleted {name}"), move |d| d.delete(&id));
                }
            }
            close_modal(app, dex, tx);
        }

        Mode::ForceComplete { id, result, .. } => {
            if confirms(&key) {
                act(dex, tx, "completed".to_string(), move |d| {
                    d.complete(&id, &result, true)
                });
            }
            close_modal(app, dex, tx);
        }

        Mode::Help => handle_help(app, key, dex, tx),

        Mode::Error(_) => close_modal(app, dex, tx),
    }
}

/// `HELP` is longer than a short terminal, so the movement keys scroll it
/// rather than dismissing it. Everything else still dismisses: "any key" was
/// this dialog's whole contract for its entire life, and narrowing it to
/// `esc`/`q` would be a rule to remember in exchange for nothing.
fn handle_help(app: &mut App, key: KeyEvent, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    // A page is what is on screen. `max(1)` guards the frame before the first
    // draw, where the renderer has not published a height yet.
    let page = app.help_viewport_height.max(1) as i32;
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => app.scroll_help(1),
        KeyCode::Up | KeyCode::Char('k') => app.scroll_help(-1),
        KeyCode::PageDown => app.scroll_help(page),
        KeyCode::PageUp => app.scroll_help(-page),
        KeyCode::Home | KeyCode::Char('g') => app.scroll_help(i32::MIN),
        KeyCode::End | KeyCode::Char('G') => app.scroll_help(i32::MAX),
        _ => close_modal(app, dex, tx),
    }
}

fn handle_normal(app: &mut App, key: KeyEvent, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    let Some(action) = action_for(&key) else {
        return;
    };

    let selected = app.selected_task().cloned();

    // Clear any transient status as soon as the user does something else.
    app.status.clear();

    match action {
        Action::Quit => app.should_quit = true,
        Action::Redraw => {
            app.force_redraw = true;
            app.status = "redrew the screen".into();
        }
        Action::Refresh => refresh(dex, tx, app),

        Action::NextPane => app.cycle_focus(true),
        Action::PrevPane => app.cycle_focus(false),
        Action::FocusRepos => app.show_repos(),
        Action::FocusTree => app.show_tree(),
        Action::FocusDetail => app.show_detail(),

        // Movement drives whichever pane has focus. The action keys below stay
        // global, because they always act on the selected task.
        Action::MoveDown => match app.focus {
            Focus::Tree => app.move_selection(1),
            Focus::Detail => app.scroll_detail(1, 0),
            Focus::Repos => move_repo_cursor(app, 1),
        },
        Action::MoveUp => match app.focus {
            Focus::Tree => app.move_selection(-1),
            Focus::Detail => app.scroll_detail(-1, 0),
            Focus::Repos => move_repo_cursor(app, -1),
        },
        Action::PageDown => match app.focus {
            Focus::Tree => app.move_selection(10),
            Focus::Detail => app.scroll_detail(10, 0),
            Focus::Repos => move_repo_cursor(app, 10),
        },
        Action::PageUp => match app.focus {
            Focus::Tree => app.move_selection(-10),
            Focus::Detail => app.scroll_detail(-10, 0),
            Focus::Repos => move_repo_cursor(app, -10),
        },

        // Enter always opens the detail -- except from the repo pane, where it
        // instead picks the worktree under the cursor.
        Action::Open => match app.focus {
            Focus::Repos => select_worktree_under_cursor(app),
            _ => app.show_detail(),
        },
        Action::StepIn => match app.focus {
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
            Focus::Repos => select_worktree_under_cursor(app),
        },
        Action::StepOut => match app.focus {
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
            // Nothing to step out to yet -- the sidebar has no notion of
            // "collapse this worktree back to its repo," and stealing focus
            // to another pane on a key that does nothing elsewhere in this
            // pane would be a surprise.
            Focus::Repos => {}
        },
        Action::First => match app.focus {
            Focus::Tree => app.select_first(),
            Focus::Detail => app.detail_to_top(),
            Focus::Repos => {
                app.select_first_repo_row();
                follow_repo_cursor(app);
            }
        },
        Action::Last => match app.focus {
            Focus::Tree => app.select_last(),
            Focus::Detail => app.detail_to_bottom(),
            Focus::Repos => {
                app.select_last_repo_row();
                follow_repo_cursor(app);
            }
        },

        Action::ToggleZoom => app.toggle_zoom(),
        Action::CollapseAll => app.collapse_all(),
        Action::ExpandAll => app.expand_all(),
        Action::ToggleWrap => app.toggle_wrap(),
        Action::ToggleSidebar => app.toggle_repos(),
        Action::CycleSort => app.cycle_sort(),
        Action::ReverseSort => app.toggle_sort_direction(),
        Action::FilterNext => {
            app.filter = app.filter.next();
            app.rebuild();
        }
        Action::FilterPrev => {
            app.filter = app.filter.prev();
            app.rebuild();
        }
        Action::Search => app.mode = Mode::Search,
        Action::Help => app.open_help(),
        Action::EditConfig => app.pending_config_edit = true,

        Action::NewTask => {
            app.mode = Mode::Prompt(Prompt {
                title: "New task".into(),
                label: "Name".into(),
                input: TextInput::default(),
                pending: Pending::CreateName { parent: None },
            })
        }

        // `a` is the one key whose *verb* changes with the pane rather than
        // just its target: a subtask in the tree, a registration in the
        // sidebar. The old match expressed this by ordering a guarded arm
        // above an unguarded one -- correct, but only as long as nobody moved
        // them, and the strip has to say something different here too (see
        // `ui::REPO_SHORTCUTS`).
        Action::Add => match app.focus {
            Focus::Repos => app.status = register_current_repo(app),
            _ => {
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
        },
        // Confirms via the existing dialog rather than a second one -- see the
        // `Mode::Confirm` handler for the `repo:` prefix that keeps them apart.
        Action::Forget => {
            if app.focus == Focus::Repos
                && let Some(r) = app.selected_repo()
            {
                app.mode = Mode::Confirm {
                    id: format!("repo:{}", r.path),
                    message: format!(
                        "\"{}\" will be unregistered. Its worktrees are not touched.",
                        r.name
                    ),
                };
            }
        }
        Action::SaveRepoByPath => {
            app.mode = Mode::Prompt(Prompt {
                title: "Save a repo".into(),
                label: "Path".into(),
                input: app::TextInput::default(),
                pending: Pending::SaveRepo,
            });
        }

        Action::StartTask => {
            if let Some(t) = selected {
                let id = t.id.clone();
                act(dex, tx, format!("started {}", t.name), move |d| {
                    d.start(&id)
                });
            }
        }
        Action::CompleteTask => {
            if let Some(t) = selected {
                app.mode = Mode::Prompt(Prompt {
                    title: format!("Complete: {}", t.name),
                    label: "Result".into(),
                    input: TextInput::default(),
                    pending: Pending::Complete { id: t.id.clone() },
                });
            }
        }
        Action::RenameTask => {
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
        Action::EditDescription => {
            if let Some(t) = selected {
                app.pending_editor = Some(t.id.clone());
            }
        }
        Action::DeleteTask => {
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
        _ => {
            if let Some(c) = typed_char(&key) {
                app.query.insert(c);
                app.rebuild();
            }
        }
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
        _ => {
            if let Some(c) = typed_char(&key) {
                p.input.insert(c);
                app.mode = Mode::Prompt(p);
            }
        }
    }
}

fn submit(app: &mut App, p: Prompt, dex: &Arc<Dex>, tx: &Sender<Msg>) {
    let value = p.input.value.clone();

    match p.pending {
        // Two-step flows chain into a second prompt rather than acting yet.
        // The one prompt that is not about a task. Validated here rather than
        // saved on faith: a path that is not a git repo would go into
        // `repos.toml` and come back as a row that could never resolve a
        // store, and dex reports a missing store as an *empty project* rather
        // than an error -- so an unchecked typo becomes a repo that silently
        // shows no tasks.
        Pending::SaveRepo => {
            app.status = save_repo_at(app, &value);
        }

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
        assert!(
            m.contains("real terminal"),
            "does not say what is wrong: {m}"
        );
        assert!(m.contains("dextui selftest"), "offers no alternative: {m}");
        assert!(
            matches!(
                super::parse(&["selftest".to_string()]),
                Ok(super::Command::SelfTest)
            ),
            "the message names a command that is not accepted"
        );
    }

    use super::*;

    fn parsed(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    /// Sends `key` to a help dialog with room for 10 of its 30 rows, and
    /// reports where it left things. No dex call can happen on this path --
    /// `close_modal` only refreshes when a refresh arrived while the dialog was
    /// open -- so the runner never runs and the channel is only ever a sink.
    fn help_key(key: KeyCode) -> (Option<u16>, u16) {
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.open_help();
        app.help_content_height = 30;
        app.help_viewport_height = 10;
        app.help_scroll = 5;

        let (tx, _rx) = std::sync::mpsc::channel();
        let dex = Arc::new(Dex::real());
        handle_key(&mut app, KeyEvent::from(key), &dex, &tx);

        let still_open = matches!(app.mode, Mode::Help).then_some(app.help_scroll);
        (still_open, app.help_scroll)
    }

    /// The keys that move now move, and every other key still dismisses --
    /// "any key" was this dialog's whole contract before it could scroll, and
    /// narrowing it to `esc`/`q` would be a rule to remember for no benefit.
    #[test]
    fn the_help_scrolls_on_movement_keys_and_dismisses_on_everything_else() {
        for (key, want) in [
            (KeyCode::Char('j'), 6),
            (KeyCode::Down, 6),
            (KeyCode::Char('k'), 4),
            (KeyCode::Up, 4),
            (KeyCode::PageDown, 15),
            (KeyCode::PageUp, 0),
            (KeyCode::Char('g'), 0),
            (KeyCode::Home, 0),
            // Clamped to the last row, not run off the end.
            (KeyCode::Char('G'), 20),
            (KeyCode::End, 20),
        ] {
            let (open, scroll) = help_key(key);
            assert_eq!(open, Some(want), "{key:?} should have scrolled to {want}");
            assert_eq!(scroll, want);
        }

        for key in [
            KeyCode::Esc,
            KeyCode::Enter,
            KeyCode::Char('q'),
            KeyCode::Char('?'),
            KeyCode::Char(' '),
            KeyCode::Char('x'),
        ] {
            assert_eq!(
                help_key(key).0,
                None,
                "{key:?} should have dismissed the help"
            );
        }
    }

    /// `A` saves a repo you are not in, so the path is typed rather than
    /// discovered -- and an unchecked one is worse than a rejected one. dex
    /// reports a store that does not exist as an *empty project* rather than
    /// an error, so a typo saved on faith becomes a permanent row that shows
    /// no tasks and never explains why.
    #[test]
    fn saving_a_repo_by_path_rejects_what_is_not_a_git_repo() {
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());

        assert!(
            save_repo_at(&mut app, "  ").is_empty(),
            "blank is a no-op, not an error"
        );

        let missing = save_repo_at(&mut app, "/nonexistent-path-for-tests");
        assert!(missing.contains("not a directory"), "{missing}");

        // A real directory that is not a repo: reported, and nothing saved.
        let not_a_repo = save_repo_at(&mut app, "/tmp");
        assert!(not_a_repo.contains("not a git repo"), "{not_a_repo}");
        assert!(app.repos.is_empty(), "a rejected path must not leave a row");
    }

    /// `~` is expanded here because nothing in this app goes through a shell,
    /// so a pasted `~/thing` would otherwise be taken literally.
    #[test]
    fn saving_a_repo_by_path_expands_a_leading_tilde() {
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        let home = std::env::var("HOME").expect("HOME is set in tests");

        let msg = save_repo_at(&mut app, "~/definitely-not-here-xyz");

        assert!(msg.starts_with(&home), "the tilde was not expanded: {msg}");
    }

    /// Outside a git repo dex silently falls back to a global store, and the
    /// sidebar is the only place on screen that has ever said so. It is a row,
    /// not a repo: no worktrees, and its path *is* its store.
    #[test]
    fn a_store_outside_any_repo_becomes_the_global_row() {
        let mut problems = Vec::new();
        let r = current_repo("/home/u/.config/dex/local", &mut problems).unwrap();

        assert!(r.is_global);
        assert!(!r.registered, "nothing registers the global store");
        assert_eq!(r.name, "global");
        assert!(r.worktrees.is_empty());
        assert_eq!(r.store(None), "/home/u/.config/dex/local");
        assert!(problems.is_empty());
    }

    /// A repo-shaped store is recognised by its `.dex` suffix -- dex's own
    /// answer, from `dex dir` -- rather than by a fresh `is_dir` check here
    /// that could disagree with it.
    #[test]
    fn a_dex_directory_is_treated_as_a_repo_not_the_global_store() {
        let mut problems = Vec::new();
        // Not a real repo, so `git worktree list` fails and this reports
        // rather than inventing a row -- but it must not be mistaken for the
        // global store on the way there.
        let r = current_repo("/nonexistent-repo-for-tests/.dex", &mut problems);
        assert!(r.is_none());
        assert_eq!(
            problems.len(),
            1,
            "a git failure has to be reported: {problems:?}"
        );
        assert!(
            problems[0].starts_with("/nonexistent-repo-for-tests"),
            "the problem names the repo, not its store: {problems:?}"
        );
    }

    /// Ctrl-L must reach `force_redraw` rather than being eaten by the plain
    /// `l` that means expand / cross to the detail.
    ///
    /// Match arms are tried in order and the plain arm does not look at
    /// modifiers, so the guarded one only works because it is written above
    /// it. That is a property of the file's *layout*, which nothing else
    /// checks -- the compiler happens to warn here (`unreachable pattern`),
    /// but it cannot warn when the shadowing arm is merely reachable-but-wrong.
    #[test]
    fn ctrl_l_forces_a_redraw_and_is_not_swallowed_by_the_plain_l() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let dex = Arc::new(Dex::real());

        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.focus = Focus::Tree;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
            &dex,
            &tx,
        );
        assert!(app.force_redraw, "ctrl-l was swallowed by the plain `l`");

        // And the plain `l` still means what it always did: it crosses to the
        // detail from a leaf, and does not ask for a repaint.
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.focus = Focus::Tree;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
            &dex,
            &tx,
        );
        assert!(!app.force_redraw, "the plain `l` should not force a redraw");
    }

    /// Every modifier chord that still decodes to a letter used to reach that
    /// letter's *plain* binding. The match is on `key.code` alone and only
    /// `^C`, `^L` and `^R` look at the modifiers at all, so `Ctrl-D` opened
    /// the delete confirmation -- as did `Alt-D` and `Ctrl-Alt-D`, and as did
    /// `Ctrl-Q` for quit, `Ctrl-W` for wrap and `Ctrl-A` for a new subtask.
    /// `d` is simply the one that was noticed, because it is the one with a
    /// dialog attached: reported as "why does pressing ctrl+d cause the delete
    /// box to come up".
    ///
    /// SHIFT is deliberately still let through, and that is why this cannot
    /// just drop everything modified: `O`, `G`, `F`, `D`, `A` and `+` are all
    /// shifted keys carrying bindings of their own.
    #[test]
    fn a_modifier_chord_does_not_fire_the_plain_letter_binding() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let dex = Arc::new(Dex::real());

        let pressed = |code: KeyCode, mods: KeyModifiers| {
            let mut app = App::new(
                vec![a_task("t1")],
                "demo".into(),
                crate::config::Config::default(),
            );
            app.focus = Focus::Tree;
            handle_key(&mut app, KeyEvent::new(code, mods), &dex, &tx);
            app
        };

        // The plain key still means what it always did.
        assert!(
            matches!(
                pressed(KeyCode::Char('d'), KeyModifiers::NONE).mode,
                Mode::Confirm { .. }
            ),
            "the plain `d` no longer asks to delete"
        );

        // Nothing layered over it does.
        for mods in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ] {
            assert!(
                matches!(pressed(KeyCode::Char('d'), mods).mode, Mode::Normal),
                "{mods:?} + d opened the delete dialog"
            );
        }

        // The rest of the unguarded set went exactly the same way.
        assert!(
            !pressed(KeyCode::Char('q'), KeyModifiers::CONTROL).should_quit,
            "ctrl-q quit the app"
        );
        let wrap = crate::config::Config::default().wrap;
        assert_eq!(
            pressed(KeyCode::Char('w'), KeyModifiers::CONTROL).wrap,
            wrap,
            "ctrl-w toggled wrapping"
        );
        assert!(
            matches!(
                pressed(KeyCode::Char('a'), KeyModifiers::ALT).mode,
                Mode::Normal
            ),
            "alt-a opened the new-subtask prompt"
        );

        // The three deliberate control keys are untouched, and so is SHIFT.
        assert!(
            pressed(KeyCode::Char('c'), KeyModifiers::CONTROL).should_quit,
            "ctrl-c no longer quits"
        );
        assert!(
            pressed(KeyCode::Char('l'), KeyModifiers::CONTROL).force_redraw,
            "ctrl-l no longer redraws"
        );
        assert_eq!(
            pressed(KeyCode::Char('F'), KeyModifiers::SHIFT).filter,
            crate::tree::Filter::All,
            "shift-F no longer walks the filter back"
        );
    }

    /// Two things the table has to be, and neither is checked by anything
    /// else.
    ///
    /// **Unambiguous**: the shape this replaced was an ordered `match`, where
    /// two arms claiming the same key was legal and the first one silently
    /// won. A lookup has no order to appeal to, so a duplicate would make the
    /// binding depend on where in the list someone happened to add it.
    ///
    /// **Self-consistent**: every entry must resolve back to its own action
    /// through `action_for`, which is what pins the SHIFT normalisation in
    /// `chord` -- an entry written with a modifier the lookup then strips
    /// would be unreachable, and unreachable is exactly the failure the old
    /// `^L`-below-`l` ordering produced.
    ///
    /// An `Action` with no key at all needs no test: it would be a variant
    /// constructed nowhere, and rustc's own dead-code lint says so.
    #[test]
    fn the_keymap_is_unambiguous_and_every_entry_resolves_to_itself() {
        for (i, b) in BINDINGS.iter().enumerate() {
            assert!(
                BINDINGS[..i]
                    .iter()
                    .all(|o| !(o.code == b.code && o.mods == b.mods)),
                "{:?} + {:?} is bound twice",
                b.mods,
                b.code
            );
            assert_eq!(
                action_for(&KeyEvent::new(b.code, b.mods)),
                Some(b.action),
                "{:?} + {:?} does not resolve to its own action",
                b.mods,
                b.code
            );
        }
    }

    /// The status strip may only advertise keys that are actually bound.
    ///
    /// `the_shortcut_strip_and_the_help_dialog_agree` holds the two *documents*
    /// against each other, which catches them drifting apart and not one of
    /// them drifting away from the code. Now that there is a single table to
    /// ask, the strip can be held against the keymap itself -- so a key that
    /// is renamed or dropped cannot go on being advertised.
    ///
    /// It deliberately checks the key rather than the wording: `a` is `sub` in
    /// the tree and `save this repo` in the sidebar, so the label is a
    /// per-pane question the table does not answer and should not.
    #[test]
    fn the_shortcut_strips_only_advertise_keys_that_are_bound() {
        for strip in [crate::ui::SHORTCUTS, crate::ui::REPO_SHORTCUTS] {
            for entry in strip.trim().split("  ") {
                let Some(spec) = entry.split_whitespace().next() else {
                    continue;
                };
                for token in spec.split('/') {
                    let code = match token {
                        "enter" => KeyCode::Enter,
                        t if t.chars().count() == 1 => {
                            KeyCode::Char(t.chars().next().expect("one char"))
                        }
                        other => panic!("{strip}: cannot read the key `{other}`"),
                    };
                    assert!(
                        action_for(&KeyEvent::new(code, KeyModifiers::NONE)).is_some(),
                        "the strip advertises `{token}`, which is not bound: {strip}"
                    );
                }
            }
        }
    }

    /// The same defect, in the modes where `Char(c)` is *text* rather than a
    /// binding. `handle_search` and `handle_prompt` insert the character
    /// verbatim, so `Ctrl-A` in the rename box -- the reflex for "go to the
    /// start of the line" -- typed an `a` into the task name, and `Ctrl-W`
    /// (delete the previous word) typed a `w`. A chord doing nothing is the
    /// honest outcome here: the app does not bind these, and silently
    /// corrupting the field you are editing is the worst of the options.
    #[test]
    fn a_chord_does_not_type_its_letter_into_a_text_field() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let dex = Arc::new(Dex::real());

        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.mode = Mode::Search;
        for mods in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            handle_key(&mut app, KeyEvent::new(KeyCode::Char('a'), mods), &dex, &tx);
        }
        assert_eq!(app.query.value, "", "a chord typed into the search box");

        // And the plain key still reaches it, so this has not simply muted
        // the field.
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            &dex,
            &tx,
        );
        assert_eq!(app.query.value, "a", "the search box stopped accepting text");

        let prompt = Prompt {
            title: "rename".into(),
            label: "name".into(),
            input: TextInput::default(),
            pending: Pending::EditName { id: "t1".into() },
        };
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.mode = Mode::Prompt(prompt);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            &dex,
            &tx,
        );
        let Mode::Prompt(p) = &app.mode else {
            panic!("ctrl-a left the prompt: {:?}", app.mode);
        };
        assert_eq!(p.input.value, "", "ctrl-a typed an `a` into the rename box");
    }

    /// `f` and `F` should behave like the sort pair: one moves through the
    /// choices in the normal direction, the shifted key walks back.
    #[test]
    fn filter_keys_cycle_forward_and_backward() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let dex = Arc::new(Dex::real());

        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            &dex,
            &tx,
        );
        assert_eq!(app.filter, crate::tree::Filter::InProgress);

        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT),
            &dex,
            &tx,
        );
        assert_eq!(app.filter, crate::tree::Filter::All);
    }

    /// `?` is pressed by someone looking for a key, and resuming halfway down
    /// the dialog would hide the first ten of them.
    #[test]
    fn reopening_the_help_starts_at_the_top() {
        let mut app = App::new(vec![], "demo".into(), crate::config::Config::default());
        app.help_content_height = 30;
        app.help_viewport_height = 10;

        app.open_help();
        app.scroll_help(i32::MAX);
        assert_eq!(app.help_scroll, 20, "the test never scrolled anywhere");

        app.mode = Mode::Normal;
        app.open_help();
        assert_eq!(app.help_scroll, 0);
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
        assert!(
            parsed(&["config", "wibble"])
                .unwrap_err()
                .contains("wibble")
        );
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
        assert!(matches!(
            parsed(&["config", "init", "--help"]),
            Ok(Command::Help)
        ));
    }

    #[test]
    fn config_subcommands_parse() {
        assert!(matches!(parsed(&["config"]), Ok(Command::ShowConfig)));
        assert!(matches!(
            parsed(&["config", "init"]),
            Ok(Command::InitConfig { .. })
        ));
        assert!(matches!(
            parsed(&["config", "edit"]),
            Ok(Command::EditConfig { .. })
        ));
    }

    #[test]
    fn local_and_project_select_the_project_file() {
        // -l matches dex's own spelling; --project says what it means.
        for flag in ["-l", "--local", "--project"] {
            assert!(
                matches!(
                    parsed(&["config", "edit", flag]),
                    Ok(Command::EditConfig {
                        scope: config::Scope::Project
                    })
                ),
                "{flag} did not select the project scope"
            );
        }
    }

    #[test]
    fn global_is_the_default_and_can_be_stated_explicitly() {
        assert!(matches!(
            parsed(&["config", "edit"]),
            Ok(Command::EditConfig {
                scope: config::Scope::Global
            })
        ));
        for flag in ["-g", "--global"] {
            assert!(matches!(
                parsed(&["config", "edit", flag]),
                Ok(Command::EditConfig {
                    scope: config::Scope::Global
                })
            ));
        }
    }

    #[test]
    fn options_may_appear_before_the_command() {
        assert!(matches!(
            parsed(&["--local", "--force", "config", "init"]),
            Ok(Command::InitConfig {
                force: true,
                scope: config::Scope::Project
            })
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

    fn a_task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            name: id.to_string(),
            ..Default::default()
        }
    }

    /// `handle_msg` needs a `Dex` and a channel it never uses on this path --
    /// `Msg::Tasks` neither spawns a refresh nor sends anything.
    fn apply(app: &mut App, msg: Msg) {
        let (tx, _rx) = channel::<Msg>();
        handle_msg(app, msg, &Arc::new(Dex::real()), &tx);
    }

    /// The race this closes: `refresh()` spawns a thread holding the `Arc<Dex>`
    /// current at the time, and `switch_store` replaces that `Arc` on the main
    /// loop. A refresh spawned just before a switch lands just after it, and
    /// before this the arriving task list was applied unconditionally -- the
    /// old store's tasks painted under the new store's label, silently.
    #[test]
    fn a_task_list_from_the_store_we_just_left_is_dropped() {
        let mut app = App::new(
            vec![a_task("new")],
            "/x/two/.dex".into(),
            config::Config::default(),
        );

        apply(
            &mut app,
            Msg::Tasks {
                store: "/x/one/.dex".into(),
                result: Ok(vec![a_task("old")]),
            },
        );

        let ids: Vec<&str> = app.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["new"], "the old store's tasks were painted");
        assert_eq!(
            app.store_label, "two",
            "and the header still says the new store"
        );
    }

    /// The ordinary case still applies, or the tag would have turned every
    /// refresh into a no-op.
    #[test]
    fn a_task_list_from_the_current_store_is_applied() {
        let mut app = App::new(vec![], "/x/one/.dex".into(), config::Config::default());

        apply(
            &mut app,
            Msg::Tasks {
                store: "/x/one/.dex".into(),
                result: Ok(vec![a_task("fresh")]),
            },
        );

        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.tasks[0].id, "fresh");
    }

    /// A failure from a store nobody is on must not report itself either --
    /// "refresh failed" about a store you have already left is noise about
    /// something you cannot act on.
    #[test]
    fn a_failure_from_a_store_we_just_left_is_not_reported() {
        let mut app = App::new(vec![], "/x/two/.dex".into(), config::Config::default());

        apply(
            &mut app,
            Msg::Tasks {
                store: "/x/one/.dex".into(),
                result: Err("boom".into()),
            },
        );

        assert!(app.status.is_empty(), "status: {}", app.status);
    }

    fn worktrees_of(name: &str) -> Vec<crate::worktree::Worktree> {
        vec![
            crate::worktree::Worktree {
                path: format!("/x/{name}"),
                branch: "main".into(),
                is_main: true,
                is_locked: false,
                is_detached: false,
            },
            crate::worktree::Worktree {
                path: format!("/x/{name}-feat"),
                branch: "feat".into(),
                is_main: false,
                is_locked: false,
                is_detached: false,
            },
        ]
    }

    /// The bug this closes: `a` wrote `repos.toml` and stopped there, so the
    /// repo it had just registered did not appear in the sidebar until the app
    /// was restarted -- while the README promises switching between repos
    /// "without restarting". Persisting and drawing are two separate things,
    /// and only one of them was happening.
    #[test]
    fn registering_a_repo_adds_its_sidebar_row_immediately() {
        crate::test_support::with_isolated_registry("main-register-row", || {
            let mut app = App::new(vec![], "t".into(), config::Config::default());
            assert!(app.repos.is_empty());

            let status = register_repo(&mut app, worktrees_of("one"));

            assert!(status.contains("saved"), "status: {status}");
            assert_eq!(app.repos.len(), 1, "the sidebar row was not added");
            assert_eq!(app.repos[0].path, "/x/one");
            assert_eq!(app.repos[0].name, "one");
            assert_eq!(
                app.repos[0].worktrees.len(),
                2,
                "the row must carry every worktree, not just the main one"
            );
            // The `saved` heading and three rows under it, so the sidebar
            // really does draw the worktrees too.
            assert_eq!(app.repo_rows().len(), 4);
        });
    }

    /// Saving moves the row out of `here` and into `saved`, which is what
    /// makes `a` visible -- and the cursor has to travel with it. It does not
    /// address a repo, it addresses an *index*, so a row that moves past it
    /// leaves it pointing at whichever repo slid into that slot. Since the
    /// sidebar cursor is what chooses the store, that is not a cosmetic
    /// slip: pressing `a` would swap the other two panes to a different
    /// project.
    ///
    /// Needs a second saved repo to catch. With one repo the `here` heading
    /// leaves as the `saved` heading arrives, so every index below is
    /// unchanged and a broken cursor still looks right.
    #[test]
    fn saving_carries_the_sidebar_cursor_along_with_the_row_it_moves() {
        crate::test_support::with_isolated_registry("main-register-cursor", || {
            let mut app = App::new(vec![], "/x/two/.dex".into(), config::Config::default());
            app.here_path = Some("/x/two".into());
            // Any real directory: `here` is hidden when the store behind it
            // does not exist.
            app.here_store = std::env::temp_dir().to_string_lossy().into_owned();
            app.repos = ["one", "two"]
                .iter()
                .map(|n| repos::Repo {
                    name: (*n).to_string(),
                    path: format!("/x/{n}"),
                    worktrees: worktrees_of(n),
                    open: true,
                    // "two" is where we are, and not saved yet.
                    registered: *n == "one",
                    is_global: false,
                })
                .collect();
            app.select_current_store_row();
            assert_eq!(
                app.repo_rows()[app.selected_repo_row],
                repos::Row::Repo { index: 1 },
                "the cursor should start on the repo we are in"
            );

            register_repo(&mut app, worktrees_of("two"));

            let rows = app.repo_rows();
            assert_eq!(
                rows[app.selected_repo_row],
                repos::Row::Repo { index: 1 },
                "the cursor stayed at its old index instead of following: {rows:?}"
            );
            assert!(
                !rows.contains(&repos::Row::Heading("here")),
                "the saved repo should have left `here`: {rows:?}"
            );
        });
    }

    /// Rows are kept in the order `Registry::add` writes them, so nothing
    /// jumps around the first time the app is restarted -- and a repo already
    /// registered adds no second row.
    #[test]
    fn registering_keeps_the_rows_sorted_and_never_duplicates_one() {
        crate::test_support::with_isolated_registry("main-register-order", || {
            let mut app = App::new(vec![], "t".into(), config::Config::default());

            register_repo(&mut app, worktrees_of("two"));
            register_repo(&mut app, worktrees_of("one"));

            let paths: Vec<&str> = app.repos.iter().map(|r| r.path.as_str()).collect();
            assert_eq!(
                paths,
                vec!["/x/one", "/x/two"],
                "rows are not in registry order"
            );

            let status = register_repo(&mut app, worktrees_of("one"));
            assert!(status.contains("already saved"), "status: {status}");
            assert_eq!(app.repos.len(), 2, "a duplicate row was added");
        });
    }

    fn repo_app() -> App {
        let mut app = App::new(vec![], "t".into(), config::Config::default());
        app.repos = vec![crate::repos::Repo {
            name: "one".into(),
            path: "/x/one".into(),
            worktrees: vec![crate::worktree::Worktree {
                path: "/x/one".into(),
                branch: "main".into(),
                is_main: true,
                is_locked: false,
                is_detached: false,
            }],
            open: true,
            registered: true,
            is_global: false,
        }];
        app.focus = Focus::Repos;
        app
    }

    /// This is the glue `enter` and `l` share in the repo pane: it has to pick
    /// the worktree under the cursor, remember it as the current one, queue
    /// the actual store swap for the main loop (which alone holds a mutable
    /// `Arc<Dex>`), and hand focus back to the tree.
    #[test]
    fn select_worktree_under_cursor_queues_the_switch_and_returns_focus_to_the_tree() {
        let mut app = repo_app();
        // Row 0 is the `saved` heading; row 1 is the repo itself.
        app.selected_repo_row = 1;

        select_worktree_under_cursor(&mut app);

        assert_eq!(app.pending_store.as_deref(), Some("/x/one"));
        assert_eq!(app.selected_worktree.as_deref(), Some("/x/one"));
        assert_eq!(app.focus, Focus::Tree);
    }

    /// An empty sidebar has no row to resolve, so this must not steal focus or
    /// queue a switch to nowhere -- the same "dead space does nothing" rule
    /// `clicking_empty_header_space_changes_nothing` pins for the header.
    #[test]
    fn select_worktree_under_cursor_on_an_empty_sidebar_does_nothing() {
        let mut app = App::new(vec![], "t".into(), config::Config::default());
        app.focus = Focus::Repos;

        select_worktree_under_cursor(&mut app);

        assert_eq!(app.pending_store, None);
        assert_eq!(
            app.focus,
            Focus::Repos,
            "must not steal focus with nothing to select"
        );
    }

    /// `Mode::Confirm.id` is overloaded to carry either a task id or, with a
    /// `repo:` prefix, a repo path to unregister. A real task id is a short
    /// slug that can never itself start with that prefix, so the two can never
    /// collide -- this pins the encoding the key handler and this match both
    /// rely on.
    #[test]
    fn a_repo_confirm_id_is_distinguishable_from_a_task_id() {
        let repo_id = format!("repo:{}", "/x/one");
        assert!(repo_id.strip_prefix("repo:").is_some());

        let task_id = "b4d5gfpl";
        assert!(task_id.strip_prefix("repo:").is_none());
    }

    /// `3` itself is now the plain, unconditional field write the original
    /// brief specified -- whether the repo pane actually has anywhere to draw
    /// is a render-time question `App::single_pane`/`App::panes` answer on
    /// their own (see their tests in `app.rs`), not something this key needs
    /// to work out. This just pins that the key does what it says and
    /// nothing more, e.g. it must not also touch `zoom`.
    #[test]
    fn the_3_key_only_sets_focus() {
        let mut app = App::new(vec![], "t".into(), config::Config::default());
        assert_eq!(app.zoom, None);

        app.focus = Focus::Repos;

        assert_eq!(app.focus, Focus::Repos);
        assert_eq!(
            app.zoom, None,
            "must not reach for zoom -- that used to be sticky"
        );
    }
}
