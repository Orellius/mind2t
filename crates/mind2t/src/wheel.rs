//! Purpose: turn macOS scroll deltas into whole terminal rows without losing the fractions.
//! Public surface: `Accumulator`.
//! Why this file: a terminal scrolls in ROWS and a trackpad reports in POINTS, and the naive
//!   conversion - round each delta and pass it on - is silently broken for the input people
//!   actually use. A slow two-finger drag delivers a stream of deltas well under one row each;
//!   rounded individually every one of them is zero, so the viewport does not move at all while
//!   the gesture is plainly happening. Nothing errors and a fast flick still works, which is
//!   what makes it a bug that ships.
//! NOT responsible for: reading events (the host's AppKit monitor does), or mouse REPORTING -
//!   a program that asked to receive wheel events itself is a separate path and is wired
//!   neither here nor in the probe.
//! Test strategy: pure, so the fractional case is a unit test rather than a gesture. What no
//!   test here can settle is the SIGN - whether a finger moving one way climbs into history -
//!   because that is a fact about AppKit's convention plus the operator's "natural scrolling"
//!   setting. It is a live-tap item and a one-line flip if it is backwards.

/// Converts scroll deltas to rows, carrying the remainder between events.
///
/// One per window. Kept across events on purpose: the remainder IS the state, and an
/// accumulator rebuilt per event is the rounding bug this type exists to prevent.
#[derive(Debug, Default)]
pub struct Accumulator {
    /// Rows earned but not yet delivered, always strictly between -1 and 1.
    residual: f64,
}

impl Accumulator {
    /// Whole rows to scroll for this event; positive climbs into history.
    ///
    /// `delta` is `NSEvent::scrollingDeltaY`, whose UNIT depends on the device and is reported
    /// by `hasPreciseScrollingDeltas`: a detented wheel already counts in lines, while a
    /// trackpad counts in points. Treating the two alike is a factor-of-twenty error in one
    /// direction or the other.
    ///
    /// `cell_height` is in DEVICE pixels, because everything this renderer touches is
    /// (project law), while the delta is in points - so the scale factor is not decoration
    /// here, it is the only thing that makes the two comparable.
    pub fn rows(&mut self, delta: f64, precise: bool, cell_height: u32, scale: f64) -> i32 {
        let rows = if precise {
            // A cell can be zero pixels tall only if the metrics are broken; dividing by it
            // would poison the residual with an infinity that never clears.
            let points_per_row = f64::from(cell_height.max(1)) / scale.max(f64::MIN_POSITIVE);
            delta / points_per_row
        } else {
            delta
        };
        if !rows.is_finite() {
            return 0;
        }

        self.residual += rows;
        let whole = self.residual.trunc();
        self.residual -= whole;
        whole as i32
    }
}

#[cfg(test)]
mod tests {
    use super::Accumulator;

    /// A 36px cell on a 2x display is 18 points tall, which is what the trackpad cases divide by.
    const CELL: u32 = 36;
    const SCALE: f64 = 2.0;

    #[test]
    fn a_wheel_detent_is_already_a_line() {
        let mut wheel = Accumulator::default();
        assert_eq!(wheel.rows(1.0, false, CELL, SCALE), 1);
        assert_eq!(wheel.rows(-3.0, false, CELL, SCALE), -3);
    }

    /// The case the naive version drops entirely: nudges smaller than a row.
    #[test]
    fn nudges_under_a_row_accumulate_instead_of_vanishing() {
        let mut wheel = Accumulator::default();
        // Six points is a third of a row here.
        let delivered: i32 = (0..9).map(|_| wheel.rows(6.0, true, CELL, SCALE)).sum();
        assert_eq!(delivered, 3, "nine third-row nudges must move three rows");
    }

    /// The first nudges deliver NOTHING, which is what the sum above cannot show on its own -
    /// an implementation that rounded each nudge UP to a row would also total three.
    #[test]
    fn a_nudge_under_a_row_delivers_nothing_yet() {
        let mut wheel = Accumulator::default();
        assert_eq!(wheel.rows(6.0, true, CELL, SCALE), 0);
        assert_eq!(wheel.rows(6.0, true, CELL, SCALE), 0);
        assert_eq!(wheel.rows(6.0, true, CELL, SCALE), 1);
    }

    /// A reversal must not leave a debt behind: the residual is a fraction of the CURRENT
    /// gesture, and one that survived a direction change would make the next scroll overshoot.
    #[test]
    fn a_reversal_cancels_rather_than_accumulating() {
        let mut wheel = Accumulator::default();
        assert_eq!(wheel.rows(9.0, true, CELL, SCALE), 0);
        assert_eq!(wheel.rows(-9.0, true, CELL, SCALE), 0);
        assert_eq!(wheel.rows(18.0, true, CELL, SCALE), 1, "the residual leaked");
    }

    #[test]
    fn broken_metrics_scroll_nothing_rather_than_diverging() {
        let mut wheel = Accumulator::default();
        assert_eq!(wheel.rows(10.0, true, 0, 0.0), 0);
        // And the accumulator still works afterwards, which it would not if an infinity or a
        // NaN had been folded into the residual.
        assert_eq!(wheel.rows(2.0, false, CELL, SCALE), 2);
    }
}
