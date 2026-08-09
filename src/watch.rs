//! Signals that the dex store changed.
//!
//! Reports only *that* something changed, never *what*: the caller re-reads via
//! `dex list --json`. That keeps us off dex's private on-disk format while
//! costing nothing at all when the store is idle.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::log;

pub const DEBOUNCE: Duration = Duration::from_millis(250);
pub const SAFETY: Duration = Duration::from_secs(10);

/// How often to look for a store to attach to while there is no watcher.
///
/// A tick with no watcher does two stats and no subprocess -- microseconds --
/// so it can afford to be much more frequent than `SAFETY`, which is sized for
/// a case that ends in a `dex list`. It is worth the difference: this is the
/// interval a brand-new store's first tasks wait behind, and ten seconds of an
/// empty pane is long enough to read as "it never loads anything" rather than
/// as "it has not looked yet".
pub const DISCONNECTED_POLL: Duration = Duration::from_secs(1);

/// How often a store with nothing happening to it may repeat itself in the log.
///
/// The unchanged tick has to be logged at all, because "decided *not* to
/// refresh" is invisible by design -- a quiet tick does nothing, which is
/// correct and indistinguishable from a dead watcher without a line saying so.
/// `log`'s module docs name that branch as the reason the log exists.
///
/// But it has to be logged *once*, not sixty times a minute. A worktree with no
/// `.dex` polls every second (`DISCONNECTED_POLL`) and every one of those ticks
/// found nothing and said so. On a real registry -- four such worktrees under
/// one repo -- that was over four lines a second, which reached the 1 MB cap in
/// about three quarters of an hour and truncated the file at the next launch.
/// The log flooded away the history it exists to keep, and what it lost is
/// exactly the kind this is for: what happened *before* the fault you are now
/// trying to reproduce.
///
/// So repeats collapse to one line per minute per store, carrying the count of
/// ticks it stands for. Nothing is hidden: an unbroken run of
/// `unchanged (x60)` still proves the loop is alive and still deciding, and the
/// first quiet tick after any activity is always logged in full -- the counter
/// resets on a change, so the return to quiet is never what gets swallowed.
const QUIET_LOG: Duration = Duration::from_secs(60);

/// The message for a quiet tick, or `None` if this one should stay silent.
///
/// Kept separate from the wall clock and from `log::line`'s file I/O, both of
/// which are awkward to unit test -- `Instant` needs a real sleep to advance,
/// and the log path is a `OnceLock` a test cannot re-resolve. Everything this
/// function needs to decide is a `u32` and an `Option<Duration>`, so it is
/// tested directly with fabricated values instead.
///
/// `run` is the number of consecutive unchanged ticks since a quiet tick was
/// last logged, including this one. `since_last_log` is `None` on the first
/// quiet tick after real activity -- an event, or a tick that found a change
/// -- which is the one that must never be swallowed, so it always logs, in
/// full and uncollapsed. Later ticks stay silent until `QUIET_LOG` has passed,
/// at which point one line reports `run`.
fn quiet_tick_message(dir: &str, run: u32, since_last_log: Option<Duration>) -> Option<String> {
    if since_last_log.is_some_and(|d| d < QUIET_LOG) {
        return None;
    }
    Some(if run <= 1 {
        format!("tick {dir} unchanged")
    } else {
        format!("tick {dir} unchanged (x{run})")
    })
}

