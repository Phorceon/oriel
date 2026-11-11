//! Termination signals: un-hide the watched apps, then exit.
//!
//! Oriel hides the apps of inactive tabs. If a signal killed it, they would stay hidden. The
//! raw handler does the one thing that is safe in a handler: it writes the signal number to a
//! pipe. A normal thread reads the pipe, un-hides the watched apps and exits. Nothing else
//! changes: minimized windows stay minimized.

use crate::appctl;
use std::collections::HashSet;
use std::ffi::{c_int, c_void};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, Once};

/// The bundle ids that the worker watches. The worker writes it, the signal thread reads it.
pub type Watched = Arc<Mutex<HashSet<String>>>;

const SIGNALS: [c_int; 3] = [1 /* SIGHUP */, 2 /* SIGINT */, 15 /* SIGTERM */];

unsafe extern "C" {
    fn pipe(fds: *mut c_int) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn signal(sig: c_int, handler: usize) -> usize;
    fn _exit(status: c_int) -> !;
}

static WRITE_FD: AtomicI32 = AtomicI32::new(-1);
static INSTALL: Once = Once::new();

/// The handler. Async-signal-safe: an atomic load and one `write`.
extern "C" fn on_signal(sig: c_int) {
    let fd = WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = sig as u8;
        // SAFETY: `write` is async-signal-safe; the buffer is one live byte.
        unsafe { write(fd, (&byte as *const u8).cast(), 1) };
    }
}

/// Install the handlers once and start the thread that acts on them.
pub fn install(watched: Watched) {
    INSTALL.call_once(|| {
        let mut fds = [0 as c_int; 2];
        // SAFETY: `fds` has room for the two descriptors.
        if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
            return;
        }
        let (read_fd, write_fd) = (fds[0], fds[1]);
        WRITE_FD.store(write_fd, Ordering::Relaxed);
        let spawned = std::thread::Builder::new().name("oriel-signals".into()).spawn(move || {
            loop {
                let mut byte = 0u8;
                // SAFETY: reads one byte into a live byte.
                let n = unsafe { read(read_fd, (&mut byte as *mut u8).cast(), 1) };
                if n != 1 {
                    if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return;
                }
                unhide_all(&watched);
                // SAFETY: leave at once, with the conventional status for a signal.
                unsafe { _exit(128 + c_int::from(byte)) };
            }
        });
        if spawned.is_err() {
            WRITE_FD.store(-1, Ordering::Relaxed);
            return;
        }
        for sig in SIGNALS {
            // SAFETY: `on_signal` is async-signal-safe. `signal` replaces the default action
            // (and any SIG_IGN) with it.
            unsafe { signal(sig, on_signal as extern "C" fn(c_int) as usize) };
        }
    });
}

/// Un-hide every watched running app, and nothing else.
fn unhide_all(watched: &Watched) {
    let bundles: Vec<String> = match watched.lock() {
        Ok(set) => set.iter().cloned().collect(),
        Err(poisoned) => poisoned.into_inner().iter().cloned().collect(),
    };
    for bundle in bundles {
        appctl::unhide_bundle(&bundle);
    }
}
