//! Oriel's shared app state: the [`Model`], the platform, and the single place where inputs
//! become effects. Both windows hold an `Arc<Core>`; all dispatching happens on the UI
//! thread (handlers, `AppHandle::update`, [`AppSpec::poll`]), so the model is touched
//! exactly once at a time.

use crate::geometry;
use crate::macwin;
use crate::ui::Ui;
use oriel_core::model::{Action, Effect, Input};
use oriel_core::platform::{Event, Platform};
use oriel_core::types::{Rect, TabId};
use oriel_core::{store, FakePlatform, Model, WindowId, WindowInfo, BAR_HEIGHT};
use rune::{AppHandle, ImageCache, Tx, Waker, WindowHandle};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The saved state's write gate: at most one write every [`geometry::SAVE_INTERVAL`], and
/// always one write after the last change.
#[derive(Default)]
struct SaveGate {
    dirty: bool,
    due: Option<f64>,
    last_saved: Option<f64>,
}

/// The deadlines of `Effect::ExpireTab`: one per tab, on Rune's clock (seconds). A newer
/// deadline for the same tab replaces the older one. Pure, so the timing is unit-tested.
#[derive(Default)]
struct Expiry {
    deadlines: std::collections::BTreeMap<TabId, f64>,
}

impl Expiry {
    fn schedule(&mut self, tab: TabId, now: f64, after_ms: u64) {
        self.deadlines.insert(tab, now + after_ms as f64 / 1000.0);
    }

    /// Remove and return the tabs whose deadline has come, oldest deadline first.
    fn take_due(&mut self, now: f64) -> Vec<TabId> {
        let mut due: Vec<(f64, TabId)> =
            self.deadlines.iter().filter(|(_, at)| now >= **at).map(|(t, at)| (*at, *t)).collect();
        due.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, tab) in &due {
            self.deadlines.remove(tab);
        }
        due.into_iter().map(|(_, tab)| tab).collect()
    }

    /// The soonest deadline left.
    fn next(&self) -> Option<f64> {
        self.deadlines.values().copied().min_by(f64::total_cmp)
    }
}

/// A bar drag Oriel drives itself: the pointer and the bar where the press began.
#[derive(Clone, Copy)]
struct BarDrag {
    anchor: (f64, f64),
    start: Rect,
}

/// A divider drag Oriel drives itself: the pointer where the press began, and the left pane's
/// width then, against the content width.
#[derive(Clone, Copy)]
struct DividerDrag {
    anchor_x: f64,
    start_left_w: f64,
    content_w: f64,
    /// When the last `Action::SplitRatio` went out (a step comes at most once per display frame).
    last_sent: Option<std::time::Instant>,
}

/// The shortest time between two `Action::SplitRatio` of one drag: a 120 Hz display frame.
const DIVIDER_STEP: std::time::Duration = std::time::Duration::from_micros(8000);

/// The bar stays invisible (alpha 0) at launch until the first window is in place under it,
/// so both appear together instead of the bar first and the window jumping in later.
#[derive(Default)]
struct Reveal {
    shown: bool,
    start: Option<f64>,
    raised_at: Option<f64>,
}

/// After a `RaiseBar` (the glued window was just presented), this long until the bar shows.
const REVEAL_AFTER_RAISE: f64 = 0.05;
/// The bar shows at the latest this long after launch (no window to wait for, or a slow app).
const REVEAL_AT_MOST: f64 = 0.6;
/// The bar's fade-in at launch, in seconds.
const REVEAL_FADE: f64 = 0.12;
/// Rune's own placement is brought up to date once the bar has rested this long.
const PLACE_SYNC_AFTER: f64 = 0.2;

/// Everything the two windows share: the model (behind the mutex the single dispatch path
/// locks), the platform, the icon cache and the handles the effects act on.
pub(crate) struct Core {
    platform: Arc<dyn Platform>,
    /// The fake itself, when the app runs on it, so the demo can inject its events.
    fake: Option<Arc<FakePlatform>>,
    model: Mutex<Model>,
    icons: OnceLock<ImageCache>,
    /// One waker per open window; a background thread wakes them all.
    wakers: Mutex<Vec<Waker>>,
    /// The windows' handles: an `Event` arriving on another thread hops to the UI thread
    /// through whichever of them the loop is running. Every window gets it; the
    /// once-guard in the sink keeps a single dispatch per event.
    apps: Mutex<Vec<AppHandle<Ui>>>,
    /// The bar's native window, once it exists (native runs only).
    bar: OnceLock<WindowHandle>,
    /// The picker's native window, to put it under the "+" button.
    picker_window: OnceLock<WindowHandle>,
    /// The system window background in light and dark (`macwin::window_backgrounds`).
    window_bg: OnceLock<(rune::Color, rune::Color)>,
    /// The last rect `Effect::MoveBar` set: a frame this close to it is our own echo.
    last_set: Mutex<Option<Rect>>,
    /// A `MoveBar` that arrived before the bar's window existed, applied on bind.
    deferred: Mutex<Option<Rect>>,
    /// Frame rects the AppKit move observer read while the loop could not turn.
    pending_moves: Mutex<VecDeque<Rect>>,
    /// The bar drag in progress, if any.
    drag: Mutex<Option<BarDrag>>,
    /// Where the divider of the split view is now (the last `Effect::Divider`).
    divider_rect: Mutex<Option<Rect>>,
    /// The pointer is over the divider.
    divider_hover: AtomicBool,
    /// The divider drag in progress, if any.
    divider_drag: Mutex<Option<DividerDrag>>,
    /// When the bar last moved (Oriel's clock) and the rect Rune's placement last got.
    last_move: Mutex<f64>,
    placed: Mutex<Option<Rect>>,
    /// The frame before a zoom to the full screen, to zoom back to.
    zoom_prev: Mutex<Option<Rect>>,
    reveal: Mutex<Reveal>,
    /// Rune's clock (`Tx::clock`) at an `Instant`: lets AppKit callbacks, which have no `Tx`,
    /// stamp the save gate on the same clock.
    clock: Mutex<Option<(f64, std::time::Instant)>>,
    gate: Mutex<SaveGate>,
    expiry: Mutex<Expiry>,
    path: PathBuf,
    demo: AtomicBool,
    demo_untrusted: AtomicBool,
    /// Headless scenes: twenty tabs, a hovered tab, a reorder drag, no app scan.
    pub(crate) demo_many: AtomicBool,
    pub(crate) demo_hover: AtomicBool,
    pub(crate) demo_reorder: AtomicBool,
    /// Headless scenes: a split view of two tabs, shown (`demo_split`) or left for a third tab.
    pub(crate) demo_split: AtomicBool,
    pub(crate) demo_split_away: AtomicBool,
    demo_no_scan: AtomicBool,
    started: AtomicBool,
    quitting: AtomicBool,
}