/// Stops the watcher when dropped.
///
/// The `notify` watcher itself lives on the thread rather than in here, because
/// whether one can exist at all is not decided once: a store directory that
/// does not exist yet cannot be watched, and this has to notice when it appears
/// and attach then. So the guard signals instead of owning, and the thread
/// releases the watcher on its way out.
pub struct StoreWatcher {
    stop: Arc<AtomicBool>,
    /// Only ever used to wake the thread out of its `recv_timeout` so it sees
    /// `stop` now rather than at the end of an interval it is already inside.
    /// Without it a dropped guard leaves a thread polling a store nobody is
    /// looking at for up to `SAFETY`, still able to trigger a refresh -- which
    /// is what `switch_store` does on every worktree change.
    wake: Sender<()>,
    /// Whether a `notify` watcher is currently attached, as opposed to the
    /// store being carried by the poll alone. Written by the thread every time
    /// that answer changes.
    ///
    /// Carried only in test builds: the thread's copy is what does the work,
    /// and keeping a second one here in a release build would be a field
    /// nothing ever reads.
    #[cfg(test)]
    attached: Arc<AtomicBool>,
}

impl StoreWatcher {
    /// Whether this store is being watched, or only polled.
    ///
    /// Test-only because nothing on screen distinguishes the two -- both keep
    /// the view correct, and only the latency differs. That is exactly why the
    /// re-attach it exists to check went unnoticed: the behaviour was right and
    /// merely ten seconds late, forever.
    #[cfg(test)]
    fn is_attached(&self) -> bool {
        self.attached.load(Ordering::Relaxed)
    }
}

impl Drop for StoreWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.wake.send(());
    }
}

/// A cheap fingerprint of a store's `tasks.jsonl`: modification time, length
/// and inode. `None` means the file does not exist (an ordinary state for a
/// worktree with no store yet, not a failure).
///
/// All three change under an atomic rename (write a temp file, rename it over
/// the original), which is exactly how dex writes and exactly the case macOS
/// can drop a notify event for -- so this is what the safety timeout compares
/// against, instead of trusting its own tick blindly.
type Stat = (SystemTime, u64, u64);

fn stat(store_dir: &str) -> Option<Stat> {
    let meta = std::fs::metadata(Path::new(store_dir).join("tasks.jsonl")).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len(), inode(&meta)))
}

/// The inode, where the platform has one.
///
/// It is the strongest third of the fingerprint -- an atomic rename gives the
/// new file a different one even when the mtime resolution and the length both
/// happen to match. `MetadataExt` is unix-only and was this crate's only
/// unix-only import, so off unix the gate falls back to mtime and length,
/// which is weaker but not broken: dex rewrites the whole file, so a change
/// that moves neither is a rewrite to a byte-identical file within one clock
/// tick.
#[cfg(unix)]
fn inode(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino()
}

#[cfg(not(unix))]
fn inode(_meta: &std::fs::Metadata) -> u64 {
    0
}

/// Attaches a `notify` watcher to `store_dir`, or reports that it could not.
///
/// Separated out because attaching is not a one-off: a brand-new project has no
/// store directory until the first task is created, and a store can also be
/// deleted out from under a running app, so this is called again on every tick
/// that finds itself without a watcher.
fn attach(store_dir: &str, raw_tx: &Sender<()>) -> Option<RecommendedWatcher> {
    if !Path::new(store_dir).is_dir() {
        return None;
    }
    let tx = raw_tx.clone();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = tx.send(());
        }
    })
    .ok()?;
    w.watch(Path::new(store_dir), RecursiveMode::NonRecursive)
        .ok()?;
    Some(w)
}

/// Starts watching `store_dir`, sending `()` on `out` whenever it changes.
///
/// A brand-new project has no store directory until the first task is created,
/// and watching a missing path fails, so in that case we fall back to the poll
/// alone -- which picks the store up when it appears, and then attaches a real
/// watcher to it.
pub fn spawn(store_dir: &str, out: Sender<()>) -> StoreWatcher {
    spawn_inner(store_dir, out, SAFETY)
}

