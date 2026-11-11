//! State that the worker, the frame mover and the AX observer thread share.
//!
//! The worker owns app lifetime and window tracking; it mirrors each tracked window here so
//! the mover can set frames and the observer thread can filter echoes without a round trip
//! through the worker's queue (which can be busy with a slow app).

use crate::ax::Ax;
use crate::frame::FrameTracker;
use oriel_core::{Event, Rect, WindowId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// A tracked window, as the mover and the observer thread need it.
#[derive(Clone)]
pub struct WinRef {
    pub pid: i32,
    pub app: Ax,
    pub el: Ax,
}

#[derive(Default)]
pub struct Shared {
    pub wins: HashMap<WindowId, WinRef>,
    /// The echo filter: last requested and last read-back frame per window.
    pub frames: FrameTracker,
    /// The newest frame asked for each window (`SetFrame` or `Present`). The mover and
    /// `Present` always apply this one, so an older request never lands after a newer one.
    pub targets: HashMap<WindowId, Rect>,
    /// The size last applied to each window by Oriel. A `SetFrame` with this size only moves.
    pub applied: HashMap<WindowId, (f64, f64)>,
    /// Windows inside a `SetFrame` burst (from the first set to the settle read-back): their
    /// moved/resized notes are echoes and never reported.
    pub bursts: HashMap<WindowId, Instant>,
}

impl Shared {
    pub fn window_of(&self, pid: i32, el: &Ax) -> Option<WindowId> {
        self.wins
            .iter()
            .find(|(_, w)| w.pid == pid && w.el == *el)
            .map(|(id, _)| *id)
    }

    pub fn forget(&mut self, id: WindowId) {
        self.wins.remove(&id);
        self.frames.forget(id);
        self.targets.remove(&id);
        self.applied.remove(&id);
        self.bursts.remove(&id);
    }
}

/// The shared state behind its lock.
#[derive(Clone, Default)]
pub struct SharedRef(Arc<Mutex<Shared>>);

impl SharedRef {
    pub fn lock(&self) -> MutexGuard<'_, Shared> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A moved/resized notification of `el` (on the observer thread): read the frame and report
/// it as the user's change unless it is an echo of Oriel's own sets.
pub fn geometry_note(shared: &SharedRef, sink: &Sink, pid: i32, el: &Ax) {
    let id = {
        let s = shared.lock();
        let Some(id) = s.window_of(pid, el) else { return };
        if s.bursts.contains_key(&id) {
            return;
        }
        id
    };
    let Some(real) = el.frame() else { return };
    let report = {
        let mut s = shared.lock();
        // A burst may have started while the frame was read.
        !s.bursts.contains_key(&id) && s.frames.should_report(id, real)
    };
    if report {
        sink(Event::WindowFrame { window: id, frame: real, by_user: true });
    }
}
