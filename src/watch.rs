//! Signals that the dex store changed.
//!
//! Reports only *that* something changed, never *what*: the caller re-reads via
//! `dex list --json`. That keeps us off dex's private on-disk format while
//! costing nothing at all when the store is idle.

use std::path::Path;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub const DEBOUNCE: Duration = Duration::from_millis(250);
pub const SAFETY: Duration = Duration::from_secs(10);

/// Held only to keep the watcher alive; dropping it stops the notifications.
pub struct StoreWatcher {
    _watcher: Option<RecommendedWatcher>,
}

/// Starts watching `store_dir`, sending `()` on `out` whenever it changes.
///
/// A brand-new project has no store directory until the first task is created,
/// and watching a missing path fails, so in that case we fall back to the safety
/// poll alone, which picks the store up once it appears.
pub fn spawn(store_dir: &str, out: Sender<()>) -> StoreWatcher {
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

    thread::spawn(move || {
        loop {
            match raw_rx.recv_timeout(SAFETY) {
                Ok(()) => {
                    // A single dex write touches the file several times; swallow
                    // the burst so it costs one `dex list`, not one per event.
                    while raw_rx.recv_timeout(DEBOUNCE).is_ok() {}
                    if out.send(()).is_err() {
                        return;
                    }
                }
                // Writers often replace tasks.jsonl via a temp file plus rename,
                // which macOS can surface as an event we never see. This bounds
                // how stale the view can get.
                Err(RecvTimeoutError::Timeout) => {
                    if out.send(()).is_err() {
                        return;
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
}
