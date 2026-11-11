//! A small AppKit bridge for the two things Rune's public API leaves indirect:
//!
//! - move notifications for the bar's own window (`NSWindowDidMove`, `NSWindowDidResize`),
//!   so a drag becomes `Action::BarMoved` frames while it happens;
//! - the default menu's Quit item (winit installs it with the `terminate:` action), which
//!   would end the process without running `Action::Quit`'s effects; it is retargeted to
//!   run Oriel's quit.
//!
//! Everything here runs on the main thread. The observer tokens and the routing target
//! live in thread-locals for the process lifetime, so nothing needs `Send` and never
//! drops mid-run. Off macOS both functions are no-ops (`main.rs` swaps in a stub).

use crate::geometry;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, class, define_class, msg_send, sel};
use objc2_foundation::{
    NSArray, NSNotification, NSNotificationCenter, NSRect, NSObjectProtocol, NSString,
};
use oriel_core::Rect;
use std::cell::RefCell;
use std::ptr::NonNull;

thread_local! {
    /// The bar's move watchers: their observer tokens (the loop ends with the process
    /// before anything here must come off).
    static MOVE_TOKENS: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
        const { RefCell::new(Vec::new()) };
}

/// Watch `window` (Rune's handle of the bar's native window) and call `on_move` with the
/// window's new Oriel-space frame on every move or resize notification. The callback runs
/// on the main thread, where AppKit posts. Returns whether the observers went in.
pub(crate) fn watch_moves(window: &rune::WindowHandle, on_move: Box<dyn Fn(Rect)>) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    if !MOVE_TOKENS.with(|tokens| tokens.borrow().is_empty()) {
        return true; // already watching the bar; a recreated window keeps its observers
    }

    // The raw handle carries the NSView; the notifications belong to its NSWindow.
    // SAFETY: raw_handle borrows the view only for this call; `retain` takes a reference we
    // keep (along with the observers) for the run. Main thread, as `with_raw_handle` asks.
    let Some(ns_view) = raw_ns_view(window) else { return false };
    // SAFETY: `-[NSView window]` on the main thread; NSView has no window before
    // it is attached, hence the Option.
    let ns_window: Option<Retained<AnyObject>> = unsafe { msg_send![&*ns_view, window] };
    let Some(ns_window) = ns_window else { return false };

    let center = NSNotificationCenter::defaultCenter();
    // Main-thread only: the notification center is documented to post the block on the
    // thread that posts, and AppKit moves the window on the main thread.
    let on_move = std::rc::Rc::new(on_move);
    let mut tokens = Vec::new();
    for name in ["NSWindowDidMoveNotification", "NSWindowDidResizeNotification"] {
        let on_move = on_move.clone();
        let window = ns_window.clone();
        // The block the notification center keeps; it reads the window's frame and calls
        // back with the rect in Oriel's coordinate space (points, top-left origin).
        let block: block2::RcBlock<dyn Fn(NonNull<NSNotification>)> = block2::RcBlock::new(
            move |_note: NonNull<NSNotification>| {
                if let Some(rect) = frame(&window) {
                    on_move(rect);
                }
            },
        );
        // SAFETY: main thread; `ns_window` is retained for the process run; `queue: None`
        // runs the block on the posting thread, which AppKit keeps the main thread for.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(&NSString::from_str(name)),
                Some(&*ns_window),
                None,
                &block,
            )
        };
        tokens.push(token);
    }
    MOVE_TOKENS.with(|slot| slot.borrow_mut().extend(tokens));
    true
}

/// The window's frame in Oriel's coordinate space: points, top-left origin of the primary
/// display, folding AppKit's bottom-left origin away with the primary screen's height.
///
/// Frames observed while the window carries a `Titled` style bit are dropped: the bar is
/// borderless for its whole life, and it only gains that bit in the transitional states
/// other tooling puts it through (winit's `is_maximized` check masks in `Titled|Resizable`
/// for a moment, which also posts resize notifications of a 32 pt taller frame). Those
/// frames would read as the user's move.
fn frame(window: &AnyObject) -> Option<Rect> {
    // SAFETY: `-[NSWindow styleMask]` and `-[NSWindow frame]` on the main thread.
    let style: u64 = unsafe { msg_send![window, styleMask] };
    if style & TITLED != 0 {
        return None;
    }
    let rect: NSRect = unsafe { msg_send![window, frame] };
    let height = primary_screen_height()?;
    Some(geometry::appkit_to_rect(
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
        height,
    ))
}

/// `NSWindowStyleMask::Titled`, the bit the bar never carries except mid-transition.
const TITLED: u64 = 0x1;

