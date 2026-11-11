//! Pure logic: coordinate conversion and echo suppression.

use oriel_core::{Rect, WindowId};
use std::collections::HashMap;

/// Convert an AppKit rect (origin bottom-left of the primary display) to the contract's space
/// (origin top-left of the primary display, y down).
pub fn appkit_to_top_left(x: f64, y: f64, w: f64, h: f64, primary_height: f64) -> Rect {
    Rect::new(x, primary_height - (y + h), w, h)
}

/// What we know about one window's frame.
struct Known {
    /// The last frame we asked for (or the last frame the user gave it).
    requested: Rect,
    /// The last frame read back from the window.
    actual: Rect,
}

/// Remembers the last frames known per window, to tell our own echoes from user changes.
#[derive(Default)]
pub struct FrameTracker {
    known: HashMap<WindowId, Known>,
}

impl FrameTracker {
    /// We set this frame ourselves. Until a read-back, the window is taken to be there.
    pub fn note_set(&mut self, id: WindowId, frame: Rect) {
        self.known.insert(id, Known { requested: frame, actual: frame });
    }

    /// The window was read back after our own set. True when the app refused: the real frame
    /// is not `close_to` the one we asked for, and the model must hear about it
    /// (`by_user: false`).
    pub fn note_actual(&mut self, id: WindowId, real: Rect) -> bool {
        let Some(k) = self.known.get_mut(&id) else { return false };
        k.actual = real;
        !real.close_to(&k.requested)
    }

    /// A moved/resized notification read `real`. True when the model must hear about it as a
    /// change by the user. A frame `close_to` the last requested or the last read-back frame
    /// is an echo (after a refused set, the app's own adjust notes arrive here). The reported
    /// frame becomes the new known frame.
    pub fn should_report(&mut self, id: WindowId, real: Rect) -> bool {
        match self.known.get(&id) {
            Some(k) if k.requested.close_to(&real) || k.actual.close_to(&real) => false,
            _ => {
                self.known.insert(id, Known { requested: real, actual: real });
                true
            }
        }
    }

    pub fn forget(&mut self, id: WindowId) {
        self.known.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_appkit_rect() {
        // A 1440x900 primary display, menu bar 25 pt: visibleFrame y=0 (dock hidden), h=875.
        let r = appkit_to_top_left(0.0, 0.0, 1440.0, 875.0, 900.0);
        assert_eq!(r, Rect::new(0.0, 25.0, 1440.0, 875.0));
    }

    #[test]
    fn converts_secondary_display_above() {
        // A display above the primary has AppKit y > primary height, so y_top is negative.
        let r = appkit_to_top_left(0.0, 900.0, 1000.0, 800.0, 900.0);
        assert_eq!(r.y, -800.0);
    }

    #[test]
    fn echo_is_suppressed() {
        let mut t = FrameTracker::default();
        let id = WindowId(7);
        t.note_set(id, Rect::new(10.0, 10.0, 500.0, 400.0));
        assert!(!t.should_report(id, Rect::new(10.4, 9.8, 500.5, 400.0)));
        assert!(t.should_report(id, Rect::new(30.0, 10.0, 500.0, 400.0)));
        // The reported frame is now the known one.
        assert!(!t.should_report(id, Rect::new(30.0, 10.0, 500.0, 400.0)));
        // Unknown windows always report.
        assert!(t.should_report(WindowId(8), Rect::new(0.0, 0.0, 1.0, 1.0)));
    }

    #[test]
    fn refusal_is_reported_once_and_its_notes_are_echoes() {
        let mut t = FrameTracker::default();
        let id = WindowId(3);
        t.note_set(id, Rect::new(0.0, 65.0, 1440.0, 835.0));
        let real = Rect::new(0.0, 65.0, 230.0, 408.0);
        assert!(t.note_actual(id, real));
        // The app's own adjust notifications read the actual frame: not a user move.
        assert!(!t.should_report(id, real));
        assert!(!t.should_report(id, Rect::new(0.3, 65.0, 230.0, 408.0)));
        // A real move is reported, and a repeat of it is not.
        let moved = Rect::new(100.0, 65.0, 230.0, 408.0);
        assert!(t.should_report(id, moved));
        assert!(!t.should_report(id, moved));
        // The follow-up set to the fit that the app takes is no refusal.
        let fit = Rect::new(605.0, 65.0, 230.0, 408.0);
        t.note_set(id, fit);
        assert!(!t.note_actual(id, fit));
        assert!(!t.should_report(id, fit));
    }
}
