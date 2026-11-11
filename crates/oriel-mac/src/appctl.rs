//! App control through AppKit: find, launch, hide, unhide, activate, quit.

use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplicationActivationOptions, NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{NSString, NSURL};
use std::path::Path;

/// The running instance of a bundle id, if any.
pub fn running(bundle_id: &str) -> Option<Retained<NSRunningApplication>> {
    let id = NSString::from_str(bundle_id);
    let list = NSRunningApplication::runningApplicationsWithBundleIdentifier(&id);
    list.iter().find(|a| !a.isTerminated())
}

pub fn pid_of(bundle_id: &str) -> Option<i32> {
    running(bundle_id).map(|a| a.processIdentifier())
}

pub fn by_pid(pid: i32) -> Option<Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).filter(|a| !a.isTerminated())
}

pub fn hide(bundle_id: &str) {
    if let Some(app) = running(bundle_id) {
        app.hide();
    }
}

/// The frontmost app's pid, from NSWorkspace (no call into that app).
pub fn frontmost_pid() -> Option<i32> {
    NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier())
}

/// Is the app hidden (Cmd+H)? False for an unknown pid.
pub fn is_hidden(pid: i32) -> bool {
    by_pid(pid).is_some_and(|a| a.isHidden())
}

pub fn unhide(pid: i32) {
    if let Some(app) = by_pid(pid) {
        app.unhide();
    }
}

pub fn unhide_bundle(bundle_id: &str) {
    if let Some(app) = running(bundle_id) {
        app.unhide();
    }
}

pub fn quit(bundle_id: &str) {
    if let Some(app) = running(bundle_id) {
        app.terminate();
    }
}

/// Ask the system to make the app frontmost. On recent macOS this can be refused for a
/// background process; the caller combines it with `AXFrontmost`.
#[allow(deprecated)]
pub fn activate(pid: i32) {
    if let Some(app) = by_pid(pid) {
        app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
    }
}

fn open(path: &Path, activates: bool) {
    let Some(s) = path.to_str() else { return };
    let url = NSURL::fileURLWithPath(&NSString::from_str(s));
    let config = NSWorkspaceOpenConfiguration::configuration();
    config.setActivates(activates);
    NSWorkspace::sharedWorkspace().openApplicationAtURL_configuration_completionHandler(&url, &config, None);
}

/// Launch an app by bundle path. The model presents the window later, so do not activate.
pub fn launch(path: &Path) {
    open(path, false);
}

/// Open the running app again: LaunchServices sends it the "reopen" event, as a Dock click.
pub fn reopen(bundle_id: &str) {
    let Some(app) = running(bundle_id) else { return };
    let Some(url) = app.bundleURL() else { return };
    let Some(path) = url.path() else { return };
    open(Path::new(&path.to_string()), true);
}