impl Core {
    pub(crate) fn new(
        model: Model,
        path: PathBuf,
        platform: Arc<dyn Platform>,
        fake: Option<Arc<FakePlatform>>,
    ) -> Arc<Core> {
        let core = Arc::new(Core {
            platform,
            fake,
            model: Mutex::new(model),
            icons: OnceLock::new(),
            wakers: Mutex::new(Vec::new()),
            apps: Mutex::new(Vec::new()),
            bar: OnceLock::new(),
            picker_window: OnceLock::new(),
            window_bg: OnceLock::new(),
            last_set: Mutex::new(None),
            deferred: Mutex::new(None),
            pending_moves: Mutex::new(VecDeque::new()),
            drag: Mutex::new(None),
            divider_rect: Mutex::new(None),
            divider_hover: AtomicBool::new(false),
            divider_drag: Mutex::new(None),
            last_move: Mutex::new(0.0),
            placed: Mutex::new(None),
            zoom_prev: Mutex::new(None),
            reveal: Mutex::new(Reveal::default()),
            clock: Mutex::new(None),
            gate: Mutex::new(SaveGate::default()),
            expiry: Mutex::new(Expiry::default()),
            path,
            demo: AtomicBool::new(false),
            demo_untrusted: AtomicBool::new(false),
            demo_many: AtomicBool::new(false),
            demo_hover: AtomicBool::new(false),
            demo_reorder: AtomicBool::new(false),
            demo_split: AtomicBool::new(false),
            demo_split_away: AtomicBool::new(false),
            demo_no_scan: AtomicBool::new(false),
            started: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
        });
        // One icon cache for both windows: a decode wakes every open window.
        let wake = core.clone();
        let _ = core.icons.set(ImageCache::new(Arc::new(move || wake.wake_windows())));
        core
    }

    /// Read the model under its lock, without a copy.
    pub(crate) fn with_model<R>(&self, f: impl FnOnce(&Model) -> R) -> R {
        f(&self.model.lock().unwrap())
    }

    /// Note Rune's clock now; see [`Core::clock`].
    pub(crate) fn observe_clock(&self, now: f64) {
        *self.clock.lock().unwrap() = Some((now, std::time::Instant::now()));
    }

    /// Rune's clock, also where no `Tx` is at hand (AppKit callbacks).
    pub(crate) fn clock(&self) -> f64 {
        self.clock.lock().unwrap().map_or(0.0, |(at, when)| at + when.elapsed().as_secs_f64())
    }

    /// The colors for one appearance, with the system window background when it is known.
    pub(crate) fn palette(&self, dark: bool) -> crate::style::Palette {
        let bg = self.window_bg.get().map(|(light, dark_bg)| if dark { *dark_bg } else { *light });
        crate::style::palette(dark, bg)
    }

    /// Read the system window background once (main thread, AppKit).
    fn load_window_background(&self) {
        if let Some((light, dark)) = macwin::window_backgrounds() {
            let color = |c: [f64; 4]| crate::style::srgb(c[0], c[1], c[2], 1.0);
            let _ = self.window_bg.set((color(light), color(dark)));
        }
    }

    /// The picker's window, to place it.
    pub(crate) fn set_picker_window(&self, window: WindowHandle) {
        let _ = self.picker_window.set(window);
    }

    /// Put the picker under the "+" button (`plus_x`, in the bar's points), inside the screen.
    pub(crate) fn place_picker(&self, plus_x: f32) {
        let Some(window) = self.picker_window.get() else { return };
        let (frame, screen) = self.with_model(|m| (m.frame, m.screen));
        let Some(bar) = self.bar_rect().or_else(|| frame.map(|f| f.bar())) else { return };
        let (w, h) = (f64::from(crate::style::PICKER_SIZE.0), f64::from(crate::style::PICKER_SIZE.1));
        let mut x = bar.x + f64::from(plus_x) - 12.0;
        let mut y = bar.y + BAR_HEIGHT + 6.0;
        if let Some(screen) = screen {
            x = x.clamp(screen.x + 8.0, (screen.x + screen.w - w - 8.0).max(screen.x + 8.0));
            y = y.clamp(screen.y, (screen.y + screen.h - h - 8.0).max(screen.y));
        }
        place(window, Rect::new(x.round(), y.round(), w, h));
    }

