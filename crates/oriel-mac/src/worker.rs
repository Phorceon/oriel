//! The worker thread: owns app lifetime and window tracking, runs every command except
//! `SetFrame` (the mover's), sends events.

use crate::ax::Ax;
use crate::msg::Msg;
use crate::observer::RunLoop;
use crate::shared::{SharedRef, WinRef};
use crate::{appctl, ffi, scan};
use core_foundation_sys::dictionary::{
    CFDictionaryCreate, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
};
use core_foundation_sys::number::kCFBooleanTrue;
use oriel_core::{Command, Event, Rect, WindowId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// The idle turn of the worker loop.
const TICK: Duration = Duration::from_millis(100);
/// The turn while a `Present` waits for its window to show.
const GATE_TICK: Duration = Duration::from_millis(8);
/// The longest a `Present` holds the hides and minimizes after it. An un-minimize animation
/// takes about 0.35 s on macOS 26.
const GATE_MAX: Duration = Duration::from_millis(650);
/// A `Present` that comes within this time after an older one that holds no command yet is
/// the second half of a pair (split view): the gate waits for both windows. Both are sent in
/// the same model step, so they reach the worker within one turn of its loop.
const PAIR_WINDOW: Duration = Duration::from_millis(100);

/// State of one watched, running app.
pub struct AppState {
    pub pid: i32,
    pub app: Option<Ax>,
    pub observer: Option<crate::observer::Observer>,
    /// `AppRunning` was sent for this run (and this trust state).
    pub announced: bool,
    /// Announce at once, even with no windows (the app was already running).
    pub immediate: bool,
    pub first_seen: Instant,
    pub next_try: Instant,
    pub tries: u32,
}

/// One tracked standard window.
pub struct WinEntry {
    pub bundle: String,
    pub pid: i32,
    pub el: Ax,
    pub title: String,
}

/// A window whose subrole was not ready: look again later.
pub struct Recheck {
    pub at: Instant,
    pub bundle: String,
    pub el: Ax,
    pub left: u8,
}

/// A `Present` whose window is not on screen in place yet. The hides and minimizes sent after
/// it wait here, so the content area under the bar is never empty.
struct Gate {
    /// The presented windows that are not on screen in place yet. Two `Present`s in a row
    /// (split view) wait together.
    lands: Vec<Landing>,
    /// The window of the newest `Present`: the one that must end frontmost.
    focus: WindowId,
    /// When the gate began.
    since: Instant,
    until: Instant,
    deferred: VecDeque<Command>,
    /// Report `focus` as focused when the gate opens (see `release_gate`). Off once the model
    /// asked for something else (a launch, a reopen, a hide of this app).
    report_focus: bool,
}

/// One presented window that the gate waits for.
struct Landing {
    window: WindowId,
    /// Where the window must be (the frame it took when the frame was set).
    target: Rect,
    /// The last on-screen bounds seen, and how many checks in a row saw them.
    last: Option<(Rect, u32)>,
    /// The frame was applied again once after the window showed somewhere else.
    reapplied: bool,
}

pub struct Worker {
    pub sink: Sink,
    pub tx: Sender<Msg>,
    pub rl: Option<RunLoop>,
    pub trusted: bool,
    pub watched: HashSet<String>,
    /// A copy of `watched` for the signal thread.
    pub shared_watched: crate::signals::Watched,
    pub apps: HashMap<String, AppState>,
    pub wins: HashMap<WindowId, WinEntry>,
    /// What the mover and the observer thread see of the windows.
    pub shared: SharedRef,
    pub rechecks: Vec<Recheck>,
    /// Apps whose activation read `AXFrontmost == false`: check once more at that time.
    pub focus_retries: Vec<(Instant, String)>,
    pub last_focus: Option<(WindowId, Instant)>,
    gate: Option<Gate>,
    last_trust: Instant,
    last_reconcile: Instant,
    pub last_sync: Instant,
}

impl Worker {
    pub fn new(
        sink: Sink,
        tx: Sender<Msg>,
        rl: Option<RunLoop>,
        shared_watched: crate::signals::Watched,
        shared: SharedRef,
    ) -> Self {
        let now = Instant::now();
        Worker {
            sink,
            tx,
            rl,
            trusted: false,
            watched: HashSet::new(),
            shared_watched,
            apps: HashMap::new(),
            wins: HashMap::new(),
            shared,
            rechecks: Vec::new(),
            focus_retries: Vec::new(),
            last_focus: None,
            gate: None,
            last_trust: now,
            last_reconcile: now,
            last_sync: now,
        }
    }

    pub fn emit(&self, e: Event) {
        (self.sink)(e);
    }

    /// Track a window here and for the mover and the observer thread.
    pub fn insert_window(&mut self, id: WindowId, entry: WinEntry, app: Ax) {
        self.shared.lock().wins.insert(id, WinRef { pid: entry.pid, app, el: entry.el.clone() });
        self.wins.insert(id, entry);
    }

    /// Forget a window everywhere.
    pub fn remove_window(&mut self, id: WindowId) {
        self.wins.remove(&id);
        self.shared.lock().forget(id);
        if self.gate.as_ref().is_some_and(|g| g.focus == id || g.lands.iter().any(|l| l.window == id)) {
            self.release_gate();
        }
    }

    pub fn run(mut self, rx: Receiver<Msg>) {
        // SAFETY: plain query.
        self.trusted = unsafe { ffi::AXIsProcessTrusted() } != 0;
        self.emit(Event::TrustChanged(self.trusted));
        loop {
            let mut batch = Vec::new();
            let wait = if self.gate.is_some() { GATE_TICK } else { TICK };
            match rx.recv_timeout(wait) {
                Ok(m) => batch.push(m),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            while let Ok(m) = rx.try_recv() {
                batch.push(m);
            }
            for m in batch {
                // A panic must not kill the worker.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(m)));
            }
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.tick()));
        }
    }

    fn handle(&mut self, m: Msg) {
        if self.gate.is_some() && debug_marks() {
            let what = match &m {
                Msg::Cmd(c) => format!("cmd {c:?}"),
                Msg::Ax(n) => format!("ax {}", n.name),
                Msg::Workspace(n) => format!("ws {n:?}"),
                Msg::Flush(_) => "flush".into(),
            };
            debug_mark(&what);
        }
        match m {
            Msg::Cmd(c) => self.exec(c),
            Msg::Workspace(n) => self.on_workspace(n),
            Msg::Ax(n) => self.on_ax(n),
            Msg::Flush(done) => {
                // Everything sent before the flush runs now, held commands included.
                self.release_gate();
                let _ = done.send(());
            }
        }
    }

    fn tick(&mut self) {
        self.check_gate();
        let now = Instant::now();
        if now.duration_since(self.last_trust) >= Duration::from_secs(1) {
            self.last_trust = now;
            self.check_trust();
        }
        // Launches and quits arrive as workspace notes; this only catches a missed one.
        if now.duration_since(self.last_reconcile) >= Duration::from_secs(3) {
            self.last_reconcile = now;
            self.reconcile();
        }
        self.tick_apps();
    }

    /// Compare `AXIsProcessTrusted` with the last known value.
    pub fn check_trust(&mut self) {
        // SAFETY: plain query.
        let t = unsafe { ffi::AXIsProcessTrusted() } != 0;
        if t != self.trusted {
            self.trusted = t;
            self.emit(Event::TrustChanged(t));
            self.on_trust_changed(t);
        }
    }

    fn exec(&mut self, cmd: Command) {
        if let Some(gate) = self.gate.as_mut() {
            let gate_app = self.wins.get(&gate.focus).map(|w| w.bundle.as_str());
            match &cmd {
                Command::Launch { .. } | Command::Reopen { .. } => gate.report_focus = false,
                Command::HideApp { bundle_id } if Some(bundle_id.as_str()) == gate_app => gate.report_focus = false,
                _ => {}
            }
            // Hides and minimizes after a `Present` wait for its window to show.
            if matches!(cmd, Command::HideApp { .. } | Command::Minimize { .. }) {
                gate.deferred.push_back(cmd);
                return;
            }
        }
        self.exec_now(cmd);
    }

    fn exec_now(&mut self, cmd: Command) {
        match cmd {
            Command::RequestTrust => {
                prompt_for_trust();
                self.check_trust();
            }
            Command::ScanApps => {
                let sink = self.sink.clone();
                let _ = std::thread::Builder::new().name("oriel-scan".into()).spawn(move || {
                    sink(Event::AppsScanned(scan::scan(&scan::default_roots())));
                });
            }
            Command::Watch { bundle_id } => self.watch(bundle_id),
            Command::Unwatch { bundle_id } => self.unwatch(&bundle_id),
            Command::Launch { app } => appctl::launch(&app.path),
            Command::Reopen { bundle_id } => appctl::reopen(&bundle_id),
            Command::Present { bundle_id, window, frame } => {
                // A newer `Present` replaces the older one's wait: its own hides and
                // minimizes describe the whole new state. Exception: the older wait holds
                // no command yet and is a few ms old. Then it is the first half of a pair
                // (split view, `Present` of the other pane first): both windows are placed
                // and raised either way, and the gate waits for both to land.
                let older = match self.gate.take() {
                    Some(g) if g.deferred.is_empty() && g.since.elapsed() < PAIR_WINDOW => g.lands,
                    _ => Vec::new(),
                };
                if self.trusted {
                    self.present(&bundle_id, window, frame);
                    if let Some(gate) = self.gate.as_mut() {
                        let mut lands: Vec<Landing> = older.into_iter().filter(|l| l.window != window).collect();
                        lands.append(&mut gate.lands);
                        gate.lands = lands;
                    }
                }
            }
            // The mover runs these (`MacPlatform::send`); one that arrives here is late.
            Command::SetFrame { .. } => {}
            Command::HideApp { bundle_id } => appctl::hide(&bundle_id),
            Command::ShowApp { bundle_id } => appctl::unhide_bundle(&bundle_id),
            Command::Minimize { window } => {
                if let (true, Some(w)) = (self.trusted, self.wins.get(&window))
                    && w.el.bool("AXMinimized") != Some(true)
                {
                    w.el.set_bool("AXMinimized", true);
                }
            }
            Command::CloseWindow { window } => {
                if let (true, Some(w)) = (self.trusted, self.wins.get(&window))
                    && let Some(button) = w.el.element("AXCloseButton")
                {
                    button.set_timeout();
                    button.perform("AXPress");
                }
            }
            Command::QuitApp { bundle_id } => appctl::quit(&bundle_id),
        }
    }

    pub fn app_element(&self, pid: i32) -> Option<Ax> {
        self.apps
            .values()
            .find(|s| s.pid == pid)
            .and_then(|s| s.app.clone())
            .or_else(|| Ax::app(pid))
    }

    /// `Present`: frame first (while still hidden or minimized), then show, raise, activate.
    fn present(&mut self, bundle_id: &str, id: WindowId, frame: Rect) {
        let Some(w) = self.wins.get(&id) else { return };
        if w.bundle != bundle_id {
            return;
        }
        let (pid, el) = (w.pid, w.el.clone());
        let Some(app) = self.app_element(pid) else { return };
        // The newest request wins: a drag may have moved on since the model sent this. The
        // window is quiet from here to its read-back: its moved/resized notes are echoes of
        // these sets (`shared::geometry_note`), never the user's.
        let frame = {
            let mut s = self.shared.lock();
            let frame = s.targets.get(&id).copied().filter(|t| !t.close_to(&frame) && s.bursts.contains_key(&id)).unwrap_or(frame);
            s.targets.insert(id, frame);
            s.frames.note_set(id, frame);
            s.bursts.insert(id, Instant::now());
            frame
        };
        let minimized = el.bool("AXMinimized") == Some(true);
        let hidden = appctl::is_hidden(pid);
        // 1. The frame, before the window shows: it appears in place, with no jump. A
        //    minimized window takes it as the frame it restores to.
        let real = place(&app, &el, frame);
        // 2. Show it. Two routes to the front: AXFrontmost works from a background process
        //    where the cooperative `activate` can be refused.
        if hidden {
            appctl::unhide(pid);
            app.set_bool("AXHidden", false);
        }
        if minimized {
            // Activate first and un-minimize last: while the window animates out of the Dock
            // the app answers no AX call, and un-minimizing already raises the window and
            // makes it main.
            app.set_bool("AXFrontmost", true);
            appctl::activate(pid);
            el.set_bool("AXMinimized", false);
        } else {
            el.perform("AXRaise");
            el.set_bool("AXMain", true);
            app.set_bool("AXFrontmost", true);
            appctl::activate(pid);
        }
        let target = real.unwrap_or(frame);
        let refused = {
            let mut s = self.shared.lock();
            s.applied.insert(id, (target.w, target.h));
            s.bursts.remove(&id);
            real.is_some_and(|real| s.frames.note_actual(id, real))
        };
        if refused {
            self.emit(Event::WindowFrame { window: id, frame: target, by_user: false });
        }
        self.gate = Some(Gate {
            lands: vec![Landing { window: id, target, last: None, reapplied: false }],
            focus: id,
            since: Instant::now(),
            until: Instant::now() + GATE_MAX,
            deferred: VecDeque::new(),
            report_focus: true,
        });
        self.check_gate();
    }

    /// Release the held commands once every presented window is on screen in place, or once
    /// the wait is over. A window that shows somewhere else and stays there (an app that
    /// ignored the frame while hidden) gets the frame once more.
    fn check_gate(&mut self) {
        let Some(gate) = self.gate.as_mut() else { return };
        let mut again: Vec<(WindowId, Rect)> = Vec::new();
        gate.lands.retain_mut(|land| {
            let bounds = crate::screen::window_bounds(land.window.0);
            if debug_marks() {
                debug_mark(&format!("gate check {:?} {bounds:?}", land.window));
            }
            let Some(b) = bounds else { return true };
            if b.close_to(&land.target) {
                return false;
            }
            let steady = match land.last {
                Some((last, n)) if last.close_to(&b) => n + 1,
                _ => 1,
            };
            land.last = Some((b, steady));
            if steady >= 4 && !land.reapplied {
                land.reapplied = true;
                again.push((land.window, land.target));
            }
            true
        });
        let over = gate.lands.is_empty() || Instant::now() >= gate.until;
        for (id, target) in again {
            if let Some(w) = self.wins.get(&id) {
                let (pid, el) = (w.pid, w.el.clone());
                if let Some(app) = self.app_element(pid) {
                    self.shared.lock().bursts.insert(id, Instant::now());
                    set_frame(&app, &el, target);
                    let real = el.frame();
                    let mut s = self.shared.lock();
                    s.bursts.remove(&id);
                    if let Some(real) = real {
                        s.frames.note_actual(id, real);
                    }
                }
            }
        }
        if over {
            self.release_gate();
        }
    }

    fn release_gate(&mut self) {
        let Some(gate) = self.gate.take() else { return };
        debug_mark(&format!("gate release {:?}", gate.focus));
        // An un-minimize or an activation can order the window in front of the bar after the
        // model raised the bar. Report the presented window as focused once it is in place
        // (only when it truly is the frontmost app's main window): the model answers with
        // `RaiseBar`, so the bar ends on top and the window's shadow never falls on it.
        if gate.report_focus {
            self.report_presented_focus(gate.focus);
        }
        for cmd in gate.deferred {
            self.exec_now(cmd);
        }
    }

    /// No AX call here: the app may still be busy with its animation, and the bar should go
    /// on top the moment the window lands. NSWorkspace says which app is frontmost.
    /// Is a `Present` of a window of this app waiting to land? Then its focus notes need no
    /// AX check (the app may answer none while it animates; the gate reports the focus).
    pub fn presenting(&self, bundle: &str) -> bool {
        self.gate.as_ref().is_some_and(|g| {
            g.report_focus
                && std::iter::once(g.focus)
                    .chain(g.lands.iter().map(|l| l.window))
                    .any(|id| self.wins.get(&id).is_some_and(|w| w.bundle == bundle))
        })
    }

    fn report_presented_focus(&mut self, id: WindowId) {
        let Some(w) = self.wins.get(&id) else { return };
        if appctl::frontmost_pid() != Some(w.pid) {
            return;
        }
        let bundle = w.bundle.clone();
        self.last_focus = Some((id, Instant::now()));
        self.emit(Event::WindowFocused { bundle_id: bundle, window: id });
    }
}

