//! The animation clock: pure arithmetic over elapsed time, and nothing else.
//!
//! In-progress markers turn, one glyph per [`FRAME`]. The frames themselves live
//! in `icons`, per tier; this module only answers *which* one, so the tier and
//! the schedule stay independent.
//!
//! **This module exists to make the idle-cost guarantee testable rather than a
//! comment.** No `App`, no clock reads, so every rule below is an assertion.
//!
//! This replaced a two-frame colour breath on a 700ms half-period, and it is
//! materially more expensive: 80ms frames are ~12.5 repaints/sec against ~1.4,
//! roughly nine times the work. Two things make that acceptable, and both are
//! pinned by tests rather than asserted here. It is paid **only while a task is
//! running** -- [`poll_timeout`] returns the untouched [`IDLE_POLL`] otherwise,
//! on the same code path, so an idle store costs exactly what it did before
//! animation existed. And it is opt-out: `animate = false` reaches this
//! function, not merely the drawing, so switching it off restores the old
//! wakeup schedule rather than leaving a fast loop drawing a still glyph.
//!
//! Unlike the colour pulse, whose clamp mattered only in the last 100ms before a
//! flip, this one is load-bearing on every iteration: 80ms is shorter than the
//! 100ms poll, so the clamp is what sets the pace.

use std::time::Duration;

/// The event loop's timeout when nothing is animating -- unchanged from before
/// animation existed. It bounds how long a store change waits to be noticed; it
/// is not a frame rate.
pub const IDLE_POLL: Duration = Duration::from_millis(100);

/// How long each frame is held.
///
/// 80ms is what `cli-spinners` uses for the braille "dots" set, and it is the
/// slowest rate that still reads as rotation rather than as a sequence of
/// separate glyphs. It is well under [`IDLE_POLL`], so unlike the colour pulse
/// this genuinely drives the loop rather than merely trimming its last wake.
pub const FRAME: Duration = Duration::from_millis(80);

/// Which frame of an `n`-frame cycle `elapsed` falls in.
///
/// `n == 0` would divide by zero and `n == 1` is a tier with nothing to
/// animate; both answer 0, so callers need no special case.
pub fn frame(elapsed: Duration, n: usize) -> usize {
    if n < 2 {
        return 0;
    }
    (elapsed.as_millis() / FRAME.as_millis()) as usize % n
}

/// How long until the frame changes. Always in `1..=FRAME`, so a clamped
/// timeout can never be zero and spin.
pub fn until_next_frame(elapsed: Duration) -> Duration {
    let into_window = (elapsed.as_millis() % FRAME.as_millis()) as u64;
    FRAME - Duration::from_millis(into_window)
}

/// The idle-cost guarantee, in one function.
///
/// `animating` is the caller's answer to "is at least one task in progress, and
/// is animation switched on" -- so turning the pulse off restores the previous
/// wakeup schedule exactly, rather than merely freezing the colour.
pub fn poll_timeout(animating: bool, elapsed: Duration) -> Duration {
    if animating {
        IDLE_POLL.min(until_next_frame(elapsed))
    } else {
        IDLE_POLL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A frame holds for its whole window. Anything changing within one would
    /// read as flicker rather than rotation.
    #[test]
    fn a_frame_is_stable_within_its_window() {
        for e in [0, 1, 40, 79] {
            assert_eq!(frame(ms(e), 10), 0, "{e}ms left the first frame");
        }
        for e in [80, 100, 159] {
            assert_eq!(frame(ms(e), 10), 1, "{e}ms left the second frame");
        }
    }

    #[test]
    fn the_frame_advances_across_a_window_boundary() {
        assert_ne!(frame(ms(79), 10), frame(ms(80), 10));
        assert_ne!(frame(ms(159), 10), frame(ms(160), 10));
    }

    /// The cycle closes: frame 0 comes back after exactly n frames, so the
    /// spinner cannot drift or stutter at the wrap.
    #[test]
    fn the_cycle_returns_to_its_first_frame() {
        for n in [4usize, 6, 10] {
            let full = 80 * n as u64;
            assert_eq!(frame(ms(full), n), 0, "{n} frames did not wrap to 0");
            assert_eq!(frame(ms(full + 80), n), 1, "{n} frames drifted after wrap");
            // Every frame is visited, none skipped.
            let seen: Vec<usize> = (0..n).map(|i| frame(ms(i as u64 * 80), n)).collect();
            assert_eq!(seen, (0..n).collect::<Vec<_>>(), "{n} frames out of order");
        }
    }

    /// A tier with a single glyph has nothing to animate, and one with none
    /// would divide by zero. Both must answer 0 rather than needing a caller
    /// to special-case them.
    #[test]
    fn a_tier_with_nothing_to_animate_stays_on_its_only_frame() {
        for e in [0u64, 80, 1000, 99_999] {
            assert_eq!(frame(ms(e), 1), 0, "{e}ms moved a one-frame tier");
            assert_eq!(frame(ms(e), 0), 0, "{e}ms divided by zero");
        }
    }

    /// THE idle-cost guard, and the part most likely to rot. With no animation
    /// the loop must wake exactly as often as it did before animation existed --
    /// not once more.
    #[test]
    fn the_poll_timeout_is_untouched_when_nothing_is_in_progress() {
        for e in [0, 1, 350, 699, 700, 1399, 1400, 9999] {
            assert_eq!(
                poll_timeout(false, ms(e)),
                IDLE_POLL,
                "{e}ms changed the idle timeout"
            );
        }
    }

    /// Unlike the colour pulse this clamp is load-bearing on every iteration:
    /// FRAME is 80ms against a 100ms poll, so it always wins. Exact equalities,
    /// not inequalities -- an inequality would pass with the clamp deleted.
    #[test]
    fn the_poll_timeout_is_clamped_to_the_next_frame_while_something_is_in_progress() {
        assert_eq!(poll_timeout(true, ms(0)), ms(80), "did not wake for the frame");
        assert_eq!(poll_timeout(true, ms(60)), ms(20), "woke late for the frame");
        assert_eq!(poll_timeout(true, ms(1340)), ms(20), "clamp stopped working");

        // It may shorten the wait but must never lengthen it, or a store change
        // would be noticed more slowly than before.
        for e in [0, 100, 350, 600, 700, 1234] {
            assert!(
                poll_timeout(true, ms(e)) <= IDLE_POLL,
                "{e}ms lengthened the timeout"
            );
        }
    }

    /// A zero timeout turns `event::poll` into a busy loop, which would burn a
    /// core for the sake of a colour.
    #[test]
    fn animation_never_makes_the_loop_spin() {
        for e in 0..1400u64 {
            assert!(poll_timeout(true, ms(e)) > Duration::ZERO, "{e}ms");
            let next = until_next_frame(ms(e));
            assert!(next > Duration::ZERO && next <= FRAME, "{e}ms -> {next:?}");
        }
    }
}