    /// The running apps' bundle ids, for the picker's marks (the model's own in fake runs, so
    /// headless renders never show what runs on this Mac).
    pub(crate) fn running_bundle_ids(&self) -> std::collections::HashSet<String> {
        if self.fake.is_some() {
            return self.with_model(|m| m.tabs.iter().filter(|t| m.is_running(&t.app.bundle_id)).map(|t| t.app.bundle_id.clone()).collect());
        }
        macwin::running_bundle_ids().into_iter().collect()
    }

    /// The app's icons.
    pub(crate) fn icons(&self) -> &ImageCache {
        self.icons.get().expect("icons made in Core::new")
    }

    // ---- Wiring ------------------------------------------------------------

    /// Where `Event`s arriving on other threads hop to the UI thread: windows register
    /// themselves as the loop comes up; the bar first (it exists in every render).
    pub(crate) fn add_app_handle(&self, handle: AppHandle<Ui>) {
        self.apps.lock().unwrap().push(handle);
    }

    // ---- Dispatch ----------------------------------------------------------

    /// Apply one input and perform its effects, in order. Call on the UI thread only.
    pub(crate) fn dispatch(&self, input: Input, now: f64) {
        let effects = self.model.lock().unwrap().handle(input);
        for effect in effects {
            self.perform(effect, now);
        }
    }

    /// The platform's events, hopped to the UI thread by `AppHandle::update`.
    pub(crate) fn dispatch_event(&self, event: Event, now: f64) {
        if crate::debug::marks_on() {
            let name = format!("{event:?}");
            crate::debug::mark(&format!("event {}", name.split(['(', ' ', '{']).next().unwrap_or("")));
        }
        self.dispatch(Input::Event(event), now);
    }

    fn perform(&self, effect: Effect, now: f64) {
        match effect {
            Effect::Platform(command) => {
                if crate::debug::marks_on() && matches!(command, oriel_core::Command::Present { .. }) {
                    crate::debug::mark("present");
                }
                if crate::debug::marks_on() && std::env::var_os("ORIEL_DEBUG_COMMANDS").is_some() {
                    crate::debug::mark(&format!("cmd {command:?}"));
                }
                self.platform.send(command)
            }
            Effect::MoveBar(rect) => self.move_bar(&rect),
            Effect::Divider(rect) => self.show_divider(rect),
            // Bring the bar in front without taking keyboard focus: directly above the glued
            // window, else at the front of its level (`macwin::raise_bar`). Rune's own
            // `AppHandle::show` does nothing for a bar that is already visible.
            Effect::RaiseBar => {
                {
                    let mut reveal = self.reveal.lock().unwrap();
                    if reveal.raised_at.is_none() {
                        reveal.raised_at = Some(now);
                    }
                }
                // The glued windows: the active one, and the other pane of a shown split.
                let glued: Vec<u32> = match crate::debug::glued_override() {
                    Some(id) => vec![id],
                    None => self.with_model(|m| {
                        let tab_window = |id| m.tab(id).and_then(|t| t.window.as_ref()).map(|w| w.id.0);
                        match m.shown_split() {
                            Some(split) => [split.left, split.right].into_iter().filter_map(tab_window).collect(),
                            None => m.active.and_then(tab_window).into_iter().collect(),
                        }
                    }),
                };
                let raised = self.bar.get().is_some_and(|bar| {
                    let raised = macwin::raise_bar(bar, &glued);
                    // The divider ends above the bar, so above both windows.
                    macwin::divider_raise(bar);
                    raised
                });
                if !raised && let Some(app) = self.apps.lock().unwrap().first() {
                    app.show();
                }
            }
            Effect::Save => self.mark_save(now),
            Effect::ExpireTab { tab, after_ms } => {
                self.expiry.lock().unwrap().schedule(tab, now, after_ms);
            }
        }
    }

    /// Move and resize the bar window to `rect` (points, top-left origin) and remember it,
    /// so `Action::BarMoved` never echoes a move Oriel made itself. Before the window is
    /// bound, the rect waits; `bind_bar_window` applies the last one.
    ///
    /// Placement goes through Rune's `WindowHandle::place` with an exact `Rect` on the
    /// primary display: the same overlay machinery Rune positions its own panels with,
    /// which reaches the real AppKit window and keeps its companion panel's origin in
    /// sync. Going below it (winit `set_outer_position`, or the panel's frame directly)
    /// lands 8 pt off — AppKit's frame keeps chrome Oriel does not draw — and reads to
    /// the move observer as the user's own move.
    fn move_bar(&self, rect: &Rect) {
        if self.bar.get().is_none() {
            *self.deferred.lock().unwrap() = Some(*rect);
            return;
        }
        self.set_bar(*rect);
    }