/// The real implementation, taking the safety interval as a parameter so
/// tests can drive it in milliseconds rather than waiting out the real 10s
/// constant. `spawn` is the only public entry point; this stays private.
fn spawn_inner(store_dir: &str, out: Sender<()>, safety: Duration) -> StoreWatcher {
    let (raw_tx, raw_rx): (Sender<()>, Receiver<()>) = channel();

    // Attached here rather than on the thread, even though the thread is what
    // owns it from now on: callers write to the store immediately after
    // `spawn` returns, and a watcher that only exists once the thread has been
    // scheduled would miss that write and leave it to the poll. Attaching
    // first makes "watched from the moment spawn returns" true again, which is
    // what it always was before this could re-attach at all.
    let mut watcher = attach(store_dir, &raw_tx);
    log::line(
        "watch",
        &match &watcher {
            Some(_) => format!("registered {store_dir}"),
            None => format!("no watcher for {store_dir}; polling until it exists"),
        },
    );

    // The baseline every tick compares against, read here for the same reason
    // the watcher is attached here: taking it on the thread instead means it is
    // taken at some unknowable moment *after* `spawn` returned, so a write in
    // that window lands in the baseline itself and every later tick then
    // correctly reports it as unchanged -- the change is not late, it is gone.
    // With a watcher attached that is survivable, since the write also fires an
    // event; with none -- a store that does not exist yet, which is exactly
    // when this matters -- nothing ever reports it and the pane simply stays
    // empty. Read before `spawn` returns, the window does not exist.
    let mut last = stat(store_dir);

    // Drives `quiet_tick_message`. `quiet_run` counts consecutive unchanged
    // ticks since a quiet tick was last logged; `quiet_logged_at` is `None`
    // exactly when the next one must log in full -- start and stay-quiet both
    // reset it as if activity had just happened, so the very first tick after
    // `spawn` is never itself the one that gets collapsed.
    let mut quiet_run: u32 = 0;
    let mut quiet_logged_at: Option<Instant> = None;

    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let attached = Arc::new(AtomicBool::new(watcher.is_some()));
    let is_attached = Arc::clone(&attached);
    let wake = raw_tx.clone();
    let dir = store_dir.to_string();

    thread::spawn(move || {
        // Also what keeps `raw_rx` connected for the life of this thread when
        // no watcher exists to hold a clone of its own -- a worktree with no
        // store yet. Without it, `raw_tx` drops the instant this closure takes
        // ownership of nothing, `raw_rx` reads as disconnected on the very
        // first tick, and the thread exits before ever reaching the timeout
        // branch below -- silently defeating the exact case that branch
        // exists to cover.
        let raw_tx = raw_tx;

        loop {
            if stopped.load(Ordering::Relaxed) {
                return;
            }

            // Without a watcher this is the *only* thing that will ever notice
            // the store, so it runs far more often -- and costs two stats, not
            // a `dex list`, on every tick that finds nothing.
            let interval = match watcher {
                Some(_) => safety,
                None => safety.min(DISCONNECTED_POLL),
            };

            let received = raw_rx.recv_timeout(interval);
            // Checked again here, not only at the top: the guard's drop wakes
            // this thread on purpose, and that wake arrives as an ordinary
            // `Ok(())` it must not go on to treat as a store event.
            if stopped.load(Ordering::Relaxed) {
                return;
            }

            match received {
                Ok(()) => {
                    log::line("watch", &format!("event {dir}"));
                    // A single dex write touches the file several times; swallow
                    // the burst so it costs one `dex list`, not one per event.
                    while raw_rx.recv_timeout(DEBOUNCE).is_ok() {}
                    // Refreshed here so the next safety tick compares against
                    // what this event already reported, not a stale baseline
                    // from before it.
                    last = stat(&dir);
                    // Real activity, so the next quiet tick must log in full --
                    // see `quiet_tick_message`.
                    quiet_run = 0;
                    quiet_logged_at = None;
                    if out.send(()).is_err() {
                        return;
                    }
                }
                // Writers often replace tasks.jsonl via a temp file plus rename,
                // which macOS can surface as an event we never see. This bounds
                // how stale the view can get -- but a blind resend here would
                // pay for a full `dex list` (~180ms of Node startup) every tick
                // just to discover, almost always, that nothing changed. A
                // stat is microseconds against that, so only report when it
                // actually disagrees with the last one we saw.
                Err(RecvTimeoutError::Timeout) => {
                    // Whether a watcher can exist is a question with a
                    // different answer at different times, so it is asked
                    // every tick rather than once at startup. Deciding it once
                    // meant a store created after launch was found by the poll
                    // -- correctly, and the log said so -- and then stayed on
                    // the poll's interval for the life of the process, with
                    // nothing ever attaching to it. A store that is deleted
                    // gets the reverse: the watcher goes, so the next tick
                    // re-attaches once it comes back.
                    let present = Path::new(&dir).is_dir();
                    if !present {
                        watcher = None;
                    } else if watcher.is_none() {
                        watcher = attach(&dir, &raw_tx);
                        if watcher.is_some() {
                            log::line("watch", &format!("registered {dir} (late)"));
                        }
                    }
                    is_attached.store(watcher.is_some(), Ordering::Relaxed);

                    let now = stat(&dir);
                    if now != last {
                        log::line("watch", &format!("tick {dir} changed"));
                        last = now;
                        // Real activity, same as the `Ok(())` branch above.
                        quiet_run = 0;
                        quiet_logged_at = None;
                        if out.send(()).is_err() {
                            return;
                        }
                    } else {
                        quiet_run += 1;
                        let since = quiet_logged_at.map(|t| t.elapsed());
                        if let Some(msg) = quiet_tick_message(&dir, quiet_run, since) {
                            log::line("watch", &msg);
                            quiet_logged_at = Some(Instant::now());
                            quiet_run = 0;
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });

    StoreWatcher {
        stop,
        wake,
        #[cfg(test)]
        attached,
    }
}

/// One watcher per store, each tagging its events with the directory they came
/// from so only that store is re-read.
///
/// Every store gets the same stat-gated safety net `spawn` already gives the
/// selected store -- there is no separate "watcher only, no poll" mode to
/// build here, since the net now costs nothing until something actually
/// changes.
///
/// The returned guards must be kept alive for the whole run; dropping one stops
/// its notifications.
pub fn spawn_many(dirs: &[String], out: Sender<String>) -> Vec<StoreWatcher> {
    dirs.iter()
        .map(|dir| {
            let (tx, rx) = channel::<()>();
            let guard = spawn(dir, tx);
            let out = out.clone();
            let dir = dir.clone();
            std::thread::spawn(move || {
                while rx.recv().is_ok() {
                    if out.send(dir.clone()).is_err() {
                        return;
                    }
                }
            });
            guard
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Touches the real filesystem, so waits are deliberately generous.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!("dextui-watch-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }

        fn write(&self, contents: &str) {
            fs::write(self.0.join("tasks.jsonl"), contents).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Waits out the events the platform replays for activity that happened
    /// *before* the watcher was attached, and reports how many it swallowed.
    ///
    /// macOS FSEvents delivers events for filesystem activity that predates the
    /// stream, **including the creation of the watched directory itself**. So
    /// "I attached a watcher, therefore the next event I see is one I caused"
    /// is not true, and two tests here were written as though it were. Both
    /// failed deterministically on GitHub's macos-14 runners while passing on
    /// macOS 26 locally -- not an OS difference in the end, just a race this
    /// machine happened to win. Observed directly with a throwaway watcher that
    /// printed whole `notify::Event`s: a `Create(File)` and two `Modify`s for a
    /// write made 6ms *before* `watch()` returned, and a `Create(Folder)` for
    /// the watched directory.
    ///
    /// This does not paper over the race, it ends it: nothing is asserted until
    /// the channel has been quiet for `quiet`, so every later event is one the
    /// test itself caused. Condition-based rather than a fixed sleep, so a slow
    /// machine waits longer instead of flaking.
    ///
    /// `cap` bounds the total wait. Without it a genuinely broken gate -- one
    /// emitting on every tick -- would keep this receiving forever and the test
    /// would hang rather than fail, which is the worse of the two.
    fn settle<T>(rx: &Receiver<T>, quiet: Duration, cap: Duration) -> usize {
        let deadline = Instant::now() + cap;
        let mut swallowed = 0;
        while Instant::now() < deadline && rx.recv_timeout(quiet).is_ok() {
            swallowed += 1;
        }
        swallowed
    }

    /// The quiet window `settle` needs, which is **not** a free choice.
    ///
    /// What is being drained is `out`, and the `Ok(())` branch does not forward
    /// an event as it arrives -- it swallows the rest of the burst for
    /// `DEBOUNCE` first, so a raw event at 5ms becomes an `out` send at 255ms.
    /// A quiet window shorter than `DEBOUNCE` therefore returns *during* the
    /// debounce, having seen nothing, and hands the still-pending emission to
    /// whatever the test asserts next.
    ///
    /// This was not theoretical: at 200ms the safety-timeout test still failed
    /// 3 runs in 15, while the multi-store test above passed 15/15 at 300ms.
    /// The margin is over `DEBOUNCE` rather than over a measured latency,
    /// because it is the debounce that sets the floor.
    const SETTLE_QUIET: Duration = DEBOUNCE.saturating_add(Duration::from_millis(150));

    #[test]
    fn fires_when_the_store_file_changes() {
        let dir = TempDir::new("fires");
        let (tx, rx) = channel();
        let _w = spawn(dir.path(), tx);

        dir.write(r#"{"id":"a"}"#);

        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "watcher did not fire on a file write"
        );
    }

    #[test]
    fn collapses_a_burst_of_writes() {
        let dir = TempDir::new("burst");
        let (tx, rx) = channel();
        let _w = spawn(dir.path(), tx);

        // A single dex write touches the file several times; without debouncing
        // this would cost one `dex list` per event.
        for i in 0..10 {
            dir.write(&format!(r#"{{"id":"a","n":{i}}}"#));
            thread::sleep(Duration::from_millis(20));
        }

        let seen = Arc::new(AtomicUsize::new(0));
        let deadline = std::time::Instant::now() + Duration::from_millis(1500);
        while std::time::Instant::now() < deadline {
            if rx.recv_timeout(Duration::from_millis(200)).is_ok() {
                seen.fetch_add(1, Ordering::SeqCst);
            }
        }

        let n = seen.load(Ordering::SeqCst);
        assert!((1..=3).contains(&n), "expected a coalesced burst, got {n}");
    }

    /// The multi-store watcher has to say *which* store changed, or the caller must
    /// re-read all of them and watching separately buys nothing.
    #[test]
    fn a_change_reports_which_store_it_came_from() {
        // A `TempDir` per run, like every other test here. This used to be a
        // fixed `temp_dir().join("dextui-watch-many")` that nothing ever
        // cleaned up, which is precisely what hid the bug below: after the
        // first run `a` and `b` already existed, so `create_dir_all` did
        // nothing and there was no directory-creation event to replay. It
        // passed 10/10 locally against a directory dated nine days earlier,
        // and failed on every fresh CI machine. Deleting that stale directory
        // reproduced the failure here immediately.
        let parent = TempDir::new("many");
        let a = parent.0.join("a");
        let b = parent.0.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let _guards = spawn_many(
            &[
                a.to_string_lossy().into_owned(),
                b.to_string_lossy().into_owned(),
            ],
            tx,
        );

        // Creating `a` and `b` is itself filesystem activity, and it happened
        // before the watchers existed -- so without this, `a` reports its own
        // creation and wins the race against the write below. That is the
        // literal CI failure: "reported the wrong store: .../a".
        settle(&rx, SETTLE_QUIET, Duration::from_secs(3));

        fs::write(b.join("tasks.jsonl"), "{}").unwrap();

        // `settle` alone is not enough to assert on the *first* thing that
        // arrives, because it cannot distinguish a replay that has already
        // been and gone from one that has not arrived yet -- so a late one
        // would land here and read as a mis-tag. Nothing ever writes to `a`,
        // so any report of it is that replay; skipping those costs nothing.
        // The claim under test is that the store which *did* change is the one
        // named, and a genuine mis-tagging bug still fails, by never producing
        // `b` at all before the deadline.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut also_seen: Vec<String> = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(store) if store.ends_with("/b") => break,
                Ok(store) => also_seen.push(store),
                Err(_) => panic!("never named the store that changed; saw {also_seen:?}"),
            }
        }
    }

    #[test]
    fn falls_back_to_the_safety_poll_when_the_store_is_missing() {
        // A brand-new project has no store directory until the first task exists.
        let dir = TempDir::new("missing");
        let missing = dir.0.join("not-created-yet");
        let (tx, rx) = channel();
        let _w = spawn(missing.to_str().unwrap(), tx);

        // The safety interval is 10s, so this proves the fallback path exists
        // without waiting for it: no watcher was created, and nothing panicked.
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    }

    // --- `stat` itself, driven directly rather than through a 10s timeout ---

    #[test]
    fn stat_of_a_missing_store_is_none() {
        let dir = TempDir::new("stat-missing");
        assert_eq!(stat(dir.path()), None);
    }

    #[test]
    fn stat_appears_once_the_file_does() {
        let dir = TempDir::new("stat-appears");
        assert_eq!(stat(dir.path()), None, "nothing written yet");
        dir.write(r#"{"id":"a"}"#);
        assert!(stat(dir.path()).is_some(), "the file now exists");
    }

    #[test]
    fn stat_is_stable_when_nothing_touches_the_file() {
        let dir = TempDir::new("stat-stable");
        dir.write(r#"{"id":"a"}"#);
        let first = stat(dir.path());
        let second = stat(dir.path());
        assert_eq!(first, second, "two reads with no write between them must agree");
    }

    #[test]
    fn stat_changes_when_the_file_does() {
        let dir = TempDir::new("stat-changes");
        dir.write(r#"{"id":"a"}"#);
        let before = stat(dir.path());
        dir.write(r#"{"id":"a","extra":"field makes this a different length"}"#);
        let after = stat(dir.path());
        assert_ne!(before, after, "a real write must change the fingerprint");
    }

    // --- the safety branch itself, via `spawn_inner` with a short interval so
    // these do not have to wait out the real 10s `SAFETY` constant ---

    const FAST: Duration = Duration::from_millis(60);

    #[test]
    fn a_safety_timeout_with_nothing_changed_emits_nothing() {
        let dir = TempDir::new("net-quiet");
        let (tx, rx) = channel();
        let _w = spawn_inner(dir.path(), tx, FAST);

        // The write is deliberately *after* the watcher attaches, and is then
        // waited for. That ordering is the whole trick, and it replaced a
        // `settle` that could not work: draining until the channel goes quiet
        // cannot tell "the replayed events have been and gone" apart from
        // "they have not arrived yet", so on a loaded runner it returned early
        // and handed the replay to the assertion below. Three green CI runs
        // and then a red one is what that looks like.
        //
        // Events arrive in order, so receiving the one for a write made after
        // `watch()` proves everything the platform meant to replay from before
        // it has already been delivered. A barrier, rather than a guess about
        // how long a replay takes -- and it also kills the false pass where
        // nothing works at all and the store stays quiet for that reason.
        dir.write(r#"{"id":"a"}"#);
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the watcher must report a write made after it attached");

        // The rest of that write's burst, which `spawn_inner` reports as one
        // event per debounce window rather than one per raw event.
        settle(&rx, SETTLE_QUIET, Duration::from_secs(3));

        // Several safety intervals now pass with the file left exactly alone.
        // This is the whole point of the stat gate: previously every one of
        // these ticks would have emitted, and the caller would have paid for
        // a `dex list` to learn nothing changed.
        assert!(
            rx.recv_timeout(Duration::from_millis(400)).is_err(),
            "an untouched store must not emit on the safety timeout"
        );
    }

    #[test]
    fn a_change_no_notify_watcher_could_see_is_still_caught_by_the_timeout() {
        // No notify watcher exists here at all: `spawn_inner` finds no
        // directory yet and creates none, exactly like
        // `falls_back_to_the_safety_poll_when_the_store_is_missing`. That is
        // the sharpest available stand-in for "the watcher missed it" --
        // nothing on the raw channel can ever fire, so anything reported can
        // only have come from the stat-gated timeout branch, never from `Ok(())`.
        let dir = TempDir::new("net-catches-misses");
        let store = dir.0.join("appears-later");
        let (tx, rx) = channel();
        let _w = spawn_inner(store.to_str().unwrap(), tx, FAST);

        assert!(
            rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "nothing to report before the store exists"
        );

        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("tasks.jsonl"), r#"{"id":"a"}"#).unwrap();

        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_ok(),
            "a change no notify watcher could see was not caught by the safety timeout"
        );
    }

    /// The bug this exists for: whether a notify watcher could be attached was
    /// decided once, at spawn. A store created after launch was therefore
    /// found by the poll -- correctly, which is what made it hard to see -- and
    /// then stayed on the poll's interval for the whole life of the process,
    /// with nothing ever attaching to it.
    ///
    /// Asserted on `is_attached` rather than on timing, because both states
    /// keep the view *correct* -- the poll finds everything eventually, which
    /// is precisely why being stuck on it went unnoticed. Only the latency
    /// differs, and a test that measured latency would be a test that measured
    /// the machine.
    #[test]
    fn a_store_created_after_launch_gets_a_real_watcher_not_just_the_poll() {
        let dir = TempDir::new("late-attach");
        let store = dir.0.join("appears-later");
        let (tx, rx) = channel();
        let w = spawn_inner(store.to_str().unwrap(), tx, FAST);

        assert!(!w.is_attached(), "nothing to attach to yet");

        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("tasks.jsonl"), r#"{"id":"a"}"#).unwrap();

        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "the poll never found a store that appeared after launch"
        );
        assert!(
            w.is_attached(),
            "the store was found but never watched -- it stays on the poll's \
             interval for the life of the process"
        );
    }

    /// The stat baseline has to be taken before `spawn` returns, not on the
    /// thread. Taken on the thread it is read at some unknowable moment after
    /// the caller is already running, so a write in that window lands in the
    /// baseline and every later tick then *correctly* reports it as unchanged:
    /// the change is not late, it is gone. With no watcher attached -- a store
    /// that does not exist yet, which is exactly when it matters -- nothing
    /// else will ever report it, and the pane stays empty indefinitely.
    ///
    /// Deliberately no sleep before the write: the window is the point.
    #[test]
    fn a_write_immediately_after_spawn_is_not_swallowed_by_the_baseline() {
        let dir = TempDir::new("baseline-race");
        let store = dir.0.join("appears-immediately");
        let (tx, rx) = channel();
        let _w = spawn_inner(store.to_str().unwrap(), tx, FAST);

        fs::create_dir_all(&store).unwrap();
        fs::write(store.join("tasks.jsonl"), r#"{"id":"a"}"#).unwrap();

        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "a store that appeared in the gap after spawn was never reported"
        );
    }

    /// The reverse of the case above, and the reason the check is a tick-by-tick
    /// question rather than a one-off retry that stops once it succeeds.
    #[test]
    fn a_store_deleted_and_recreated_is_watched_again() {
        let dir = TempDir::new("re-attach");
        let store = dir.0.join("goes-away");
        fs::create_dir_all(&store).unwrap();

        let (tx, _rx) = channel();
        let w = spawn_inner(store.to_str().unwrap(), tx, FAST);
        assert!(w.is_attached(), "the store existed at spawn");

        fs::remove_dir_all(&store).unwrap();
        // Generous: removing and recreating a directory produces a burst of
        // notify events, and each one costs a DEBOUNCE before the loop reaches
        // the timeout branch where attaching is decided.
        thread::sleep(Duration::from_millis(900));
        assert!(!w.is_attached(), "still claims to watch a store that is gone");

        fs::create_dir_all(&store).unwrap();
        // Generous: removing and recreating a directory produces a burst of
        // notify events, and each one costs a DEBOUNCE before the loop reaches
        // the timeout branch where attaching is decided.
        thread::sleep(Duration::from_millis(900));
        assert!(w.is_attached(), "never re-attached to the recreated store");
    }

    /// Dropping the guard used to drop the notify watcher but leave the thread
    /// polling the store forever, still able to trigger a refresh -- and
    /// `switch_store` drops one on every worktree change.
    #[test]
    fn dropping_the_guard_stops_the_thread_promptly() {
        let dir = TempDir::new("guard-stops");
        dir.write(r#"{"id":"a"}"#);
        let (tx, rx) = channel();
        let w = spawn_inner(dir.path(), tx, FAST);

        drop(w);
        thread::sleep(Duration::from_millis(200));

        dir.write(r#"{"id":"a","changed":true}"#);
        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "a dropped watcher is still reporting changes"
        );
    }

    // --- `quiet_tick_message`, driven directly with fabricated durations
    // rather than through a real clock ---

    #[test]
    fn the_first_quiet_tick_after_activity_always_logs_in_full() {
        assert_eq!(
            quiet_tick_message("/x", 1, None),
            Some("tick /x unchanged".to_string())
        );
    }

    #[test]
    fn a_quiet_tick_stays_silent_before_the_window_elapses() {
        assert_eq!(
            quiet_tick_message("/x", 5, Some(Duration::from_secs(30))),
            None,
            "logged again before a minute of quiet had passed"
        );
    }

    #[test]
    fn a_quiet_tick_logs_once_the_window_elapses_and_reports_the_run() {
        assert_eq!(
            quiet_tick_message("/x", 60, Some(QUIET_LOG)),
            Some("tick /x unchanged (x60)".to_string()),
            "the run should be reported once the window is up"
        );
    }

    /// The one-tick-in-a-slow-window case: enough time passed to warrant
    /// logging, but only a single quiet tick happened in it. It reads exactly
    /// like the first-ever quiet tick, not `(x1)`, which would just be noise.
    #[test]
    fn a_lone_quiet_tick_after_a_long_gap_has_no_count_suffix() {
        assert_eq!(
            quiet_tick_message("/x", 1, Some(QUIET_LOG)),
            Some("tick /x unchanged".to_string())
        );
    }

    // No test drives this through the real loop and reads the log file back,
    // the way the other sections here drive `spawn_inner` and read `rx` back.
    // `log::LOG_PATH` is a process-wide `OnceLock`: once one test resolves it,
    // it is resolved for the rest of the binary, including every other test
    // that logs anything at all -- there is no way to give this one test its
    // own log file without corrupting whichever other test happens to run
    // concurrently and share it. `log.rs`'s own tests avoid this the same
    // way, by calling `write_line` directly rather than going through `init`.
    // The wiring that reads `quiet_run`/`quiet_logged_at` and calls this
    // function is small enough to verify by inspection instead.

    #[test]
    fn a_missing_tasks_file_emits_nothing_until_it_appears() {
        // Unlike the previous test, the store *directory* exists here, so a
        // real notify watcher does attach -- this is the ordinary shape of a
        // freshly-registered repo whose worktree has no tasks yet.
        let dir = TempDir::new("net-file-appears");
        let (tx, rx) = channel();
        let _w = spawn_inner(dir.path(), tx, FAST);

        assert!(
            rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "no tasks.jsonl yet, so there is nothing to report"
        );

        dir.write(r#"{"id":"a"}"#);

        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_ok(),
            "the file's appearance must be reported"
        );
    }
}