/// Set size, position, size again (windows clamp the size to the screen they are on before
/// the move). `AXEnhancedUserInterface` is switched off around the change when it is on.
pub fn set_frame(app: &Ax, win: &Ax, f: Rect) {
    let eui = app.bool("AXEnhancedUserInterface") == Some(true);
    if eui {
        app.set_bool("AXEnhancedUserInterface", false);
    }
    win.set_size(f.w, f.h);
    win.set_position(f.x, f.y);
    win.set_size(f.w, f.h);
    if eui {
        app.set_bool("AXEnhancedUserInterface", true);
    }
}

/// Set the frame and read it back. A window that refuses the size goes to its fit (the size
/// it took, top-aligned and centered in `f`). Returns the frame read back.
pub fn place(app: &Ax, win: &Ax, f: Rect) -> Option<Rect> {
    set_frame(app, win, f);
    let real = win.frame()?;
    if (real.w - f.w).abs() >= 1.0 || (real.h - f.h).abs() >= 1.0 {
        let fit = f.fit(real.w, real.h);
        if !real.close_to(&fit) {
            win.set_position(fit.x, fit.y);
            return win.frame();
        }
    }
    Some(real)
}

/// Show the system prompt (`AXIsProcessTrustedWithOptions` with the prompt option).
fn prompt_for_trust() {
    // SAFETY: a one-entry dictionary of immortal CF constants; the dictionary is released
    // after the call.
    unsafe {
        let keys = [ffi::kAXTrustedCheckOptionPrompt as *const c_void];
        let values = [kCFBooleanTrue as *const c_void];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        if let Some(d) = ffi::Cf::from_owned(dict as *const c_void) {
            ffi::AXIsProcessTrustedWithOptions(d.as_ptr());
        }
    }
}

/// Is `ORIEL_MAC_DEBUG_MARKS` set?
fn debug_marks() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ORIEL_MAC_DEBUG_MARKS").is_some())
}

/// With `ORIEL_MAC_DEBUG_MARKS` set: a timestamped line on stderr (epoch ms), for the harness.
fn debug_mark(what: &str) {
    if debug_marks() {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        eprintln!("oriel-mark {ms:.3} mac {what}");
    }
}