    /// Put the bar at `rect` now. On macOS the panel's frame is set directly (synchronously,
    /// the same `setFrame:` Rune's `place` ends in, one loop turn sooner); Rune's own
    /// placement is brought up to date once the bar rests ([`Core::sync_placement`]).
    fn set_bar(&self, rect: Rect) {
        let Some(bar) = self.bar.get() else { return };
        // An exact 40 pt strip, whatever the model's rect was tall: minimum sizes never
        // fight a small bar, and the echo filter compares like with like.
        let rect = Rect::new(rect.x, rect.y, rect.w, BAR_HEIGHT);
        *self.last_set.lock().unwrap() = Some(rect);
        *self.last_move.lock().unwrap() = self.clock();
        if !macwin::set_bar_frame(bar, rect) {
            place(bar, rect);
            *self.placed.lock().unwrap() = Some(rect);
        }
    }

    /// Give Rune's placement the bar's rect once the bar has rested, so a later re-apply of
    /// Rune's stored placement (a display change) never moves the bar back. `place` compares
    /// frames first, so this moves nothing on screen.
    pub(crate) fn sync_placement(&self, now: f64, tx: &mut Tx) {
        let Some(bar) = self.bar.get() else { return };
        let Some(rect) = *self.last_set.lock().unwrap() else { return };
        if self.placed.lock().unwrap().is_some_and(|p| p == rect) || self.drag.lock().unwrap().is_some() {
            return;
        }
        let due = *self.last_move.lock().unwrap() + PLACE_SYNC_AFTER;
        if now < due {
            tx.wake_at(due);
            return;
        }
        place(bar, rect);
        *self.placed.lock().unwrap() = Some(rect);
    }

    // ---- The divider of the split view ---------------------------------------

    /// `Effect::Divider`: put the divider window in the gap between the two panes, or hide it.
    /// Without a bar window (a headless run) there is nothing to show.
    fn show_divider(&self, rect: Option<Rect>) {
        *self.divider_rect.lock().unwrap() = rect;
        if crate::debug::marks_on() {
            crate::debug::mark(&format!("divider {rect:?}"));
        }
        let Some(bar) = self.bar.get() else { return };
        if rect.is_some() {
            self.apply_divider_look();
        }
        macwin::divider_set(bar, rect);
        if rect.is_some() {
            macwin::divider_raise(bar);
        }
        if crate::debug::marks_on() {
            crate::debug::mark(&macwin::divider_debug(bar).unwrap_or_else(|| "divider-window none".into()));
        }
    }

    /// The divider's look: quiet at rest, stronger under the pointer and while dragged.
    fn apply_divider_look(&self) {
        let p = self.palette(macwin::is_dark());
        let strong = self.divider_hover.load(Ordering::Acquire) || self.divider_drag.lock().unwrap().is_some();
        let (line, width) =
            if strong { (p.text_secondary.alpha(0.55), 3.0) } else { (p.separator, 1.0) };
        macwin::divider_look(macwin::DividerLook {
            gap: crate::style::to_srgb(p.strip),
            line: crate::style::to_srgb(line),
            line_width: width,
        });
    }

    /// The pointer entered or left the divider.
    pub(crate) fn divider_hovered(&self, inside: bool) {
        self.divider_hover.store(inside, Ordering::Release);
        self.apply_divider_look();
    }

    /// A press on the divider: from now on the left pane follows the pointer, on every drag event
    /// (the same AppKit event monitor as the bar drag), but at most once per display frame.
    pub(crate) fn begin_divider_drag(self: &Arc<Self>) {
        let Some((px, _)) = macwin::pointer() else { return };
        let Some(divider) = *self.divider_rect.lock().unwrap() else { return };
        let Some(content) = self.with_model(|m| m.frame.map(|f| f.content())) else { return };
        *self.divider_drag.lock().unwrap() = Some(DividerDrag {
            anchor_x: px,
            start_left_w: divider.x - content.x,
            content_w: content.w,
            last_sent: None,
        });
        self.apply_divider_look();
        macwin::divider_cursor();
        let (on_move, on_up) = (self.clone(), self.clone());
        macwin::start_drag_monitor(
            Box::new(move || on_move.follow_divider(false)),
            Box::new(move || on_up.end_divider_drag()),
        );
    }

    /// Send the ratio the pointer asks for now. `last` (the release) is never held back.
    pub(crate) fn follow_divider(&self, last: bool) {
        let Some(mut drag) = *self.divider_drag.lock().unwrap() else { return };
        let Some((px, _)) = macwin::pointer() else { return };
        if !last && drag.last_sent.is_some_and(|at| at.elapsed() < DIVIDER_STEP) {
            return;
        }
        macwin::divider_cursor();
        let left_w = drag.start_left_w + (px - drag.anchor_x);
        let ratio = oriel_core::ratio_for_left(drag.content_w, left_w);
        drag.last_sent = Some(std::time::Instant::now());
        if let Some(slot) = self.divider_drag.lock().unwrap().as_mut() {
            slot.last_sent = drag.last_sent;
        }
        self.dispatch(Input::Action(Action::SplitRatio(ratio)), self.clock());
    }

    /// The pointer came up: the drag is over.
    pub(crate) fn end_divider_drag(&self) {
        if self.divider_drag.lock().unwrap().is_none() {
            return;
        }
        self.follow_divider(true);
        *self.divider_drag.lock().unwrap() = None;
        self.apply_divider_look();
        dispatch_main(macwin::stop_drag_monitor);
        self.wake_windows();
    }

    // ---- Dragging the bar --------------------------------------------------

