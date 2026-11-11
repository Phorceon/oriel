//! A safe wrapper around one `AXUIElement`.

use crate::ffi::*;
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFTypeRef};
use core_foundation_sys::number::{CFBooleanGetTypeID, CFBooleanGetValue, CFBooleanRef, kCFBooleanFalse, kCFBooleanTrue};
use oriel_core::{Rect, WindowId};
use std::ffi::c_void;

/// How long one AX call may wait for a slow app, in seconds.
pub const AX_TIMEOUT: f32 = 0.35;

/// A retained accessibility element (an app, a window, a button).
#[derive(Clone, PartialEq)]
pub struct Ax(pub Cf);

impl Ax {
    /// The application element of a process.
    pub fn app(pid: i32) -> Option<Ax> {
        // SAFETY: plain call; the result is a +1 object or null.
        let el = unsafe { Cf::from_owned(AXUIElementCreateApplication(pid) as *const c_void) }?;
        let ax = Ax(el);
        ax.set_timeout();
        Some(ax)
    }

    /// Wrap a borrowed element pointer (from a callback).
    pub fn retain(ptr: *const c_void) -> Option<Ax> {
        // SAFETY: the caller passes a live AXUIElementRef or null.
        unsafe { Cf::retain(ptr) }.map(Ax)
    }

    fn raw(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    pub fn set_timeout(&self) {
        // SAFETY: valid element.
        unsafe { AXUIElementSetMessagingTimeout(self.raw(), AX_TIMEOUT) };
    }

    fn copy(&self, name: &str) -> Option<Cf> {
        let key = cfstr(name)?;
        let mut out: CFTypeRef = std::ptr::null();
        // SAFETY: valid element and key; `out` is a valid out pointer; a +1 value on success.
        let err = unsafe { AXUIElementCopyAttributeValue(self.raw(), key.as_ptr() as _, &mut out) };
        if err != AX_OK {
            return None;
        }
        // SAFETY: on success `out` is a +1 object or null.
        unsafe { Cf::from_owned(out) }
    }

    pub fn string(&self, name: &str) -> Option<String> {
        cf_to_string(&self.copy(name)?)
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        let v = self.copy(name)?;
        // SAFETY: the type is checked before the value is read as a CFBoolean.
        unsafe {
            if CFGetTypeID(v.as_ptr() as CFTypeRef) != CFBooleanGetTypeID() {
                return None;
            }
            Some(CFBooleanGetValue(v.as_ptr() as CFBooleanRef))
        }
    }

    pub fn element(&self, name: &str) -> Option<Ax> {
        self.copy(name).map(Ax)
    }

    /// The elements of an array attribute (for example `AXWindows`).
    pub fn elements(&self, name: &str) -> Vec<Ax> {
        let Some(arr) = self.copy(name) else { return Vec::new() };
        let mut out = Vec::new();
        // SAFETY: the type is checked; indices are in range; items are retained before the
        // array is released.
        unsafe {
            if CFGetTypeID(arr.as_ptr() as CFTypeRef) != CFArrayGetTypeID() {
                return out;
            }
            let a = arr.as_ptr() as CFArrayRef;
            for i in 0..CFArrayGetCount(a) {
                if let Some(item) = Cf::retain(CFArrayGetValueAtIndex(a, i)) {
                    out.push(Ax(item));
                }
            }
        }
        out
    }

    pub fn set_bool(&self, name: &str, value: bool) -> bool {
        let Some(key) = cfstr(name) else { return false };
        // SAFETY: valid element and key; the CFBoolean constants are immortal.
        let err = unsafe {
            let b = if value { kCFBooleanTrue } else { kCFBooleanFalse };
            AXUIElementSetAttributeValue(self.raw(), key.as_ptr() as _, b as CFTypeRef)
        };
        err == AX_OK
    }

    fn set_value(&self, name: &str, ty: i32, ptr: *const c_void) -> bool {
        let Some(key) = cfstr(name) else { return false };
        // SAFETY: `ptr` points to a live CGPoint/CGSize matching `ty`; the AXValue is +1.
        unsafe {
            let Some(v) = Cf::from_owned(AXValueCreate(ty, ptr)) else { return false };
            AXUIElementSetAttributeValue(self.raw(), key.as_ptr() as _, v.as_ptr() as CFTypeRef) == AX_OK
        }
    }

    pub fn set_position(&self, x: f64, y: f64) -> bool {
        let p = CGPoint { x, y };
        self.set_value("AXPosition", AX_VALUE_CGPOINT, &p as *const CGPoint as *const c_void)
    }

    pub fn set_size(&self, w: f64, h: f64) -> bool {
        let s = CGSize { width: w, height: h };
        self.set_value("AXSize", AX_VALUE_CGSIZE, &s as *const CGSize as *const c_void)
    }

    fn get_value<T: Default>(&self, name: &str, ty: i32) -> Option<T> {
        let v = self.copy(name)?;
        let mut out = T::default();
        // SAFETY: `out` is a valid, writable T whose layout matches the AXValue type `ty`.
        let ok = unsafe { AXValueGetValue(v.as_ptr() as CFTypeRef, ty, &mut out as *mut T as *mut c_void) };
        (ok != 0).then_some(out)
    }

    /// The window frame in global top-left coordinates.
    pub fn frame(&self) -> Option<Rect> {
        let p: CGPoint = self.get_value("AXPosition", AX_VALUE_CGPOINT)?;
        let s: CGSize = self.get_value("AXSize", AX_VALUE_CGSIZE)?;
        Some(Rect::new(p.x, p.y, s.width, s.height))
    }

    pub fn perform(&self, action: &str) -> bool {
        let Some(key) = cfstr(action) else { return false };
        // SAFETY: valid element and action name.
        unsafe { AXUIElementPerformAction(self.raw(), key.as_ptr() as _) == AX_OK }
    }

    pub fn window_id(&self) -> Option<WindowId> {
        let mut id: u32 = 0;
        // SAFETY: valid element; `id` is a valid out pointer.
        let err = unsafe { _AXUIElementGetWindow(self.raw(), &mut id) };
        (err == AX_OK && id != 0).then_some(WindowId(id))
    }

    /// A document-like window: AXRole `AXWindow` and AXSubrole `AXStandardWindow`, with at
    /// least one of the close, minimize and zoom buttons. The button test drops open/save
    /// panels, which some apps (TextEdit) mark as standard windows. A minimized window
    /// reports another subrole (`AXDialog` on macOS 26), so it passes on the minimize button.
    pub fn is_standard_window(&self) -> bool {
        if self.string("AXRole").as_deref() != Some("AXWindow") {
            return false;
        }
        let has_min = self.element("AXMinimizeButton").is_some();
        if self.bool("AXMinimized") == Some(true) {
            return has_min;
        }
        self.string("AXSubrole").as_deref() == Some("AXStandardWindow")
            && (has_min || ["AXCloseButton", "AXZoomButton"].iter().any(|b| self.element(b).is_some()))
    }

    pub fn title(&self) -> String {
        self.string("AXTitle").unwrap_or_default()
    }

    /// One line of attributes for debugging (`ORIEL_MAC_DEBUG=1`).
    pub fn describe(&self) -> String {
        format!(
            "role={:?} subrole={:?} title={:?} modal={:?} minBtn={} zoomBtn={} closeBtn={} minimized={:?}",
            self.string("AXRole"),
            self.string("AXSubrole"),
            self.title(),
            self.bool("AXModal"),
            self.element("AXMinimizeButton").is_some(),
            self.element("AXZoomButton").is_some(),
            self.element("AXCloseButton").is_some(),
            self.bool("AXMinimized"),
        )
    }
}
