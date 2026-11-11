//! macOS implementation of `oriel_core::Platform`.
//!
//! Accessibility (AX) moves and raises real app windows. NSWorkspace launches, hides and quits
//! apps. `send` only puts a command on a channel; one worker thread runs it, so the caller
//! never waits for a slow app. `SetFrame` skips that queue: a thread of its own (`mover.rs`)
//! runs it, newest first. A third thread runs the CFRunLoop of the AX observers and handles
//! moved/resized notes itself, and the main thread holds the AppKit notification observers.

mod appctl;
mod apps;
mod ax;
mod ffi;
mod frame;
mod mover;
mod msg;
mod observer;
mod scan;
mod screen;
mod shared;
mod signals;
mod worker;

use msg::Msg;
use oriel_core::{Command, EventSink, Platform};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use frame::{FrameTracker, appkit_to_top_left};
pub use screen::window_bounds;
pub use scan::scan as scan_apps;

/// The real macOS platform: Accessibility for windows, NSWorkspace for apps.
pub struct MacPlatform {
    tx: Sender<Msg>,
    rx: Mutex<Option<Receiver<Msg>>>,
    /// `SetFrame` goes here, never through the worker's queue.
    mover: Arc<mover::Mover>,
    shared: shared::SharedRef,
}

impl MacPlatform {
    /// Create the platform. Call on the main thread.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx: Mutex::new(Some(rx)), mover: Arc::default(), shared: shared::SharedRef::default() }
    }
}

impl Platform for MacPlatform {
    fn start(&self, sink: EventSink) {
        let Some(rx) = self.rx.lock().ok().and_then(|mut r| r.take()) else { return };
        let sink: worker::Sink = Arc::new(sink);
        let tx = self.tx.clone();

        // Main-thread part: screen and workspace notifications.
        let (s, t) = (sink.clone(), tx.clone());
        match objc2::MainThreadMarker::new() {
            Some(mtm) => screen::setup_main_thread(mtm, s, t),
            None => dispatch2::DispatchQueue::main().exec_async(move || {
                if let Some(mtm) = objc2::MainThreadMarker::new() {
                    screen::setup_main_thread(mtm, s, t);
                }
            }),
        }

        let rl = observer::spawn_runloop_thread();
        // A signal must not leave the hidden apps hidden.
        let watched = signals::Watched::default();
        signals::install(watched.clone());
        mover::spawn(self.mover.clone(), self.shared.clone(), sink.clone());
        let w = worker::Worker::new(sink, tx, rl, watched, self.shared.clone());
        let _ = std::thread::Builder::new().name("oriel-worker".into()).spawn(move || w.run(rx));
    }

    fn send(&self, command: Command) {
        match command {
            Command::SetFrame { window, frame } => self.mover.request(window, frame),
            command => {
                let _ = self.tx.send(Msg::Cmd(command));
            }
        }
    }

    fn flush(&self, timeout: Duration) {
        let (done, wait) = channel();
        if self.tx.send(Msg::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(timeout);
        }
    }
}