    /// A press on the bar's empty space: from now on the bar follows the pointer, moved by
    /// Oriel itself on every pointer event (not by the window server's native drag, which
    /// tells the app where the bar went only through notifications the loop drains late).
    /// Each step moves the bar's own window at once and sends the glued window's frame to
    /// the platform's frame mover in the same turn.
    pub(crate) fn begin_bar_drag(self: &Arc<Self>) {
        let Some(anchor) = macwin::pointer() else { return };
        let Some(start) = self.bar_rect() else { return };
        *self.drag.lock().unwrap() = Some(BarDrag { anchor, start });
        let (on_move, on_up) = (self.clone(), self.clone());
        macwin::start_drag_monitor(
            Box::new(move || on_move.follow_pointer()),
            Box::new(move || on_up.end_bar_drag()),
        );
    }

    /// Move the bar to where the pointer is now, relative to where the drag began.
    pub(crate) fn follow_pointer(&self) {
        let Some(drag) = *self.drag.lock().unwrap() else { return };
        let Some((px, py)) = macwin::pointer() else { return };
        let rect = Rect::new(
            drag.start.x + (px - drag.anchor.0).round(),
            drag.start.y + (py - drag.anchor.1).round(),
            drag.start.w,
            BAR_HEIGHT,
        );
        self.drag_bar_to(self.keep_on_screen(rect));
    }

    /// The pointer came up: the drag is over. The platform's mover reads the window back once
    /// on its own (its burst settles), and the save gate writes once.
    pub(crate) fn end_bar_drag(&self) {
        if self.drag.lock().unwrap().is_none() {
            return;
        }
        self.follow_pointer();
        *self.drag.lock().unwrap() = None;
        // Not from inside the monitor's own handler: remove it on the next turn.
        dispatch_main(macwin::stop_drag_monitor);
        self.wake_windows();
    }

    /// One drag step: the bar's window now, the model (which sends the glued window's
    /// `SetFrame`) in the same turn.
    pub(crate) fn drag_bar_to(&self, rect: Rect) {
        let now = self.clock();
        self.set_bar(rect);
        self.dispatch(Input::Action(Action::BarMoved(rect)), now);
    }

    /// The bar's rect now: where Oriel last put it, else the model's.
    fn bar_rect(&self) -> Option<Rect> {
        let set = *self.last_set.lock().unwrap();
        set.or_else(|| self.with_model(|m| m.frame.map(|f| f.bar())))
    }

    /// Keep the bar under the menu bar and above the bottom of the screen, as the window
    /// server keeps a titled window.
    fn keep_on_screen(&self, rect: Rect) -> Rect {
        let Some(screen) = self.with_model(|m| m.screen) else { return rect };
        let top = screen.y;
        let bottom = (screen.y + screen.h - BAR_HEIGHT).max(top);
        Rect::new(rect.x, rect.y.clamp(top, bottom), rect.w, rect.h)
    }

    /// A double click on the bar's empty space: zoom the whole Oriel frame to the visible
    /// screen, or back to the frame before.
    pub(crate) fn toggle_zoom(&self, now: f64) {
        let (frame, screen) = self.with_model(|m| (m.frame, m.screen));
        let (Some(frame), Some(screen)) = (frame, screen) else { return };
        let target = if frame.close_to(&screen) {
            match self.zoom_prev.lock().unwrap().take() {
                Some(previous) => previous,
                None => return,
            }
        } else {
            *self.zoom_prev.lock().unwrap() = Some(frame);
            screen
        };
        self.dispatch(Input::Action(Action::Zoom(target)), now);
    }

    /// Debug hook (`ORIEL_DEBUG_SCRIPT` `drag`): drive the bar drag along x, back and forth
    /// at `speed` pt/s over `span` pt for `secs`, one step per 120 Hz timer tick. Each step
    /// runs [`Core::drag_bar_to`], as a pointer event does.
    pub(crate) fn debug_drag(self: &Arc<Self>, speed: f64, span: f64, secs: f64, end: (f64, f64)) {
        let Some(start) = self.bar_rect() else { return };
        let core = self.clone();
        let t0 = std::time::Instant::now();
        *self.drag.lock().unwrap() = Some(BarDrag { anchor: (0.0, 0.0), start });
        macwin::every_frame(Box::new(move || {
            let t = t0.elapsed().as_secs_f64();
            let done = t > secs;
            let phase = (speed * t) % (2.0 * span);
            let dx = if done { end.0 } else if phase < span { phase } else { 2.0 * span - phase };
            let dy = if done { end.1 } else { 0.0 };
            let rect = Rect::new(start.x + dx.round(), start.y + dy.round(), start.w, BAR_HEIGHT);
            core.drag_bar_to(core.keep_on_screen(rect));
            if done {
                *core.drag.lock().unwrap() = None;
                crate::debug::mark("drag-end");
                if let Some(bar) = core.bar.get() {
                    crate::debug::mark(&macwin::divider_debug(bar).unwrap_or_default());
                }
                core.wake_windows();
            }
            !done
        }));
    }

    // ---- The launch reveal ---------------------------------------------------

