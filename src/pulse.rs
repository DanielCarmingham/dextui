//! The animation clock: pure arithmetic over elapsed time, and nothing else.
//!
//! Two frames, not a ramp. In-progress rows alternate between [`crate::theme::ACTIVE`]
//! and a bold bright variant every [`HALF_PERIOD`]; the glyph never changes shape.
//!
//! **This module exists to make the idle-cost guarantee testable rather than a
//! comment.** No `App`, no clock reads, so every rule below is an assertion.
//!
//! The non-obvious part: the event loop already wakes every [`IDLE_POLL`], which
//! is far shorter than the half-period. The clamp in [`poll_timeout`] is
//! therefore *not* what makes animation possible -- it only matters in the final
//! [`IDLE_POLL`] before a flip, where it lands the wake **on** the boundary
//! rather than up to 100ms late (a 14% jitter on a 700ms breath). It never
//! lengthens the timeout, so a store change is noticed exactly as fast as
//! before. When nothing is in progress the function returns the literal old
//! constant: same code path, same wakeup count, zero extra redraws.

use std::time::Duration;

/// The event loop's timeout when nothing is animating -- unchanged from before
/// animation existed. It bounds how long a store change waits to be noticed; it
/// is not a frame rate.
pub const IDLE_POLL: Duration = Duration::from_millis(100);

/// How long each of the two frames lasts. A full breath is twice this.
pub const HALF_PERIOD: Duration = Duration::from_millis(700);

/// Which of the two frames `elapsed` falls in.
pub fn phase(elapsed: Duration) -> bool {
    (elapsed.as_millis() / HALF_PERIOD.as_millis()) % 2 == 1
}

/// How long until the frame changes. Always in `1..=HALF_PERIOD`, so a clamped
/// timeout can never be zero and spin.
pub fn until_next_flip(elapsed: Duration) -> Duration {
    let into_window = (elapsed.as_millis() % HALF_PERIOD.as_millis()) as u64;
    HALF_PERIOD - Duration::from_millis(into_window)
}

/// The idle-cost guarantee, in one function.
///
/// `animating` is the caller's answer to "is at least one task in progress, and
/// is animation switched on" -- so turning the pulse off restores the previous
/// wakeup schedule exactly, rather than merely freezing the colour.
pub fn poll_timeout(animating: bool, elapsed: Duration) -> Duration {
    if animating {
        IDLE_POLL.min(until_next_flip(elapsed))
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

    /// Two frames means the glyph holds still for a whole window. Anything that
    /// changed within one would read as flicker rather than a breath.
    #[test]
    fn the_phase_is_stable_within_a_window() {
        for e in [0, 1, 350, 699] {
            assert_eq!(phase(ms(e)), phase(ms(0)), "{e}ms left the first window");
        }
        for e in [700, 1000, 1399] {
            assert_eq!(phase(ms(e)), phase(ms(700)), "{e}ms left the second window");
        }
    }

    #[test]
    fn the_phase_flips_across_a_window_boundary() {
        assert_ne!(phase(ms(699)), phase(ms(700)));
        assert_ne!(phase(ms(1399)), phase(ms(1400)));
    }

    /// Guards against someone "improving" this into a four-frame ramp, which in
    /// a terminal reads as flicker rather than as motion.
    #[test]
    fn there_are_exactly_two_frames() {
        assert_eq!(phase(ms(0)), phase(ms(1400)), "a full breath must return");
        assert_eq!(phase(ms(350)), phase(ms(1750)));
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

    /// The clamp looks like dead code -- `IDLE_POLL` wins in ~86% of iterations
    /// -- so this pins the 14% where it does not. Do not weaken the exact
    /// equalities to an inequality: an inequality passes with the clamp deleted.
    #[test]
    fn the_poll_timeout_is_clamped_to_the_next_flip_while_something_is_in_progress() {
        assert_eq!(poll_timeout(true, ms(680)), ms(20), "woke late for the flip");
        assert_eq!(poll_timeout(true, ms(1380)), ms(20), "clamp stopped working");

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
            let next = until_next_flip(ms(e));
            assert!(next > Duration::ZERO && next <= HALF_PERIOD, "{e}ms -> {next:?}");
        }
    }
}
