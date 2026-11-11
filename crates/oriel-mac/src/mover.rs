//! The frame mover: a thread that only runs `SetFrame`, so a drag never waits behind the
//! worker (a `Present` that waits for an animation, a window scan of a slow app).
//!
//! - Newest wins: `request` replaces a frame not yet applied for that window.
//! - Position only when the size is the size last applied: one AX call per frame.
//! - `AXEnhancedUserInterface` is read once per burst and per app (and switched off for the
//!   burst when it is on), not on every frame.
//! - No read-back per frame. A burst settles when no request came for [`SETTLE`]: then one
//!   full set if only positions were set, one read-back, the fit for a refused size, and at
//!   most one refusal report.

use crate::shared::{SharedRef, Sink};
use oriel_core::{Event, Rect, WindowId};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How long a window must go without a `SetFrame` before its burst settles.
pub const SETTLE: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct Mover {
    pending: Mutex<HashMap<WindowId, Rect>>,
    cv: Condvar,
}

impl Mover {
    /// Queue a frame for a window; a frame not yet applied for it is dropped.
    pub fn request(&self, id: WindowId, frame: Rect) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.insert(id, frame);
        self.cv.notify_one();
    }
}

struct Burst {
    last: Instant,
    frame: Rect,
    pid: i32,
    /// Some sets in this burst only moved the window.
    moved_only: bool,
}

struct State {
    shared: SharedRef,
    sink: Sink,
    bursts: HashMap<WindowId, Burst>,
    /// Apps whose `AXEnhancedUserInterface` this mover switched off, to switch back on.
    eui_off: HashMap<i32, crate::ax::Ax>,
    /// Apps whose `AXEnhancedUserInterface` was read in the current burst (and was off).
    eui_checked: HashMap<i32, ()>,
}

pub fn spawn(mover: Arc<Mover>, shared: SharedRef, sink: Sink) {
    let mut st = State { shared, sink, bursts: HashMap::new(), eui_off: HashMap::new(), eui_checked: HashMap::new() };
    let _ = std::thread::Builder::new().name("oriel-mover".into()).spawn(move || {
        loop {
            let batch: Vec<(WindowId, Rect)> = {
                let mut pending = mover.pending.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if !pending.is_empty() {
                        break;
                    }
                    let next = st.bursts.values().map(|b| b.last + SETTLE).min();
                    match next {
                        None => pending = mover.cv.wait(pending).unwrap_or_else(|e| e.into_inner()),
                        Some(at) => {
                            let now = Instant::now();
                            if at <= now {
                                break;
                            }
                            pending = mover.cv.wait_timeout(pending, at - now).unwrap_or_else(|e| e.into_inner()).0;
                        }
                    }
                }
                pending.drain().collect()
            };
            for (id, frame) in batch {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| st.apply(id, frame)));
            }
            let now = Instant::now();
            let due: Vec<WindowId> =
                st.bursts.iter().filter(|(_, b)| b.last + SETTLE <= now).map(|(id, _)| *id).collect();
            for id in due {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| st.settle(id)));
            }
        }
    });
}

impl State {
    fn apply(&mut self, id: WindowId, frame: Rect) {
        let now = Instant::now();
        let (w, applied) = {
            let mut s = self.shared.lock();
            let Some(w) = s.wins.get(&id).cloned() else { return };
            s.bursts.insert(id, now);
            s.targets.insert(id, frame);
            s.frames.note_set(id, frame);
            (w, s.applied.get(&id).copied())
        };
        // Once per burst and app: a window under `AXEnhancedUserInterface` animates every set.
        if !self.eui_checked.contains_key(&w.pid) && !self.eui_off.contains_key(&w.pid) {
            self.eui_checked.insert(w.pid, ());
            if w.app.bool("AXEnhancedUserInterface") == Some(true) {
                w.app.set_bool("AXEnhancedUserInterface", false);
                self.eui_off.insert(w.pid, w.app.clone());
            }
        }
        let same_size = applied.is_some_and(|(aw, ah)| (aw - frame.w).abs() < 0.5 && (ah - frame.h).abs() < 0.5);
        if same_size {
            w.el.set_position(frame.x, frame.y);
        } else {
            w.el.set_size(frame.w, frame.h);
            w.el.set_position(frame.x, frame.y);
            w.el.set_size(frame.w, frame.h);
            self.shared.lock().applied.insert(id, (frame.w, frame.h));
        }
        let burst = self.bursts.entry(id).or_insert(Burst { last: now, frame, pid: w.pid, moved_only: false });
        burst.last = now;
        burst.frame = frame;
        burst.moved_only |= same_size;
    }

    /// The burst on `id` is over: make sure the window has the size, read it back once, put a
    /// refusing window at its fit, and report a refusal once.
    fn settle(&mut self, id: WindowId) {
        let Some(burst) = self.bursts.remove(&id) else { return };
        let w = self.shared.lock().wins.get(&id).cloned();
        let real = w.as_ref().and_then(|w| {
            let mut real = w.el.frame()?;
            let frame = burst.frame;
            let size_ok = (real.w - frame.w).abs() < 1.0 && (real.h - frame.h).abs() < 1.0;
            if !size_ok && burst.moved_only {
                // The size was taken as known; set it once for real.
                w.el.set_size(frame.w, frame.h);
                w.el.set_position(frame.x, frame.y);
                w.el.set_size(frame.w, frame.h);
                real = w.el.frame()?;
            }
            let size_ok = (real.w - frame.w).abs() < 1.0 && (real.h - frame.h).abs() < 1.0;
            if !size_ok {
                let fit = frame.fit(real.w, real.h);
                if !real.close_to(&fit) {
                    w.el.set_position(fit.x, fit.y);
                    real = w.el.frame()?;
                }
            }
            Some(real)
        });
        if !self.bursts.values().any(|b| b.pid == burst.pid) {
            self.eui_checked.remove(&burst.pid);
            if let Some(app) = self.eui_off.remove(&burst.pid) {
                app.set_bool("AXEnhancedUserInterface", true);
            }
        }
        let refused = {
            let mut s = self.shared.lock();
            // A newer burst may have started on this window from another thread's request.
            if s.bursts.get(&id).is_some_and(|at| *at > burst.last) {
                return;
            }
            s.bursts.remove(&id);
            match real {
                Some(real) => {
                    s.applied.insert(id, (real.w, real.h));
                    s.frames.note_actual(id, real).then_some(real)
                }
                None => None,
            }
        };
        if let Some(real) = refused {
            (self.sink)(Event::WindowFrame { window: id, frame: real, by_user: false });
        }
    }
}