    /// Show the bar once the first window is in place, or at the latest after
    /// [`REVEAL_AT_MOST`]. Called from the bar's `poll`.
    pub(crate) fn tick_reveal(&self, now: f64, tx: &mut Tx) {
        let Some(bar) = self.bar.get() else { return };
        let mut reveal = self.reveal.lock().unwrap();
        if reveal.shown {
            return;
        }
        let start = *reveal.start.get_or_insert(now);
        let mut due = start + REVEAL_AT_MOST;
        if let Some(raised) = reveal.raised_at {
            due = due.min(raised + REVEAL_AFTER_RAISE);
        }
        if now >= due {
            reveal.shown = true;
            crate::debug::mark("bar-reveal");
            if crate::debug::marks_on() {
                crate::debug::mark(&format!("bar-collection-behavior later {:?}", macwin::bar_collection_behavior(bar)));
            }
            macwin::set_bar_alpha(bar, 1.0, REVEAL_FADE);
        } else {
            tx.wake_at(due);
        }
    }


    /// Dispatch `Action::ExpireTab` for every tab whose grace has run out, and ask to be woken
    /// at the next deadline. Called from the bar's `poll`. `true`: a tab may have gone.
    pub(crate) fn tick_expiry(&self, now: f64, tx: &mut Tx) -> bool {
        let due = self.expiry.lock().unwrap().take_due(now);
        for tab in &due {
            self.dispatch(Input::Action(Action::ExpireTab(*tab)), now);
        }
        if let Some(next) = self.expiry.lock().unwrap().next() {
            tx.wake_at(next);
        }
        !due.is_empty()
    }

    fn mark_save(&self, now: f64) {
        let mut gate = self.gate.lock().unwrap();
        if !gate.dirty {
            let due = now + geometry::SAVE_INTERVAL;
            let floor = gate.last_saved.map_or(0.0, |at| at + geometry::SAVE_INTERVAL);
            gate.due = Some(due.max(floor));
        }
        gate.dirty = true;
    }

    /// Write the saved state once its wait has passed; otherwise ask to be woken then.
    /// `false`: writing alone changes nothing the view reads.
    pub(crate) fn flush_saves(&self, now: f64, tx: &mut Tx) -> bool {
        let mut gate = self.gate.lock().unwrap();
        if !gate.dirty {
            return false;
        }
        if gate.due.is_some_and(|due| now >= due) {
            let saved = self.model.lock().unwrap().saved();
            if let Err(error) = store::save(&self.path, &saved) {
                eprintln!("oriel: could not save {}: {error}", self.path.display());
            }
            gate.dirty = false;
            gate.last_saved = Some(now);
            gate.due = None;
        } else if let Some(due) = gate.due {
            tx.wake_at(due);
        }
        false
    }

    // ---- The bar window ----------------------------------------------------

    /// The bar's native window exists (native runs only): keep it, watch it move, route the
    /// menu's Quit item, and apply any `MoveBar` that arrived first. Called from
    /// `WindowSpec::on_created`, with the core it shares.
    pub(crate) fn bind_bar_window(self: &Arc<Self>, window: &WindowHandle) {
        let _ = self.bar.set(window.clone());
        // AppKit move/resize notifications push rects; poll drains them into the model.
        let core = self.clone();
        macwin::watch_moves(window, Box::new(move |rect| core.note_appkit_move(rect)));
        // winit's default menu has Quit ⌘Q as `terminate:`, which exits without running
        // `Action::Quit`'s un-hiding; point it at Oriel's own quit path: the bar's own
        // runtime runs it as an update.
        if let Some(app) = self.apps.lock().unwrap().first().cloned() {
            macwin::route_menu_quit(Box::new(move || {
                app.update(|ui: &mut Ui, tx: &mut Tx| ui.quit(tx));
            }));
        }
        // Invisible until the first window is in place (`tick_reveal`); at its saved place
        // from the first frame, so it never animates in from a default spot.
        self.load_window_background();
        macwin::no_appear_animation(window);
        // The divider of the split view: a hidden child window of the bar until a split shows.
        let (hover_core, press_core) = (self.clone(), self.clone());
        macwin::divider_install(
            window,
            macwin::DividerHooks {
                hover: Box::new(move |inside| hover_core.divider_hovered(inside)),
                press: Box::new(move || press_core.begin_divider_drag()),
            },
        );
        let pending = *self.divider_rect.lock().unwrap();
        if pending.is_some() {
            self.show_divider(pending);
        }
        // Gone in Mission Control and App Exposé: they show only the glued app window.
        let mask = macwin::hide_in_mission_control(window);
        crate::debug::mark(&format!("bar-collection-behavior set {mask:?}"));
        macwin::set_bar_alpha(window, 0.0, 0.0);
        let rect = self.deferred.lock().unwrap().take().or_else(|| self.with_model(|m| m.frame.map(|f| f.bar())));
        if let Some(rect) = rect {
            self.move_bar(&rect);
        }
        window.show();
    }

    /// An AppKit notification saw the bar's frame change just now.
    pub(crate) fn note_appkit_move(&self, rect: Rect) {
        if self.is_own_move(&rect) {
            return;
        }
        self.pending_moves.lock().unwrap().push_back(rect);
        self.wake_windows();
    }

    fn is_own_move(&self, rect: &Rect) -> bool {
        if self.drag.lock().unwrap().is_some() {
            return true;
        }
        self.last_set.lock().unwrap().is_some_and(|set| set.close_to(rect))
    }