/// Put the bar in front without key focus and without activating Oriel.
///
/// Rune's `AppHandle::show` is no use for this: the bar's view lives in a `RuneOverlayPanel`
/// and a reveal of an already-visible panel orders nothing in (`Effect::OrderIn` runs only
/// `if !panel.isVisible()`). The view's own window is the panel, so it is the one to order.
///
/// With `above` (the glued windows' `CGWindowID`s: one, or two in a split view) the bar goes
/// directly above each of them that is on screen, one after the other, so it ends above all.
/// It stays behind any app window that is in front of them. When none of them is on screen
/// (or `above` is empty), the bar goes to the front of its level. Both calls leave the key
/// window and the active app alone.
pub(crate) fn raise_bar(window: &rune::WindowHandle, above: &[u32]) -> bool {
    let Some(ns_view) = raw_ns_view(window) else { return false };
    // SAFETY: main thread (`raw_ns_view` checked); `-[NSView window]` is an Option.
    let ns_window: Option<Retained<AnyObject>> = unsafe { msg_send![&*ns_view, window] };
    let Some(ns_window) = ns_window else { return false };
    let visible: Vec<u32> = above.iter().copied().filter(|id| on_screen(*id)).collect();
    if visible.is_empty() {
        let _: () = unsafe { msg_send![&*ns_window, orderFrontRegardless] };
    }
    for id in visible {
        // NSWindowAbove = 1. The ordering is within the bar's own window level.
        let _: () = unsafe { msg_send![&*ns_window, orderWindow: 1isize, relativeTo: id as isize] };
    }
    true
}

/// The bar's AppKit window (the panel Rune moved the view into), retained.
fn bar_window(window: &rune::WindowHandle) -> Option<Retained<AnyObject>> {
    let ns_view = raw_ns_view(window)?;
    // SAFETY: main thread (`raw_ns_view` checked); `-[NSView window]` is an Option.
    unsafe { msg_send![&*ns_view, window] }
}

/// Move and resize the bar's window to `rect` (Oriel points) at once, on this thread.
///
/// Rune's `WindowHandle::place` ends in the same `setFrame:` on the same panel, with the same
/// flip around the primary screen's height, but one event-loop turn later. During a drag that
/// turn is a visible lag, so the bar is set here directly. Returns false off the main thread
/// or before the window exists.
pub(crate) fn set_bar_frame(window: &rune::WindowHandle, rect: Rect) -> bool {
    let Some(ns_window) = bar_window(window) else { return false };
    let Some(height) = primary_screen_height() else { return false };
    let target = NSRect::new(
        objc2_foundation::NSPoint::new(rect.x, height - rect.y - rect.h),
        objc2_foundation::NSSize::new(rect.w, rect.h),
    );
    // SAFETY: main thread; plain frame read and set on a live window.
    let current: NSRect = unsafe { msg_send![&*ns_window, frame] };
    if current == target {
        return true;
    }
    if current.size == target.size {
        let _: () = unsafe { msg_send![&*ns_window, setFrameOrigin: target.origin] };
    } else {
        let _: () = unsafe { msg_send![&*ns_window, setFrame: target, display: true] };
    }
    true
}

/// The pointer in Oriel points (top-left origin of the primary display).
pub(crate) fn pointer() -> Option<(f64, f64)> {
    MainThreadMarker::new()?;
    let height = primary_screen_height()?;
    // SAFETY: `+[NSEvent mouseLocation]` is a plain class read.
    let p: objc2_foundation::NSPoint = unsafe { msg_send![class!(NSEvent), mouseLocation] };
    Some((p.x, height - p.y))
}

thread_local! {
    /// The local event monitor of a bar drag, while one runs.
    static DRAG_MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

/// Follow the pointer for a bar drag: `on_move` runs for every left-button drag event the
/// app receives and `on_up` once at the release, both on the main thread as AppKit
/// dispatches the event (before Rune sees it), so the bar moves the moment the pointer does.
pub(crate) fn start_drag_monitor(on_move: Box<dyn Fn()>, on_up: Box<dyn Fn()>) -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    stop_drag_monitor();
    const LEFT_MOUSE_UP: u64 = 1 << 2;
    const LEFT_MOUSE_DRAGGED: u64 = 1 << 6;
    let block = block2::RcBlock::new(move |event: NonNull<AnyObject>| -> *mut AnyObject {
        // SAFETY: AppKit passes a live event for the duration of the call.
        let kind: u64 = unsafe { msg_send![event.as_ref(), type] };
        if kind == 2 {
            on_up();
        } else {
            on_move();
        }
        event.as_ptr()
    });
    // SAFETY: main thread; the block is copied by AppKit; the token is kept until removed.
    let token: Option<Retained<AnyObject>> = unsafe {
        msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: LEFT_MOUSE_UP | LEFT_MOUSE_DRAGGED, handler: &*block]
    };
    let ok = token.is_some();
    DRAG_MONITOR.with(|slot| *slot.borrow_mut() = token);
    ok
}

