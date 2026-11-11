//! Worker part 2: app lifetime, window tracking, AX notifications.

use crate::ax::Ax;
use crate::msg::{AxNote, WsNote};
use crate::observer::Observer;
use crate::worker::{AppState, Recheck, WinEntry, Worker};
use crate::appctl;
use oriel_core::{Event, WindowId, WindowInfo};
use std::time::{Duration, Instant};

/// A fresh launch waits this long for its first window before `AppRunning` goes out empty.
const LAUNCH_SETTLE: Duration = Duration::from_secs(2);
/// About ten seconds of observer attempts, every 250 ms.
const MAX_ATTACH_TRIES: u32 = 40;
/// The wait before the one re-check of an activation that read `AXFrontmost == false`.
const FOCUS_RETRY: Duration = Duration::from_millis(100);
const APP_NOTES: [&str; 4] =
    ["AXWindowCreated", "AXFocusedWindowChanged", "AXMainWindowChanged", "AXApplicationActivated"];
const WINDOW_NOTES: [&str; 5] =
    ["AXUIElementDestroyed", "AXTitleChanged", "AXMoved", "AXResized", "AXWindowMiniaturized"];
/// How often the windows of each app are listed again, in case a creation note was missed.
const SYNC_EVERY: Duration = Duration::from_secs(5);

impl Worker {
    pub fn watch(&mut self, bundle: String) {
        self.watched.insert(bundle.clone());
        self.sync_shared_watched();
        let Some(pid) = appctl::pid_of(&bundle) else { return };
        match self.apps.get_mut(&bundle) {
            Some(st) if st.pid == pid => {
                if st.observer.is_some() || !self.trusted {
                    let windows = if self.trusted { self.sync_windows(&bundle, false) } else { Vec::new() };
                    if let Some(st) = self.apps.get_mut(&bundle) {
                        st.announced = true;
                    }
                    self.emit(Event::AppRunning { bundle_id: bundle, windows });
                } else {
                    st.immediate = true;
                }
            }
            _ => {
                self.stop_app(&bundle, self.apps.contains_key(&bundle));
                self.start_app(bundle, pid, true);
            }
        }
    }

    /// Copy `watched` for the signal thread.
    fn sync_shared_watched(&self) {
        if let Ok(mut shared) = self.shared_watched.lock() {
            shared.clone_from(&self.watched);
        }
    }

    pub fn unwatch(&mut self, bundle: &str) {
        self.watched.remove(bundle);
        self.sync_shared_watched();
        self.stop_app(bundle, false);
    }

    fn start_app(&mut self, bundle: String, pid: i32, immediate: bool) {
        let now = Instant::now();
        self.apps.insert(
            bundle.clone(),
            AppState {
                pid,
                app: None,
                observer: None,
                announced: false,
                immediate,
                first_seen: now,
                next_try: now,
                tries: 0,
            },
        );
        self.try_attach(&bundle);
        self.maybe_announce(&bundle);
    }

    /// Forget an app and its windows. Optionally tell the model it stopped.
    pub fn stop_app(&mut self, bundle: &str, emit: bool) {
        let had = self.apps.remove(bundle).is_some();
        let ids: Vec<WindowId> = self.wins.iter().filter(|(_, w)| w.bundle == bundle).map(|(i, _)| *i).collect();
        for id in ids {
            self.remove_window(id);
        }
        self.rechecks.retain(|r| r.bundle != bundle);
        self.focus_retries.retain(|(_, b)| b != bundle);
        if emit && had {
            self.emit(Event::AppStopped { bundle_id: bundle.to_owned() });
        }
    }

    /// Start and stop state to match the running apps.
    pub fn reconcile(&mut self) {
        for b in self.watched.clone() {
            let pid = appctl::pid_of(&b);
            match (pid, self.apps.get(&b).map(|s| s.pid)) {
                (None, Some(_)) => self.stop_app(&b, true),
                (Some(p), Some(old)) if p != old => {
                    self.stop_app(&b, true);
                    self.start_app(b, p, false);
                }
                (Some(p), None) => self.start_app(b, p, false),
                _ => {}
            }
        }
    }

    pub fn on_trust_changed(&mut self, trusted: bool) {
        let ids: Vec<WindowId> = self.wins.keys().copied().collect();
        for id in ids {
            self.remove_window(id);
        }
        self.rechecks.clear();
        let now = Instant::now();
        for st in self.apps.values_mut() {
            st.app = None;
            st.observer = None;
            st.announced = false;
            st.immediate = true;
            st.tries = 0;
            st.next_try = now;
        }
        if !trusted {
            // Without trust the apps stay "running" for the model, with no windows.
            let names: Vec<String> = self.apps.keys().cloned().collect();
            for b in names {
                self.maybe_announce(&b);
            }
        }
    }