    /// Track the bar's frame for moves Oriel did not make (the system moved the window, for
    /// example after a display change): the AppKit observer queues them and the poll sends
    /// the newest one to the model. Oriel's own moves (drag, `MoveBar`) never get here:
    /// their notifications read the rect Oriel set last.
    pub(crate) fn track_bar(&self, now: f64) -> bool {
        let latest = self.pending_moves.lock().unwrap().drain(..).next_back();
        let Some(rect) = latest else { return false };
        if self.is_own_move(&rect) || self.drag.lock().unwrap().is_some() {
            return false;
        }
        *self.last_set.lock().unwrap() = Some(rect);
        *self.placed.lock().unwrap() = None;
        self.dispatch(Input::Action(Action::BarMoved(rect)), now);
        true
    }

    // ---- The platform ------------------------------------------------------

    /// Start the platform once, with the sink that hops its events (any thread) to the UI
    /// thread through `AppHandle::update`. Each event goes to every registered window and
    /// is dispatched once, by whichever window's loop drains it first.
    pub(crate) fn start_platform(&self) {
        if self.started.swap(true, Ordering::AcqRel) {
            return;
        }
        let apps = self.apps.lock().unwrap().clone();
        if apps.is_empty() {
            eprintln!("oriel: no window was registered before the platform started");
            return;
        }
        let sink: oriel_core::EventSink = Box::new(move |event| {
            let spent = Arc::new(AtomicBool::new(false));
            for app in &apps {
                let (spent, event) = (spent.clone(), event.clone());
                app.update(move |ui: &mut Ui, tx: &mut Tx| {
                    if spent.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    ui.core.dispatch_event(event, tx.clock());
                });
            }
        });
        self.platform.start(sink);
    }

    /// Seed the fake-scene demo once: apps with windows, so headless runs render a full bar.
    pub(crate) fn seed_demo(&self, now: f64) -> bool {
        if self.fake.is_none() || self.demo.swap(true, Ordering::AcqRel) {
            return false;
        }
        let apps = demo_apps();
        let platform = self.fake.as_ref().unwrap();
        let first = |name: &str| apps.iter().find(|a| a.name == name).cloned().expect("demo app");
        let [safari, notes, music, mail] = ["Safari", "Notes", "Music", "Mail"].map(first);
        for app in [&safari, &notes, &music, &mail] {
            self.dispatch(Input::Action(Action::AddApp(app.clone())), now);
        }
        // Safari runs with two windows: two tabs, the first active.
        platform.emit(Event::AppRunning {
            bundle_id: safari.bundle_id.clone(),
            windows: windows(&[(101, "Rune — a native UI framework"), (102, "Weekly Plan — Notes")]),
        });
        // Notes runs with one window.
        platform.emit(Event::AppRunning { bundle_id: notes.bundle_id.clone(), windows: windows(&[(103, "Trip to Lisbon")]) });
        // Music runs with no standard window: its tab stays a placeholder. Mail does not run.
        platform.emit(Event::AppRunning { bundle_id: music.bundle_id.clone(), windows: Vec::new() });
        if self.demo_many.load(Ordering::Acquire) {
            // Sixteen more apps, most of them running with a window: twenty tabs.
            let mut id = 200;
            for (i, app) in apps.iter().filter(|a| ![&safari, &notes, &music, &mail].contains(a)).take(16).enumerate() {
                self.dispatch(Input::Action(Action::AddApp(app.clone())), now);
                if i % 5 != 4 {
                    id += 1;
                    platform.emit(Event::AppRunning { bundle_id: app.bundle_id.clone(), windows: windows(&[(id, &app.name)]) });
                }
            }
        }
        if !self.demo_no_scan.load(Ordering::Acquire) {
            platform.emit(Event::AppsScanned(apps));
        }
        // Bring the first window to the front, as activity there would.
        platform.emit(Event::WindowFocused { bundle_id: safari.bundle_id.clone(), window: WindowId(101) });
        if self.demo_untrusted.load(Ordering::Acquire) {
            self.set_trusted(false);
        }
        true
    }

    /// `--scene loading` (picker): the app scan never answers.
    pub(crate) fn demo_no_scan(&self) {
        self.demo_no_scan.store(true, Ordering::Release);
    }

    /// `--scene many` (bar): twenty tabs.
    pub(crate) fn demo_many(&self) {
        self.demo_many.store(true, Ordering::Release);
    }

    /// `--scene hover` (bar): the pointer on a tab.
    pub(crate) fn demo_hover(&self) {
        self.demo_hover.store(true, Ordering::Release);
    }

    /// `--scene split` / `split-away` (bar): two tabs joined, shown or with a third tab active.
    pub(crate) fn demo_split(&self, away: bool) {
        self.demo_split.store(!away, Ordering::Release);
        self.demo_split_away.store(away, Ordering::Release);
    }

    /// `--scene reorder` (bar): a tab dragged one place to the right.
    pub(crate) fn demo_reorder(&self) {
        self.demo_reorder.store(true, Ordering::Release);
    }

    /// The "needs Accessibility" screenshot: pretend permission is missing once seeded.
    pub(crate) fn demo_untrusted(&self) {
        self.demo_untrusted.store(true, Ordering::Release);
        if self.demo.load(Ordering::Acquire) {
            self.set_trusted(false);
        }
    }

    fn set_trusted(&self, trusted: bool) {
        self.model.lock().unwrap().trusted = trusted;
    }