/// End a bar drag's pointer following.
pub(crate) fn stop_drag_monitor() {
    let token = DRAG_MONITOR.with(|slot| slot.borrow_mut().take());
    if let Some(token) = token {
        // SAFETY: main thread; the token came from `addLocalMonitorForEventsMatchingMask:`.
        let _: () = unsafe { msg_send![class!(NSEvent), removeMonitor: &*token] };
    }
}

/// No system zoom-in when the bar orders in: it shows by its own fade (`set_bar_alpha`).
pub(crate) fn no_appear_animation(window: &rune::WindowHandle) {
    let Some(ns_window) = bar_window(window) else { return };
    // NSWindowAnimationBehaviorNone = 2.
    let _: () = unsafe { msg_send![&*ns_window, setAnimationBehavior: 2isize] };
}

/// Hide the bar in Mission Control and App Exposé, so they show only the glued app window.
///
/// Rune gives its overlay panel `Stationary`, which keeps the bar at full size while the app
/// window shrinks to a thumbnail. `Transient` (hidden by Exposé) replaces it. The other bits
/// Rune sets stay. `Transient`, `Managed` and `Stationary` exclude each other. Returns the mask
/// read back from the panel.
pub(crate) fn hide_in_mission_control(window: &rune::WindowHandle) -> Option<usize> {
    const TRANSIENT: usize = 1 << 3;
    const STATIONARY: usize = 1 << 4;
    let ns_window = bar_window(window)?;
    // SAFETY: main thread (`raw_ns_view` checked in `bar_window`).
    unsafe {
        let current: usize = msg_send![&*ns_window, collectionBehavior];
        if crate::debug::marks_on() {
            let class: Retained<AnyObject> = msg_send![&*ns_window, class];
            let name: Retained<AnyObject> = msg_send![&*class, description];
            let utf8: *const std::ffi::c_char = msg_send![&*name, UTF8String];
            let name = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
            crate::debug::mark(&format!("bar-panel before mask {current} class {name}"));
        }
        let wanted = (current & !STATIONARY) | TRANSIENT;
        let _: () = msg_send![&*ns_window, setCollectionBehavior: wanted];
        Some(msg_send![&*ns_window, collectionBehavior])
    }
}

/// Debug: the bar panel's current `collectionBehavior`.
pub(crate) fn bar_collection_behavior(window: &rune::WindowHandle) -> Option<usize> {
    let ns_window = bar_window(window)?;
    // SAFETY: main thread; plain getter.
    Some(unsafe { msg_send![&*ns_window, collectionBehavior] })
}

/// Show or hide the bar by its alpha, without ordering it in or out. `fade` animates it.
pub(crate) fn set_bar_alpha(window: &rune::WindowHandle, alpha: f64, fade: f64) -> bool {
    let Some(ns_window) = bar_window(window) else { return false };
    // SAFETY: main thread; NSAnimationContext groups the animator change.
    unsafe {
        if fade > 0.0 {
            let _: () = msg_send![class!(NSAnimationContext), beginGrouping];
            let ctx: Retained<AnyObject> = msg_send![class!(NSAnimationContext), currentContext];
            let _: () = msg_send![&*ctx, setDuration: fade];
            let animator: Retained<AnyObject> = msg_send![&*ns_window, animator];
            let _: () = msg_send![&*animator, setAlphaValue: alpha];
            let _: () = msg_send![class!(NSAnimationContext), endGrouping];
        } else {
            let _: () = msg_send![&*ns_window, setAlphaValue: alpha];
        }
    }
    true
}

/// Debug hook: call `step` on the main thread at 120 Hz, from an NSTimer, until it returns
/// false.
pub(crate) fn every_frame(step: Box<dyn Fn() -> bool>) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let block = block2::RcBlock::new(move |timer: NonNull<AnyObject>| {
        if !step() {
            // SAFETY: the timer passed in is live; invalidating stops it.
            let _: () = unsafe { msg_send![timer.as_ref(), invalidate] };
        }
    });
    // SAFETY: main thread; the run loop retains the timer, the timer copies the block.
    unsafe {
        let timer: Retained<AnyObject> = msg_send![class!(NSTimer), timerWithTimeInterval: 1.0f64 / 120.0, repeats: true, block: &*block];
        let run_loop: Retained<AnyObject> = msg_send![class!(NSRunLoop), mainRunLoop];
        let common = NSString::from_str("kCFRunLoopCommonModes");
        let _: () = msg_send![&*run_loop, addTimer: &*timer, forMode: &*common];
    }
}