    pub fn tick_apps(&mut self) {
        let names: Vec<String> = self.apps.keys().cloned().collect();
        for b in &names {
            self.try_attach(b);
            self.maybe_announce(b);
        }
        let now = Instant::now();
        let (due, rest): (Vec<Recheck>, Vec<Recheck>) =
            std::mem::take(&mut self.rechecks).into_iter().partition(|r| r.at <= now);
        self.rechecks = rest;
        for mut r in due {
            if !self.apps.contains_key(&r.bundle) || self.add_window(&r.bundle, &r.el, true) {
                continue;
            }
            r.left = r.left.saturating_sub(1);
            if r.left > 0 {
                r.at = now + Duration::from_millis(500);
                self.rechecks.push(r);
            }
        }
        let (due, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.focus_retries).into_iter().partition(|(at, _)| *at <= now);
        self.focus_retries = rest;
        for (_, bundle) in due {
            self.focus_check(&bundle, false);
        }
        // A safety net for a missed `AXWindowCreated`; cheap for windows already known.
        if self.trusted && now.duration_since(self.last_sync) >= SYNC_EVERY {
            self.last_sync = now;
            for b in &names {
                if self.apps.get(b).is_some_and(|s| s.announced && s.observer.is_some()) {
                    self.sync_windows(b, true);
                }
            }
        }
    }

    /// Create the AX observer and subscribe. Retried every 250 ms for a few seconds.
    fn try_attach(&mut self, bundle: &str) {
        let now = Instant::now();
        let (tx, rl, trusted) = (self.tx.clone(), self.rl, self.trusted);
        let Some(st) = self.apps.get_mut(bundle) else { return };
        if !trusted || st.observer.is_some() || now < st.next_try || st.tries >= MAX_ATTACH_TRIES {
            return;
        }
        st.tries += 1;
        st.next_try = now + Duration::from_millis(250);
        let (Some(rl), Some(app)) = (rl, Ax::app(st.pid)) else { return };
        // The app must answer before it is worth subscribing.
        if app.string("AXRole").is_none() {
            return;
        }
        let geo = (self.shared.clone(), self.sink.clone());
        let Some(st) = self.apps.get_mut(bundle) else { return };
        let Some(obs) = Observer::new(st.pid, tx, rl, geo) else { return };
        for n in APP_NOTES {
            obs.add(&app, n);
        }
        st.app = Some(app);
        st.observer = Some(obs);
    }

    fn maybe_announce(&mut self, bundle: &str) {
        let Some(st) = self.apps.get(bundle) else { return };
        if st.announced {
            return;
        }
        let (immediate, first_seen) = (st.immediate, st.first_seen);
        let windows = if !self.trusted {
            Vec::new()
        } else if st.observer.is_none() {
            return;
        } else {
            let w = self.sync_windows(bundle, false);
            if !immediate && w.is_empty() && first_seen.elapsed() < LAUNCH_SETTLE {
                return;
            }
            w
        };
        if let Some(st) = self.apps.get_mut(bundle) {
            st.announced = true;
        }
        self.emit(Event::AppRunning { bundle_id: bundle.to_owned(), windows });
    }

    /// Read the standard windows (front to back) and track the new ones. With `emit_new`,
    /// send `WindowOpened` for each new one.
    pub fn sync_windows(&mut self, bundle: &str, emit_new: bool) -> Vec<WindowInfo> {
        let Some(app) = self.apps.get(bundle).and_then(|s| s.app.clone()) else { return Vec::new() };
        let mut out = Vec::new();
        for el in app.elements("AXWindows") {
            let Some(id) = el.window_id() else { continue };
            // A known window was checked when it was tracked: no more AX calls for it.
            if let Some(w) = self.wins.get(&id) {
                out.push(WindowInfo { id, title: w.title.clone() });
                continue;
            }
            if std::env::var_os("ORIEL_MAC_DEBUG").is_some() {
                eprintln!("[sync] {bundle}: {}", el.describe());
            }
            if !el.is_standard_window() {
                continue;
            }
            if let Some(info) = self.track(bundle, &el) {
                if emit_new {
                    self.emit(Event::WindowOpened { bundle_id: bundle.to_owned(), window: info.clone() });
                }
                out.push(info);
            }
        }
        out
    }

    /// Track a window and subscribe to its notifications. `None` if already tracked.
    fn track(&mut self, bundle: &str, el: &Ax) -> Option<WindowInfo> {
        let id = el.window_id()?;
        if self.wins.contains_key(&id) {
            return None;
        }
        let st = self.apps.get(bundle)?;
        let obs = st.observer.as_ref()?;
        el.set_timeout();
        for n in WINDOW_NOTES {
            obs.add(el, n);
        }
        let title = el.title();
        let (pid, app) = (st.pid, st.app.clone()?);
        self.insert_window(id, WinEntry { bundle: bundle.to_owned(), pid, el: el.clone(), title: title.clone() }, app);
        Some(WindowInfo { id, title })
    }

