//! Raw bindings for the Accessibility API and a few Core Foundation helpers.
//!
//! Everything here is a thin layer. The safe wrappers live in `ax.rs`.

use core_foundation_sys::base::{CFEqual, CFGetTypeID, CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::runloop::CFRunLoopSourceRef;
use core_foundation_sys::string::{
    CFStringCreateWithBytes, CFStringGetBytes, CFStringGetLength, CFStringGetTypeID, CFStringRef,
    kCFStringEncodingUTF8,
};
use std::ffi::c_void;
use std::ptr::NonNull;

pub type AXError = i32;
pub const AX_OK: AXError = 0;
pub const AX_VALUE_CGPOINT: i32 = 1;
pub const AX_VALUE_CGSIZE: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

pub type AXObserverCallback =
    extern "C" fn(observer: *mut c_void, element: *mut c_void, notification: CFStringRef, refcon: *mut c_void);

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub static kAXTrustedCheckOptionPrompt: CFStringRef;
    pub fn AXIsProcessTrusted() -> u8;
    pub fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    pub fn AXUIElementCreateApplication(pid: i32) -> *mut c_void;
    pub fn AXUIElementCopyAttributeValue(el: *mut c_void, attr: CFStringRef, value: *mut CFTypeRef) -> AXError;
    pub fn AXUIElementSetAttributeValue(el: *mut c_void, attr: CFStringRef, value: CFTypeRef) -> AXError;
    pub fn AXUIElementPerformAction(el: *mut c_void, action: CFStringRef) -> AXError;
    pub fn AXUIElementSetMessagingTimeout(el: *mut c_void, seconds: f32) -> AXError;
    pub fn AXValueCreate(ty: i32, value: *const c_void) -> CFTypeRef;
    pub fn AXValueGetValue(value: CFTypeRef, ty: i32, out: *mut c_void) -> u8;
    pub fn AXObserverCreate(pid: i32, cb: AXObserverCallback, out: *mut *mut c_void) -> AXError;
    pub fn AXObserverAddNotification(
        observer: *mut c_void,
        el: *mut c_void,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    pub fn AXObserverGetRunLoopSource(observer: *mut c_void) -> CFRunLoopSourceRef;
    /// Private but stable: every macOS window manager uses it.
    pub fn _AXUIElementGetWindow(el: *mut c_void, out: *mut u32) -> AXError;
}


/// An owned Core Foundation object. Released on drop.
pub struct Cf(NonNull<c_void>);

// SAFETY: Core Foundation objects are thread-safe to retain, release and compare. The AX
// elements and strings stored in `Cf` are immutable values.
unsafe impl Send for Cf {}
// SAFETY: see above.
unsafe impl Sync for Cf {}

impl Cf {
    /// Take ownership of a +1 reference (a "Create" or "Copy" result). `None` for null.
    ///
    /// # Safety
    /// `ptr` must be null or a valid CF object whose retain count the caller owns.
    pub unsafe fn from_owned(ptr: *const c_void) -> Option<Cf> {
        NonNull::new(ptr as *mut c_void).map(Cf)
    }

    /// Retain a borrowed object. `None` for null.
    ///
    /// # Safety
    /// `ptr` must be null or a valid CF object.
    pub unsafe fn retain(ptr: *const c_void) -> Option<Cf> {
        let nn = NonNull::new(ptr as *mut c_void)?;
        // SAFETY: the caller promises that `ptr` is a valid CF object.
        unsafe { CFRetain(nn.as_ptr() as CFTypeRef) };
        Some(Cf(nn))
    }

    pub fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

impl Clone for Cf {
    fn clone(&self) -> Self {
        // SAFETY: `self` owns a valid CF object.
        unsafe { CFRetain(self.as_ptr() as CFTypeRef) };
        Cf(self.0)
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: `self` owns one reference.
        unsafe { CFRelease(self.as_ptr() as CFTypeRef) };
    }
}

impl PartialEq for Cf {
    fn eq(&self, other: &Self) -> bool {
        // SAFETY: both pointers are valid CF objects.
        unsafe { CFEqual(self.as_ptr() as CFTypeRef, other.as_ptr() as CFTypeRef) != 0 }
    }
}

/// Make a CFString from a Rust string.
pub fn cfstr(s: &str) -> Option<Cf> {
    // SAFETY: the byte pointer and length come from a live `&str`; the result is a +1 object.
    unsafe {
        let r = CFStringCreateWithBytes(
            std::ptr::null(),
            s.as_ptr(),
            s.len() as isize,
            kCFStringEncodingUTF8,
            0,
        );
        Cf::from_owned(r as *const c_void)
    }
}

/// Read a CFString object as a Rust string. `None` if `obj` is not a CFString.
pub fn cf_to_string(obj: &Cf) -> Option<String> {
    cf_ptr_to_string(obj.as_ptr())
}

/// Read a borrowed CFString pointer. `None` if null or not a CFString.
pub fn cf_ptr_to_string(ptr: *const c_void) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `ptr` is non-null and valid by the caller's contract; the type is checked
    // before it is used as a string; the buffer is sized by CFStringGetBytes' own answer.
    unsafe {
        if CFGetTypeID(ptr as CFTypeRef) != CFStringGetTypeID() {
            return None;
        }
        let s = ptr as CFStringRef;
        let len = CFStringGetLength(s);
        let range = core_foundation_sys::base::CFRange { location: 0, length: len };
        let mut needed: isize = 0;
        CFStringGetBytes(s, range, kCFStringEncodingUTF8, 0, 0, std::ptr::null_mut(), 0, &mut needed);
        let mut buf = vec![0u8; needed.max(0) as usize];
        let mut used: isize = 0;
        CFStringGetBytes(
            s,
            range,
            kCFStringEncodingUTF8,
            0,
            0,
            buf.as_mut_ptr(),
            buf.len() as isize,
            &mut used,
        );
        buf.truncate(used.max(0) as usize);
        String::from_utf8(buf).ok()
    }
}