    /// The one quit path: perform `Action::Quit`'s effects (un-hide the managed apps, save)
    /// exactly once and end the loop. `on_close`, ⌘Q and the routed menu Quit all use it.
    pub(crate) fn quit(&self, tx: &mut Tx) {
        if !self.quitting.swap(true, Ordering::AcqRel) {
            let now = tx.clock();
            self.dispatch(Input::Action(Action::Quit), now);
            // `ShowApp` must run before the process exits.
            self.platform.flush(std::time::Duration::from_millis(800));
            // The exit's own save: no wake_at will fire another poll.
            let saved = self.model.lock().unwrap().saved();
            if let Err(error) = store::save(&self.path, &saved) {
                eprintln!("oriel: could not save {}: {error}", self.path.display());
            }
        }
        tx.quit();
    }

    pub(crate) fn add_waker(&self, waker: Waker) {
        self.wakers.lock().unwrap().push(waker);
    }

    /// Wake every open window, so the loop runs a turn (and, with it, `poll`).
    pub(crate) fn wake_windows(&self) {
        for waker in self.wakers.lock().unwrap().iter() {
            waker();
        }
    }
}

/// `[(window id, title)]` → the platform's window descriptions.
fn windows(sources: &[(u32, &str)]) -> Vec<WindowInfo> {
    sources
        .iter()
        .map(|&(id, title)| WindowInfo { id: WindowId(id), title: title.to_string() })
        .collect()
}

/// Rune's placement of the bar window: an exact rect on the primary display.
fn place(bar: &WindowHandle, rect: Rect) {
    bar.place(
        rune::DisplayTarget::Primary,
        rune::OverlayPlacement::Rect { x: rect.x as f32, y: rect.y as f32, width: rect.w as f32, height: rect.h as f32 },
    );
}

/// Run `f` on the main queue, on a later turn.
fn dispatch_main(f: fn()) {
    #[cfg(target_os = "macos")]
    dispatch2::DispatchQueue::main().exec_async(f);
    #[cfg(not(target_os = "macos"))]
    f();
}

/// The demo's installed apps: real system apps, so headless renders show real icons.
fn demo_apps() -> Vec<oriel_core::AppInfo> {
    let mut apps: Vec<oriel_core::AppInfo> = [
        ("com.apple.Safari", "Safari", "/Applications/Safari.app"),
        ("com.apple.Notes", "Notes", "/System/Applications/Notes.app"),
        ("com.apple.Music", "Music", "/System/Applications/Music.app"),
        ("com.apple.mail", "Mail", "/System/Applications/Mail.app"),
        ("com.apple.iCal", "Calendar", "/System/Applications/Calendar.app"),
        ("com.apple.calculator", "Calculator", "/System/Applications/Calculator.app"),
        ("com.apple.TextEdit", "TextEdit", "/System/Applications/TextEdit.app"),
        ("com.apple.Preview", "Preview", "/System/Applications/Preview.app"),
        ("com.apple.Maps", "Maps", "/System/Applications/Maps.app"),
        ("com.apple.Photos", "Photos", "/System/Applications/Photos.app"),
        ("com.apple.reminders", "Reminders", "/System/Applications/Reminders.app"),
        ("com.apple.Terminal", "Terminal", "/System/Applications/Utilities/Terminal.app"),
        ("com.apple.systempreferences", "System Settings", "/System/Applications/System Settings.app"),
        ("com.apple.AppStore", "App Store", "/System/Applications/App Store.app"),
        ("com.apple.FaceTime", "FaceTime", "/System/Applications/FaceTime.app"),
        ("com.apple.MobileSMS", "Messages", "/System/Applications/Messages.app"),
        ("com.apple.podcasts", "Podcasts", "/System/Applications/Podcasts.app"),
        ("com.apple.freeform", "Freeform", "/System/Applications/Freeform.app"),
        ("com.apple.Home", "Home", "/System/Applications/Home.app"),
        ("com.apple.weather", "Weather", "/System/Applications/Weather.app"),
        ("com.apple.ActivityMonitor", "Activity Monitor", "/System/Applications/Utilities/Activity Monitor.app"),
        ("com.apple.Dictionary", "Dictionary", "/System/Applications/Dictionary.app"),
    ]
    .into_iter()
    .map(|(bundle_id, name, path)| oriel_core::AppInfo { bundle_id: bundle_id.into(), name: name.into(), path: path.into() })
    .collect();
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deadline_fires_after_its_delay_and_only_once() {
        let mut e = Expiry::default();
        e.schedule(TabId(1), 10.0, 4000);
        assert_eq!(e.next(), Some(14.0));
        assert!(e.take_due(13.9).is_empty());
        assert_eq!(e.take_due(14.0), vec![TabId(1)]);
        assert!(e.take_due(20.0).is_empty());
        assert_eq!(e.next(), None);
    }

    #[test]
    fn a_newer_deadline_for_the_same_tab_replaces_the_older() {
        let mut e = Expiry::default();
        e.schedule(TabId(1), 10.0, 4000);
        e.schedule(TabId(1), 12.0, 4000);
        assert!(e.take_due(14.5).is_empty());
        assert_eq!(e.take_due(16.0), vec![TabId(1)]);
    }

    #[test]
    fn due_tabs_come_out_in_deadline_order() {
        let mut e = Expiry::default();
        e.schedule(TabId(2), 1.0, 4000);
        e.schedule(TabId(1), 2.0, 4000);
        assert_eq!(e.take_due(7.0), vec![TabId(2), TabId(1)]);
    }
}