/// The system window background (`NSColor.windowBackgroundColor`) in the light and the dark
/// appearance, as sRGB components: the color of a standard window's title bar, which the
/// active tab takes so it runs into the glued window.
pub(crate) fn window_backgrounds() -> Option<([f64; 4], [f64; 4])> {
    MainThreadMarker::new()?;
    let resolve = |name: &str| -> Option<[f64; 4]> {
        let out = std::rc::Rc::new(std::cell::Cell::new(None));
        let slot = out.clone();
        // SAFETY: main thread; AppKit class methods and a block run synchronously by
        // `performAsCurrentDrawingAppearance:`.
        unsafe {
            let appearance: Option<Retained<AnyObject>> =
                msg_send![class!(NSAppearance), appearanceNamed: &*NSString::from_str(name)];
            let appearance = appearance?;
            let block = block2::RcBlock::new(move || {
                let color: Retained<AnyObject> = msg_send![class!(NSColor), windowBackgroundColor];
                let space: Retained<AnyObject> = msg_send![class!(NSColorSpace), sRGBColorSpace];
                let rgb: Option<Retained<AnyObject>> = msg_send![&*color, colorUsingColorSpace: &*space];
                if let Some(rgb) = rgb {
                    let (r, g, b, a): (f64, f64, f64, f64) = (
                        msg_send![&*rgb, redComponent],
                        msg_send![&*rgb, greenComponent],
                        msg_send![&*rgb, blueComponent],
                        msg_send![&*rgb, alphaComponent],
                    );
                    slot.set(Some([r, g, b, a]));
                }
            });
            let _: () = msg_send![&*appearance, performAsCurrentDrawingAppearance: &*block];
        }
        out.get()
    };
    Some((resolve("NSAppearanceNameAqua")?, resolve("NSAppearanceNameDarkAqua")?))
}

/// The bundle ids of the running apps (NSWorkspace), for the picker's "Running" marks.
pub(crate) fn running_bundle_ids() -> Vec<String> {
    if MainThreadMarker::new().is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: main thread; plain NSWorkspace reads, every object retained while used.
    unsafe {
        let ws: Retained<AnyObject> = msg_send![class!(NSWorkspace), sharedWorkspace];
        let apps: Retained<NSArray<AnyObject>> = msg_send![&*ws, runningApplications];
        for app in apps.iter() {
            let id: Option<Retained<NSString>> = msg_send![&*app, bundleIdentifier];
            if let Some(id) = id {
                out.push(id.to_string());
            }
        }
    }
    out
}

mod cg {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub static kCGWindowIsOnscreen: *const c_void;
        pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFArrayGetCount(array: *const c_void) -> isize;
        pub fn CFArrayGetValueAtIndex(array: *const c_void, i: isize) -> *const c_void;
        pub fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        pub fn CFBooleanGetValue(b: *const c_void) -> bool;
        pub fn CFRelease(object: *const c_void);
    }
}

/// Is this `CGWindowID` a window on screen right now? (`IncludingWindow` alone lists just
/// that window; with `OnScreenOnly` it would list every window on screen.)
fn on_screen(id: u32) -> bool {
    const INCLUDING_WINDOW: u32 = 1 << 3;
    // SAFETY: plain CoreGraphics reads; the returned array is owned (Copy rule) and released;
    // the values read from it are borrowed while it lives.
    unsafe {
        let list = cg::CGWindowListCopyWindowInfo(INCLUDING_WINDOW, id);
        if list.is_null() {
            return false;
        }
        let mut on = false;
        if cg::CFArrayGetCount(list) > 0 {
            let info = cg::CFArrayGetValueAtIndex(list, 0);
            let value = cg::CFDictionaryGetValue(info, cg::kCGWindowIsOnscreen);
            on = !value.is_null() && cg::CFBooleanGetValue(value);
        }
        cg::CFRelease(list);
        on
    }
}

/// The window's `NSView`, retained: the raw handle only borrows it for a call.
fn raw_ns_view(window: &rune::WindowHandle) -> Option<Retained<AnyObject>> {
    MainThreadMarker::new()?;
    // SAFETY: raw_handle borrows the view only for this call; `retain` takes a reference
    // we keep (along with the observers) for the run. Main thread, as `with_raw_handle`
    // asks.
    window
        .with_raw_handle(|raw| {
            match raw.as_raw() {
                raw_window_handle::RawWindowHandle::AppKit(handle) => {
                    Some(unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<AnyObject>())
                        .expect("the view is alive inside with_raw_handle") })
                }
                _ => None,
            }
        })
        .ok()
        .flatten()
}

