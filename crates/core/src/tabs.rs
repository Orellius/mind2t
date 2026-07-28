//! Purpose: horizontal tab stops.
//! Public surface: `TabStops`.
//! Why this file: tab stops are per-terminal state with their own small grammar (HTS, TBC,
//!   CHT, CBT) and no relationship to the grid, so they do not belong in either.
//! NOT responsible for: moving the cursor. It answers "where is the next stop", and
//!   `terminal.rs` decides what to do with the answer.
//! Test strategy: unit tests below; the corpus checks the result against Ghostty.

/// Tab stops are every 8 columns until a program says otherwise.
const INTERVAL: u16 = 8;

/// The set of horizontal tab stops, one bit per column.
#[derive(Debug, Clone)]
pub struct TabStops {
    stops: Vec<bool>,
}

impl TabStops {
    /// Fresh stops at every multiple of 8, excluding column 0 -- a tab at the left margin
    /// must advance, so column 0 is never a stop.
    pub fn new(cols: u16) -> TabStops {
        let mut stops = vec![false; usize::from(cols)];
        let mut x = INTERVAL;
        while usize::from(x) < stops.len() {
            stops[usize::from(x)] = true;
            x += INTERVAL;
        }
        TabStops { stops }
    }

    pub fn set(&mut self, x: u16) {
        if let Some(stop) = self.stops.get_mut(usize::from(x)) {
            *stop = true;
        }
    }

    pub fn clear(&mut self, x: u16) {
        if let Some(stop) = self.stops.get_mut(usize::from(x)) {
            *stop = false;
        }
    }

    pub fn clear_all(&mut self) {
        self.stops.iter_mut().for_each(|stop| *stop = false);
    }

    /// The next stop strictly right of `x`, or the last column when none remains.
    ///
    /// Falling back to the last column rather than staying put is what stops a program that
    /// tabs in a loop from wedging the cursor.
    pub fn next(&self, x: u16) -> u16 {
        let last = self.last_col();
        for candidate in (x + 1)..=last {
            if self.stops.get(usize::from(candidate)).copied().unwrap_or(false) {
                return candidate;
            }
        }
        last
    }

    /// The previous stop strictly left of `x`, or column 0 when none remains.
    pub fn previous(&self, x: u16) -> u16 {
        let mut candidate = x;
        while candidate > 0 {
            candidate -= 1;
            if self.stops.get(usize::from(candidate)).copied().unwrap_or(false) {
                return candidate;
            }
        }
        0
    }

    fn last_col(&self) -> u16 {
        u16::try_from(self.stops.len())
            .unwrap_or(u16::MAX)
            .saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_stops_are_every_eight_columns() {
        let tabs = TabStops::new(40);
        assert_eq!(tabs.next(0), 8);
        assert_eq!(tabs.next(8), 16);
        assert_eq!(tabs.next(15), 16);
    }

    #[test]
    fn column_zero_is_never_a_stop_so_a_tab_always_advances() {
        let tabs = TabStops::new(40);
        assert_ne!(tabs.next(0), 0);
    }

    #[test]
    fn past_the_last_stop_the_cursor_parks_on_the_last_column() {
        let tabs = TabStops::new(20);
        assert_eq!(tabs.next(16), 19, "no stop at 24, so clamp to the last column");
        assert_eq!(tabs.next(19), 19, "and stay there rather than wedging");
    }

    #[test]
    fn a_custom_stop_is_honoured_and_clearable() {
        let mut tabs = TabStops::new(40);
        tabs.set(3);
        assert_eq!(tabs.next(0), 3);
        tabs.clear(3);
        assert_eq!(tabs.next(0), 8);
    }

    #[test]
    fn clearing_every_stop_leaves_only_the_last_column() {
        let mut tabs = TabStops::new(20);
        tabs.clear_all();
        assert_eq!(tabs.next(0), 19);
    }

    #[test]
    fn backward_tabs_walk_left_and_stop_at_zero() {
        let tabs = TabStops::new(40);
        assert_eq!(tabs.previous(20), 16);
        assert_eq!(tabs.previous(16), 8);
        assert_eq!(tabs.previous(8), 0);
        assert_eq!(tabs.previous(0), 0);
    }
}
