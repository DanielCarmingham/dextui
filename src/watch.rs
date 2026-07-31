//! Signals that the dex store changed.
//!
//! Reports only *that* something changed, never *what*: the caller re-reads via
//! `dex list --json`. That keeps us off dex's private on-disk format while
//! costing nothing at all when the store is idle.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub const DEBOUNCE: Duration = Duration::from_millis(250);
pub const SAFETY: Duration = Duration::from_secs(10);

/// Held only to keep the watcher alive; dropping it stops the notifications.
pub struct StoreWatcher {
    _watcher: Option<RecommendedWatcher>,
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
    Some((mtime, meta.len(), meta.ino()))
}

/// Starts watching `store_dir`, sending `()` on `out` whenever it changes.
///
/// A brand-new project has no store directory until the first task is created,
/// and watching a missing path fails, so in that case we fall back to the safety
/// net alone, which picks the store up once it appears.
pub fn spawn(store_dir: &str, out: Sender<()>) -> StoreWatcher {
    spawn_inner(store_dir, out, SAFETY)
}

/// The real implementation, taking the safety interval as a parameter so
/// tests can drive it in milliseconds rather than waiting out the real 10s
/// constant. `spawn` is the only public entry point; this stays private.
fn spawn_inner(store_dir: &str, out: Sender<()>, safety: Duration) -> StoreWatcher {
    let (raw_tx, raw_rx): (Sender<()>, Receiver<()>) = channel();

    let watcher = if Path::new(store_dir).is_dir() {
        let tx = raw_tx.clone();
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                let _ = tx.send(());
            }
        }) {
            Ok(mut w) => match w.watch(Path::new(store_dir), RecursiveMode::NonRecursive) {
                Ok(()) => Some(w),
                Err(_) => None,
            },
            Err(_) => None,
        }
    } else {
        None
    };

    let dir = store_dir.to_string();
    thread::spawn(move || {
        // Keeps `raw_rx` connected for the life of this thread even when no
        // real notify watcher exists to hold a clone of its own -- e.g. a
        // worktree with no store yet. Without this, `raw_tx` (never cloned
        // in that case) drops the instant `spawn_inner` returns, `raw_rx`
        // reads as disconnected on the very first tick, and the thread exits
        // before ever reaching the safety branch below -- silently defeating
        // the exact case ("a brand-new project has no store directory until
        // the first task is created") that branch exists to cover.
        let _raw_tx = raw_tx;

        // The baseline every safety tick compares against. Read once up
        // front so a store that already existed when `spawn` was called
        // does not look like a change on the very first tick.
        let mut last = stat(&dir);

        loop {
            match raw_rx.recv_timeout(safety) {
                Ok(()) => {
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
                    let now = stat(&dir);
                    if now != last {
                        last = now;
                        if out.send(()).is_err() {
                            return;
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });

    StoreWatcher { _watcher: watcher }
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