    /// Add a new standard window and report it. False if it is not (yet) a standard window.
    fn add_window(&mut self, bundle: &str, el: &Ax, report: bool) -> bool {
        if !el.is_standard_window() {
            return false;
        }
        if let Some(info) = self.track(bundle, el) {
            let announced = self.apps.get(bundle).is_some_and(|s| s.announced);
            if report && announced {
                self.emit(Event::WindowOpened { bundle_id: bundle.to_owned(), window: info });
                // A new window is often also the main window; its focus note came too early.
                self.focus_check(bundle, false);
            }
        }
        true
    }

    fn bundle_of(&self, pid: i32) -> Option<String> {
        self.apps.iter().find(|(_, s)| s.pid == pid).map(|(b, _)| b.clone())
    }

    fn lookup(&self, pid: i32, el: &Ax) -> Option<WindowId> {
        self.wins
            .iter()
            .find(|(_, w)| w.pid == pid && w.el == *el)
            .map(|(id, _)| *id)
            .or_else(|| el.window_id().filter(|id| self.wins.contains_key(id)))
    }

    pub fn on_workspace(&mut self, n: WsNote) {
        match n {
            WsNote::Reconcile => self.reconcile(),
            WsNote::Activated(pid) => {
                if let Some(b) = self.bundle_of(pid) {
                    self.focus_check(&b, true);
                }
            }
        }
    }

    pub fn on_ax(&mut self, n: AxNote) {
        let Some(bundle) = self.bundle_of(n.pid) else { return };
        match n.name.as_str() {
            "AXWindowCreated" => {
                if !self.add_window(&bundle, &n.element, true) {
                    // The subrole is sometimes not ready yet: look again shortly.
                    let at = Instant::now() + Duration::from_millis(150);
                    self.rechecks.push(Recheck { at, bundle, el: n.element, left: 5 });
                }
            }
            "AXUIElementDestroyed" => {
                // A dead element cannot answer: match it by identity.
                let found = self.wins.iter().find(|(_, w)| w.pid == n.pid && w.el == n.element).map(|(i, _)| *i);
                if let Some(id) = found {
                    self.remove_window(id);
                    self.emit(Event::WindowClosed { bundle_id: bundle, window: id });
                }
            }
            "AXTitleChanged" => {
                if let Some(id) = self.lookup(n.pid, &n.element) {
                    let title = n.element.title();
                    if let Some(w) = self.wins.get_mut(&id)
                        && w.title != title
                    {
                        w.title = title.clone();
                        self.emit(Event::WindowTitle { window: id, title });
                    }
                }
            }
            "AXApplicationActivated" => self.focus_check(&bundle, true),
            // The app put its remaining window in front after a minimize, maybe over the bar:
            // report its main window again, so the model raises the bar (once more).
            "AXWindowMiniaturized" => {
                self.last_focus = None;
                self.focus_check(&bundle, false);
            }
            "AXMainWindowChanged" | "AXFocusedWindowChanged" => self.focus_check(&bundle, false),
            _ => {}
        }
    }

    /// Send `WindowFocused` for the main (or focused) tracked window of a frontmost app.
    fn focus_check(&mut self, bundle: &str, known_front: bool) {
        if self.presenting(bundle) {
            return;
        }
        let Some(st) = self.apps.get(bundle) else { return };
        if !st.announced {
            return;
        }
        let (pid, Some(app)) = (st.pid, st.app.clone()) else { return };
        // Always check, also for an "activated" note: a note can be stale when the user
        // switches tabs fast, and a stale `WindowFocused` makes two tabs fight for the front.
        let front = app.bool("AXFrontmost");
        if std::env::var_os("ORIEL_MAC_DEBUG").is_some() {
            eprintln!("[focus] {bundle}: known_front={known_front} AXFrontmost={front:?}");
        }
        if front != Some(true) {
            // An "activated" note can come a moment before the app reports itself frontmost
            // (seen live: the first note reads `false`, a later one `true`). Look once more
            // soon, instead of dropping a real activation. A retry never schedules another.
            if known_front && !self.focus_retries.iter().any(|(_, b)| b == bundle) {
                self.focus_retries.push((Instant::now() + FOCUS_RETRY, bundle.to_owned()));
            }
            return;
        }
        for attr in ["AXMainWindow", "AXFocusedWindow"] {
            let Some(el) = app.element(attr) else { continue };
            let Some(id) = self.lookup(pid, &el) else { continue };
            let now = Instant::now();
            if self.last_focus.is_some_and(|(w, t)| w == id && now.duration_since(t) < Duration::from_millis(150)) {
                return;
            }
            self.last_focus = Some((id, now));
            self.emit(Event::WindowFocused { bundle_id: bundle.to_owned(), window: id });
            return;
        }
    }
}
