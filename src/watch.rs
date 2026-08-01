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
use std::time::{Duration, SystemTime};

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
                        if out.send(()).is_err() {
                            return;
                        }
                    } else {
                        log::line("watch", &format!("tick {dir} unchanged"));
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
        let dir = std::env::temp_dir().join("dextui-watch-many");
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let _guards = spawn_many(
            &[
                a.to_string_lossy().into_owned(),
                b.to_string_lossy().into_owned(),
            ],
            tx,
        );

        std::fs::write(b.join("tasks.jsonl"), "{}").unwrap();

        let got = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(got.ends_with("/b"), "reported the wrong store: {got}");
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
        dir.write(r#"{"id":"a"}"#);
        let (tx, rx) = channel();
        let _w = spawn_inner(dir.path(), tx, FAST);

        // Several safety intervals pass with the file left exactly alone.
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