/// The primary display's height in points: AppKit flips its y axis around it.
fn primary_screen_height() -> Option<f64> {
    // SAFETY: `+[NSScreen screens]` and `-[NSScreen frame]` on the main thread.
    let screens: Option<Retained<NSArray<AnyObject>>> = unsafe { msg_send![class!(NSScreen), screens] };
    let screens = screens?;
    let primary: Option<Retained<AnyObject>> = unsafe { msg_send![&*screens, firstObject] };
    let primary = primary?;
    let rect: NSRect = unsafe { msg_send![&*primary, frame] };
    Some(rect.size.height)
}

/// The menubar's Quit routing: point the item that would `terminate:` at a target of ours.
pub(crate) fn route_menu_quit(quit: Box<dyn Fn()>) -> bool {
    quit_target::install(quit)
}

mod quit_target {
    use super::*;
    use objc2::{AnyThread, DefinedClass};

    // A tiny target class: the menu item's action stays `terminate:`, its target becomes
    // this, so the built-in ⌘Q runs Oriel's quit (and its effects) instead.
    define_class!(
        // SAFETY: NSObject has no subclassing requirements; the ivar is a `RefCell` touched
        // only on the main thread; `terminate:` is not otherwise implemented on NSObject
        // subclasses in this process.
        #[unsafe(super(objc2::runtime::NSObject))]
        #[name = "OrielQuitTarget"]
        #[ivars = Ivars]
        pub(crate) struct QuitTarget;

        impl QuitTarget {
            #[unsafe(method(terminate:))]
            #[allow(non_snake_case)]
            unsafe fn terminate_(&self, _sender: Option<&AnyObject>) {
                if let Some(quit) = self.ivars().quit.borrow().as_ref() {
                    quit();
                }
            }
        }
    );

    /// The target's stored work.
    pub(crate) struct Ivars {
        quit: RefCell<Option<Box<dyn Fn()>>>,
    }

    thread_local! {
        /// The target instance, alive for the process (any window creation may re-route).
        static TARGET: RefCell<Option<Retained<QuitTarget>>> = const { RefCell::new(None) };
    }

    /// Install the target once, then retarget the default menu's `terminate:` item to it.
    pub(crate) fn install(quit: Box<dyn Fn()>) -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        TARGET.with(|slot| {
            if slot.borrow().is_none() {
                let this = QuitTarget::alloc().set_ivars(Ivars { quit: RefCell::new(Some(quit)) });
                // SAFETY: NSObject's plain `init` on the subclass on the main thread.
                let installed: Retained<QuitTarget> = unsafe { msg_send![super(this), init] };
                *slot.borrow_mut() = Some(installed);
            }
            let target = slot.borrow().as_ref().expect("installed above").clone();
            retarget(&target)
        })
    }

    /// Give the app menu's `terminate:` items our target.
    fn retarget(target: &QuitTarget) -> bool {
        // SAFETY: `NSApplication` is main-thread; every menu read is retained.
        let app: Retained<AnyObject> = unsafe { msg_send![class!(NSApplication), sharedApplication] };
        let Some(root): Option<Retained<AnyObject>> = (unsafe { msg_send![&*app, mainMenu] }) else {
            return false;
        };
        let Some(app_menu): Option<Retained<AnyObject>> = (unsafe { msg_send![&*root, itemAtIndex: 0isize] }) else {
            return false;
        };
        let Some(menu): Option<Retained<AnyObject>> = (unsafe { msg_send![&*app_menu, submenu] }) else {
            return false;
        };
        let count: isize = unsafe { msg_send![&*menu, numberOfItems] };
        let mut routed = false;
        for index in 0..count {
            let Some(item): Option<Retained<AnyObject>> =
                (unsafe { msg_send![&*menu, itemAtIndex: index] })
            else {
                continue;
            };
            let action: Option<objc2::runtime::Sel> = unsafe { msg_send![&*item, action] };
            if action == Some(sel!(terminate:)) {
                // SAFETY: the target stays alive in the thread-local for the run.
                let _: () = unsafe { msg_send![&*item, setTarget: target] };
                routed = true;
            }
        }
        routed
    }
}


/// Is the app in the dark appearance now? (`NSApp.effectiveAppearance`.)
pub(crate) fn is_dark() -> bool {
    if MainThreadMarker::new().is_none() {
        return false;
    }
    // SAFETY: main thread; plain AppKit reads.
    unsafe {
        let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
        let appearance: Option<Retained<AnyObject>> = msg_send![&*app, effectiveAppearance];
        let Some(appearance) = appearance else { return false };
        let names = NSArray::from_retained_slice(&[
            NSString::from_str("NSAppearanceNameAqua"),
            NSString::from_str("NSAppearanceNameDarkAqua"),
        ]);
        let best: Option<Retained<NSString>> = msg_send![&*appearance, bestMatchFromAppearancesWithNames: &*names];
        best.is_some_and(|name| name.to_string().contains("Dark"))
    }
}

