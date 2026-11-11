//! `AXObserver` wrapper and the thread that runs the CFRunLoop for all observers.

use crate::ax::Ax;
use crate::ffi::*;
use crate::msg::{AxNote, Msg};
use crate::shared::{SharedRef, Sink};
use core_foundation_sys::base::CFTypeRef;
use core_foundation_sys::date::CFAbsoluteTimeGetCurrent;
use core_foundation_sys::runloop::*;
use core_foundation_sys::string::CFStringRef;
use std::ffi::c_void;
use std::sync::mpsc::{Sender, channel};

/// A handle to the observer thread's run loop.
#[derive(Clone, Copy)]
pub struct RunLoop(CFRunLoopRef);

// SAFETY: CFRunLoop's add/remove source and wake-up functions are thread-safe. The run loop
// lives as long as the process (its thread never ends).
unsafe impl Send for RunLoop {}

extern "C" fn noop_timer(_t: CFRunLoopTimerRef, _info: *mut c_void) {}

/// Start the thread that runs the observers' run loop. `None` if the thread cannot start.
pub fn spawn_runloop_thread() -> Option<RunLoop> {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("oriel-ax-runloop".into())
        .spawn(move || {
            // SAFETY: plain run loop calls on the current thread. A far-future repeating timer
            // keeps the loop alive while it has no observer source yet.
            unsafe {
                let rl = CFRunLoopGetCurrent();
                let timer = CFRunLoopTimerCreate(
                    std::ptr::null(),
                    CFAbsoluteTimeGetCurrent() + 1.0e9,
                    1.0e9,
                    0,
                    0,
                    noop_timer,
                    std::ptr::null_mut(),
                );
                if !timer.is_null() {
                    CFRunLoopAddTimer(rl, timer, kCFRunLoopDefaultMode);
                }
                let _ = tx.send(RunLoop(rl));
                CFRunLoopRun();
            }
        })
        .ok()?;
    rx.recv().ok()
}

struct Ctx {
    pid: i32,
    tx: Sender<Msg>,
    /// Moved and resized notes are handled right here, on this thread: the bar follows a
    /// window the user drags without waiting behind the worker's queue.
    geo: (SharedRef, Sink),
}

extern "C" fn callback(_observer: *mut c_void, element: *mut c_void, notification: CFStringRef, refcon: *mut c_void) {
    let _ = std::panic::catch_unwind(|| {
        // SAFETY: `refcon` is the leaked `Ctx` that `Observer::new` registered; it is never freed.
        let ctx = unsafe { &*(refcon as *const Ctx) };
        let (Some(element), Some(name)) = (Ax::retain(element), cf_ptr_to_string(notification as *const c_void)) else {
            return;
        };
        if name == "AXMoved" || name == "AXResized" {
            crate::shared::geometry_note(&ctx.geo.0, &ctx.geo.1, ctx.pid, &element);
            return;
        }
        let _ = ctx.tx.send(Msg::Ax(AxNote { pid: ctx.pid, element, name }));
    });
}

/// One `AXObserver` attached to the observer thread's run loop.
pub struct Observer {
    obs: Cf,
    rl: RunLoop,
    refcon: *mut c_void,
}

// SAFETY: the observer object and its context are only used through thread-safe CF calls.
unsafe impl Send for Observer {}

impl Observer {
    pub fn new(pid: i32, tx: Sender<Msg>, rl: RunLoop, geo: (SharedRef, Sink)) -> Option<Observer> {
        let mut raw: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid callback and out pointer; a +1 observer on success.
        let err = unsafe { AXObserverCreate(pid, callback, &mut raw) };
        if err != AX_OK {
            return None;
        }
        // SAFETY: `raw` is a +1 object on success.
        let obs = unsafe { Cf::from_owned(raw) }?;
        // The context is leaked on purpose: a callback may still run after the observer is
        // dropped, and the context is tiny.
        let refcon = Box::into_raw(Box::new(Ctx { pid, tx, geo })) as *mut c_void;
        // SAFETY: the source belongs to the observer; the run loop outlives it.
        unsafe {
            let src = AXObserverGetRunLoopSource(obs.as_ptr());
            CFRunLoopAddSource(rl.0, src, kCFRunLoopDefaultMode);
            CFRunLoopWakeUp(rl.0);
        }
        Some(Observer { obs, rl, refcon })
    }

    /// Subscribe to one notification on `el`. True when it worked.
    pub fn add(&self, el: &Ax, name: &str) -> bool {
        let Some(n) = cfstr(name) else { return false };
        // SAFETY: valid observer, element and name; `refcon` stays valid.
        let err = unsafe { AXObserverAddNotification(self.obs.as_ptr(), el.0.as_ptr(), n.as_ptr() as CFTypeRef as _, self.refcon) };
        err == AX_OK || err == -25209 // kAXErrorNotificationAlreadyRegistered
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: remove the source before the observer is released.
        unsafe {
            let src = AXObserverGetRunLoopSource(self.obs.as_ptr());
            CFRunLoopRemoveSource(self.rl.0, src, kCFRunLoopDefaultMode);
        }
    }
}
