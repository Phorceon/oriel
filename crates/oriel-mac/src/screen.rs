//! The visible area of the main screen, and the main-thread notifications.

use crate::frame::appkit_to_top_left;
use crate::msg::{Msg, WsNote};
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSScreen, NSWorkspace,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidLaunchApplicationNotification,
    NSWorkspaceDidTerminateApplicationNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSOperationQueue};
use oriel_core::{Event, Rect};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc::Sender;

type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// The main screen's `visibleFrame` in the contract's coordinate space.
pub fn visible_frame(mtm: MainThreadMarker) -> Option<Rect> {
    let screens = NSScreen::screens(mtm);
    let primary = screens.iter().next()?;
    let main = NSScreen::mainScreen(mtm).unwrap_or_else(|| primary.clone());
    let v = main.visibleFrame();
    let primary_h = primary.frame().size.height;
    Some(appkit_to_top_left(v.origin.x, v.origin.y, v.size.width, v.size.height, primary_h))
}

fn emit_screen(mtm: MainThreadMarker, sink: &Sink) {
    if let Some(visible) = visible_frame(mtm) {
        sink(Event::ScreenChanged { visible });
    }
}

type Token = Retained<ProtocolObject<dyn NSObjectProtocol>>;

fn observe(
    center: &NSNotificationCenter,
    name: &objc2_foundation::NSNotificationName,
    handler: impl Fn(&NSNotification) + 'static,
) -> Token {
    let block = RcBlock::new(move |n: NonNull<NSNotification>| {
        // SAFETY: AppKit passes a valid notification for the duration of the call.
        handler(unsafe { n.as_ref() });
    });
    // SAFETY: the block is copied by the center; the queue is the main queue.
    unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(name),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        )
    }
}

/// Register all main-thread observers and send the first screen event.
pub fn setup_main_thread(mtm: MainThreadMarker, sink: Sink, tx: Sender<Msg>) {
    emit_screen(mtm, &sink);
    let mut tokens: Vec<Token> = Vec::new();

    let s = sink.clone();
    // SAFETY: the notification name statics are valid for the process lifetime.
    let name = unsafe { NSApplicationDidChangeScreenParametersNotification };
    tokens.push(observe(&NSNotificationCenter::defaultCenter(), name, move |_| emit_screen(mtm, &s)));

    let ws_center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: statics, as above.
    let (launch, term, activate) = unsafe {
        (
            NSWorkspaceDidLaunchApplicationNotification,
            NSWorkspaceDidTerminateApplicationNotification,
            NSWorkspaceDidActivateApplicationNotification,
        )
    };
    for name in [launch, term] {
        let tx = tx.clone();
        tokens.push(observe(&ws_center, name, move |_| {
            let _ = tx.send(Msg::Workspace(WsNote::Reconcile));
        }));
    }
    let tx2 = tx.clone();
    tokens.push(observe(&ws_center, activate, move |_| {
        if let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() {
            let _ = tx2.send(Msg::Workspace(WsNote::Activated(app.processIdentifier())));
        }
    }));
    // The observers live as long as the process. The tokens are not Send, so leak them.
    std::mem::forget(tokens);
}

mod cg {
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct CGRect {
        pub x: f64,
        pub y: f64,
        pub w: f64,
        pub h: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub static kCGWindowBounds: *const c_void;
        pub static kCGWindowIsOnscreen: *const c_void;
        pub fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
        pub fn CGRectMakeWithDictionaryRepresentation(dict: *const c_void, rect: *mut CGRect) -> bool;
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

/// Where the window server shows this window now (global top-left points), or `None` when
/// it is not on screen (hidden, minimized, closed). No permission needed.
pub fn window_bounds(id: u32) -> Option<Rect> {
    const INCLUDING_WINDOW: u32 = 1 << 3;
    // SAFETY: plain CoreGraphics/CoreFoundation reads. The array follows the Copy rule and is
    // released; the values read from it are borrowed while it lives.
    unsafe {
        let list = cg::CGWindowListCopyWindowInfo(INCLUDING_WINDOW, id);
        if list.is_null() {
            return None;
        }
        let mut out = None;
        if cg::CFArrayGetCount(list) > 0 {
            let info = cg::CFArrayGetValueAtIndex(list, 0);
            let on = cg::CFDictionaryGetValue(info, cg::kCGWindowIsOnscreen);
            let bounds = cg::CFDictionaryGetValue(info, cg::kCGWindowBounds);
            let mut r = cg::CGRect::default();
            if !on.is_null() && cg::CFBooleanGetValue(on) && !bounds.is_null()
                && cg::CGRectMakeWithDictionaryRepresentation(bounds, &mut r)
            {
                out = Some(Rect::new(r.x, r.y, r.w, r.h));
            }
        }
        cg::CFRelease(list);
        out
    }
}