/// How the divider of a split view looks: sRGB components (red, green, blue, alpha).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DividerLook {
    /// The gap between the two windows (the bar's strip color).
    pub gap: [f64; 4],
    /// The line in the middle of the gap, and its width in points.
    pub line: [f64; 4],
    pub line_width: f64,
}

/// What the divider tells the app crate: the pointer enters or leaves it, a press on it.
pub(crate) struct DividerHooks {
    pub hover: Box<dyn Fn(bool)>,
    pub press: Box<dyn Fn()>,
}

thread_local! {
    static DIVIDER: RefCell<Option<divider::Parts>> = const { RefCell::new(None) };
}

/// Make the divider's window (hidden) and tie it to the bar. Once; main thread only.
///
/// The divider is a small borderless, non-activating `NSPanel` of Oriel's own, a child window
/// of the bar's panel (`addChildWindow:ordered:`): the window server moves a child together
/// with its parent, so during a bar drag the divider has no lag at all. It never becomes key,
/// never activates Oriel, and is `Transient` in Mission Control like the bar.
pub(crate) fn divider_install(bar: &rune::WindowHandle, hooks: DividerHooks) -> bool {
    divider::install(bar, hooks)
}

/// Show the divider at `rect` (Oriel points), or hide it.
pub(crate) fn divider_set(bar: &rune::WindowHandle, rect: Option<Rect>) {
    divider::set(bar, rect);
}

/// Change the divider's look (hover, drag, appearance).
pub(crate) fn divider_look(look: DividerLook) {
    divider::look(look);
}

/// Order the divider directly above the bar (so above both app windows too).
pub(crate) fn divider_raise(bar: &rune::WindowHandle) {
    divider::raise(bar);
}

/// Debug: the divider window's frame (Oriel points), whether it is on screen, whether it is the
/// bar's child, its level and collection behavior, and whether it can take focus.
pub(crate) fn divider_debug(bar: &rune::WindowHandle) -> Option<String> {
    divider::debug(bar)
}

/// Set the left-right resize cursor now (a divider drag keeps it while the pointer leaves the strip).
pub(crate) fn divider_cursor() {
    divider::cursor();
}

mod divider {
    use super::*;
    use objc2::rc::Allocated;
    use objc2::{AnyThread, DefinedClass};
    use objc2_foundation::{NSPoint, NSSize};

    pub(super) struct Parts {
        panel: Retained<AnyObject>,
        gap: Retained<AnyObject>,
        line: Retained<AnyObject>,
        _target: Retained<Target>,
        _monitor: Option<Retained<AnyObject>>,
        attached: bool,
        rect: Option<Rect>,
        look: Option<DividerLook>,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements; the ivars are touched only on the
        // main thread; the three methods are the tracking area owner's callbacks.
        #[unsafe(super(objc2::runtime::NSObject))]
        #[name = "OrielDividerTarget"]
        #[ivars = Ivars]
        pub(crate) struct Target;

        impl Target {
            #[unsafe(method(mouseEntered:))]
            #[allow(non_snake_case)]
            unsafe fn mouseEntered_(&self, _event: Option<&AnyObject>) {
                (self.ivars().hover)(true);
            }

            #[unsafe(method(mouseExited:))]
            #[allow(non_snake_case)]
            unsafe fn mouseExited_(&self, _event: Option<&AnyObject>) {
                (self.ivars().hover)(false);
            }

            #[unsafe(method(cursorUpdate:))]
            #[allow(non_snake_case)]
            unsafe fn cursorUpdate_(&self, _event: Option<&AnyObject>) {
                cursor();
            }
        }
    );

    pub(crate) struct Ivars {
        hover: Box<dyn Fn(bool)>,
    }

    fn ns_rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    fn color(c: [f64; 4]) -> Retained<AnyObject> {
        // SAFETY: plain class method.
        unsafe { msg_send![class!(NSColor), colorWithSRGBRed: c[0], green: c[1], blue: c[2], alpha: c[3]] }
    }

    /// A custom `NSBox` filled with one color, no border: a flat rectangle.
    fn flat_box(frame: NSRect) -> Retained<AnyObject> {
        // SAFETY: main thread; `NSBox` initialised with its designated initializer.
        unsafe {
            let alloc: Allocated<AnyObject> = msg_send![class!(NSBox), alloc];
            let view: Retained<AnyObject> = msg_send![alloc, initWithFrame: frame];
            // NSBoxCustom = 4; NSNoTitle = 0.
            let _: () = msg_send![&*view, setBoxType: 4usize];
            let _: () = msg_send![&*view, setTitlePosition: 0usize];
            let _: () = msg_send![&*view, setBorderWidth: 0.0f64];
            let _: () = msg_send![&*view, setContentViewMargins: NSSize::new(0.0, 0.0)];
            view
        }
    }

    pub(super) fn install(bar: &rune::WindowHandle, hooks: DividerHooks) -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        if DIVIDER.with(|d| d.borrow().is_some()) {
            return true;
        }
        let Some(bar_window) = bar_window(bar) else { return false };
        let DividerHooks { hover, press } = hooks;
        // SAFETY: main thread throughout; every object is retained by `Parts` for the run.
        let parts = unsafe {
            // NSWindowStyleMaskNonactivatingPanel = 1 << 7; borderless = 0. NSBackingStoreBuffered = 2.
            let alloc: Allocated<AnyObject> = msg_send![class!(NSPanel), alloc];
            let panel: Retained<AnyObject> = msg_send![
                alloc,
                initWithContentRect: ns_rect(0.0, 0.0, 8.0, 100.0),
                styleMask: 1usize << 7,
                backing: 2usize,
                defer: false
            ];
            let clear: Retained<AnyObject> = msg_send![class!(NSColor), clearColor];
            let _: () = msg_send![&*panel, setReleasedWhenClosed: false];
            let _: () = msg_send![&*panel, setOpaque: false];
            let _: () = msg_send![&*panel, setBackgroundColor: &*clear];
            let _: () = msg_send![&*panel, setHasShadow: false];
            let _: () = msg_send![&*panel, setHidesOnDeactivate: false];
            let _: () = msg_send![&*panel, setBecomesKeyOnlyIfNeeded: true];
            let _: () = msg_send![&*panel, setAnimationBehavior: 2isize];
            // Transient (1 << 3): hidden in Mission Control and App Exposé, as the bar is.
            // IgnoresCycle (1 << 6): not in Cmd+` cycling.
            let _: () = msg_send![&*panel, setCollectionBehavior: (1usize << 3) | (1usize << 6)];
            let level: isize = msg_send![&*bar_window, level];
            let _: () = msg_send![&*panel, setLevel: level];

            let content = flat_box(ns_rect(0.0, 0.0, 8.0, 100.0));
            let gap = flat_box(ns_rect(0.0, 0.0, 8.0, 100.0));
            let line = flat_box(ns_rect(3.5, 0.0, 1.0, 100.0));
            let _: () = msg_send![&*content, addSubview: &*gap];
            let _: () = msg_send![&*content, addSubview: &*line];
            let _: () = msg_send![&*panel, setContentView: &*content];

            let target = Target::alloc().set_ivars(Ivars { hover });
            let target: Retained<Target> = msg_send![super(target), init];
            // MouseEnteredAndExited 0x1 | CursorUpdate 0x4 | ActiveAlways 0x80 | InVisibleRect 0x200.
            let area_alloc: Allocated<AnyObject> = msg_send![class!(NSTrackingArea), alloc];
            let area: Retained<AnyObject> = msg_send![
                area_alloc,
                initWithRect: ns_rect(0.0, 0.0, 0.0, 0.0),
                options: 0x1usize | 0x4 | 0x80 | 0x200,
                owner: &*target,
                userInfo: std::ptr::null::<AnyObject>()
            ];
            let _: () = msg_send![&*content, addTrackingArea: &*area];

            // A press on the divider: a local monitor sees it before AppKit's own dispatch.
            let panel_ptr = Retained::as_ptr(&panel) as usize;
            let block = block2::RcBlock::new(move |event: NonNull<AnyObject>| -> *mut AnyObject {
                let window: *mut AnyObject = msg_send![event.as_ref(), window];
                if window as usize == panel_ptr {
                    press();
                    return std::ptr::null_mut();
                }
                event.as_ptr()
            });
            const LEFT_MOUSE_DOWN: u64 = 1 << 1;
            let monitor: Option<Retained<AnyObject>> = msg_send![
                class!(NSEvent),
                addLocalMonitorForEventsMatchingMask: LEFT_MOUSE_DOWN,
                handler: &*block
            ];
            Parts {
                panel,
                gap,
                line,
                _target: target,
                _monitor: monitor,
                attached: false,
                rect: None,
                look: None,
            }
        };
        DIVIDER.with(|d| *d.borrow_mut() = Some(parts));
        true
    }

    pub(super) fn set(bar: &rune::WindowHandle, rect: Option<Rect>) {
        let Some(bar_window) = bar_window(bar) else { return };
        let Some(height) = primary_screen_height() else { return };
        DIVIDER.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(parts) = slot.as_mut() else { return };
            parts.rect = rect;
            // SAFETY: main thread; plain window calls on live objects.
            unsafe {
                let Some(rect) = rect else {
                    if parts.attached {
                        let _: () = msg_send![&*bar_window, removeChildWindow: &*parts.panel];
                        parts.attached = false;
                    }
                    let _: () = msg_send![&*parts.panel, orderOut: std::ptr::null::<AnyObject>()];
                    return;
                };
                let frame = ns_rect(rect.x, height - rect.y - rect.h, rect.w, rect.h);
                let current: NSRect = msg_send![&*parts.panel, frame];
                if current != frame {
                    let _: () = msg_send![&*parts.panel, setFrame: frame, display: true];
                    layout(parts, rect);
                }
                if !parts.attached {
                    let level: isize = msg_send![&*bar_window, level];
                    let _: () = msg_send![&*parts.panel, setLevel: level];
                    // NSWindowAbove = 1.
                    let _: () = msg_send![&*bar_window, addChildWindow: &*parts.panel, ordered: 1isize];
                    parts.attached = true;
                    let _: () = msg_send![&*parts.panel, orderFrontRegardless];
                }
            }
        });
    }

    /// The gap fills the panel; the line sits in the middle of it, full height.
    fn layout(parts: &Parts, rect: Rect) {
        let width = parts.look.map_or(1.0, |l| l.line_width);
        // SAFETY: main thread; plain frame sets.
        unsafe {
            let _: () = msg_send![&*parts.gap, setFrame: ns_rect(0.0, 0.0, rect.w, rect.h)];
            let _: () = msg_send![&*parts.line, setFrame: ns_rect(((rect.w - width) / 2.0).floor(), 0.0, width, rect.h)];
        }
    }

    pub(super) fn look(look: DividerLook) {
        DIVIDER.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(parts) = slot.as_mut() else { return };
            if parts.look == Some(look) {
                return;
            }
            parts.look = Some(look);
            // SAFETY: main thread; plain setters on live boxes.
            unsafe {
                let _: () = msg_send![&*parts.gap, setFillColor: &*color(look.gap)];
                let _: () = msg_send![&*parts.line, setFillColor: &*color(look.line)];
            }
            if let Some(rect) = parts.rect {
                layout(parts, rect);
            }
        });
    }

    pub(super) fn raise(bar: &rune::WindowHandle) {
        let Some(bar_window) = bar_window(bar) else { return };
        DIVIDER.with(|slot| {
            let slot = slot.borrow();
            let Some(parts) = slot.as_ref() else { return };
            if parts.rect.is_none() {
                return;
            }
            // SAFETY: main thread; the bar's window number is a plain getter.
            unsafe {
                let number: isize = msg_send![&*bar_window, windowNumber];
                // NSWindowAbove = 1.
                let _: () = msg_send![&*parts.panel, orderWindow: 1isize, relativeTo: number];
            }
        });
    }

    pub(super) fn debug(bar: &rune::WindowHandle) -> Option<String> {
        let bar_window = bar_window(bar)?;
        let height = primary_screen_height()?;
        DIVIDER.with(|slot| {
            let slot = slot.borrow();
            let parts = slot.as_ref()?;
            // SAFETY: main thread; plain getters.
            unsafe {
                let frame: NSRect = msg_send![&*parts.panel, frame];
                let visible: bool = msg_send![&*parts.panel, isVisible];
                let level: isize = msg_send![&*parts.panel, level];
                let bar_level: isize = msg_send![&*bar_window, level];
                let behavior: usize = msg_send![&*parts.panel, collectionBehavior];
                let key: bool = msg_send![&*parts.panel, canBecomeKeyWindow];
                let main: bool = msg_send![&*parts.panel, canBecomeMainWindow];
                let parent: Option<Retained<AnyObject>> = msg_send![&*parts.panel, parentWindow];
                let is_child = parent.is_some_and(|p| Retained::as_ptr(&p) == Retained::as_ptr(&bar_window));
                let order: isize = msg_send![&*parts.panel, windowNumber];
                let bar_frame: NSRect = msg_send![&*bar_window, frame];
                Some(format!(
                    "divider-window x={} y={} w={} h={} visible={visible} child_of_bar={is_child} level={level} bar_level={bar_level} behavior={behavior} can_key={key} can_main={main} number={order} bar_x={} bar_y={}",
                    frame.origin.x,
                    height - frame.origin.y - frame.size.height,
                    frame.size.width,
                    frame.size.height,
                    bar_frame.origin.x,
                    height - bar_frame.origin.y - bar_frame.size.height,
                ))
            }
        })
    }

    pub(super) fn cursor() {
        // SAFETY: plain class method and setter.
        unsafe {
            let cursor: Retained<AnyObject> = msg_send![class!(NSCursor), resizeLeftRightCursor];
            let _: () = msg_send![&*cursor, set];
        }
    }
}
