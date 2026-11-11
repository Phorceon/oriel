//! The contract between the model and the operating system.
//!
//! The model never calls the OS. It returns [`Command`]s. The platform never changes tabs. It
//! reports [`Event`]s. The app crate moves both between them.

use crate::types::{AppInfo, Rect, WindowId, WindowInfo};
use std::sync::Mutex;

/// A request from the model to the OS. Every command is idempotent and must not fail loudly:
/// a command for an app that is not running, or a window that is gone, does nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Show the system Accessibility permission prompt (and open the Settings pane). The
    /// platform then checks about once a second and sends [`Event::TrustChanged`]`(true)`
    /// when the user grants access.
    RequestTrust,
    /// List the installed apps. Answer: [`Event::AppsScanned`].
    ScanApps,
    /// Start to watch an app, running or not. Answer now, if it runs: [`Event::AppRunning`].
    /// Later: every other app and window event for this bundle id, including a launch after
    /// this call. A second `Watch` for the same app sends [`Event::AppRunning`] again if it runs.
    Watch { bundle_id: String },
    /// Stop watching. No more events for this bundle id.
    Unwatch { bundle_id: String },
    /// Launch the app if it is not running. Do not wait. The launch shows as
    /// [`Event::AppRunning`] when the app is watched.
    Launch { app: AppInfo },
    /// The app runs but has no standard window: activate it and send the "reopen" Apple
    /// event (what a Dock icon click does), so it opens a window.
    Reopen { bundle_id: String },
    /// Make this window the visible one: set its frame FIRST (while its app is still hidden
    /// or the window still minimized, so it appears in place, with no jump), then unhide its
    /// app, un-minimize the window, raise it, and make its app frontmost.
    ///
    /// A window that refuses the size is put at the *fit* of the size it took
    /// ([`Rect::fit`] of `frame`) before it shows, and the refusal is reported with that rect.
    ///
    /// Order: every `HideApp` and `Minimize` sent after a `Present` waits until the presented
    /// window is on screen in place (at most about 0.6 s), so the content area under the bar
    /// is never empty. A newer `Present` cancels the wait for an older one, with one exception:
    /// split view sends two `Present`s in a row (the other pane first). Both windows are placed,
    /// shown and raised, in order, and the wait that holds the later hides and minimizes covers
    /// both windows. An older `Present` that came a moment before (about 0.1 s) and holds no
    /// command yet counts as the first of such a pair.
    Present { bundle_id: String, window: WindowId, frame: Rect },
    /// Only move and resize the window (the bar moved and the window follows). No raise,
    /// no activation. Called on every display frame during a drag (for two windows per frame in
    /// split view; every rule below holds for each window on its own), so:
    ///
    /// - newest wins: a frame not yet applied is replaced by a newer one for that window;
    /// - it runs on a thread of its own that nothing else blocks, and never waits for the
    ///   caller;
    /// - when the size is the size last applied, it only sets the position (one IPC call);
    /// - the window is read back once, when no `SetFrame` came for that window for a short
    ///   moment (about 0.1 s): one refusal report per burst, not per frame. A refusing
    ///   window is then put at its fit, as for `Present`.
    SetFrame { window: WindowId, frame: Rect },
    /// Hide the app (as Cmd+H). Its windows leave the screen and Mission Control. Waits for a
    /// pending `Present` (see there).
    HideApp { bundle_id: String },
    /// Unhide the app without activating it.
    ShowApp { bundle_id: String },
    /// Minimize one window. Waits for a pending `Present` (see there).
    Minimize { window: WindowId },
    /// Close one window (press its close button). The app can ask to save.
    CloseWindow { window: WindowId },
    /// Quit the app politely (as Cmd+Q). The app can ask to save.
    QuitApp { bundle_id: String },
}

/// A fact from the OS for the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Sent once after `Platform::start`, then on every change: does Oriel have
    /// Accessibility permission?
    TrustChanged(bool),
    /// Sent once after `Platform::start`, then on every change: the usable area of the main
    /// display (without the menu bar and the Dock).
    ScreenChanged { visible: Rect },
    /// The answer to [`Command::ScanApps`], sorted by name, without duplicate bundle ids.
    AppsScanned(Vec<AppInfo>),
    /// A watched app runs. `windows` are its standard windows now, front to back. Sent on
    /// `Watch` of a running app and when a watched app finishes launching.
    AppRunning { bundle_id: String, windows: Vec<WindowInfo> },
    /// A watched app quit (or crashed).
    AppStopped { bundle_id: String },
    /// A watched app opened a standard window. Dialogs, sheets, popovers, palettes and
    /// tooltips are not standard windows and are never reported.
    WindowOpened { bundle_id: String, window: WindowInfo },
    /// A reported window closed.
    WindowClosed { bundle_id: String, window: WindowId },
    /// A reported window changed its title.
    WindowTitle { window: WindowId, title: String },
    /// The user brought a watched app's window to the front by a path outside Oriel
    /// (Cmd+Tab, the Dock, a click on the window, the app's Window menu). Sent every time a
    /// watched app becomes frontmost and every time its main window changes while it is
    /// frontmost. Also sent for the window of a `Present` (the model ignores that echo).
    WindowFocused { bundle_id: String, window: WindowId },
    /// A reported window is now at `frame` and the model did not ask for that.
    ///
    /// `by_user: false`: a refusal. The platform ran `Present` or `SetFrame`, read the window
    /// back, and the real frame was not `close_to` the requested one (the app has a fixed or
    /// minimum size, or the OS clamped the position). The platform sends it once per
    /// `Present` and once per `SetFrame` burst, with the frame the window took (its fit when
    /// the size was refused).
    ///
    /// `by_user: true`: a moved/resized notification that is not an echo: the user (or
    /// another program) moved or resized the window.
    ///
    /// Echo filter: for a `by_user: true` report the platform must NOT send a frame that is
    /// `close_to` the last frame it requested on that window, or `close_to` the last frame it
    /// read back (the notifications that follow a refused set are not user moves). While a
    /// `SetFrame` burst runs on a window, and until its read-back, the platform sends no
    /// `by_user: true` report for that window at all: its notifications trail the requests
    /// and are echoes.
    WindowFrame { window: WindowId, frame: Rect, by_user: bool },
}

/// Where a platform delivers its events. Called from any thread.
pub type EventSink = Box<dyn Fn(Event) + Send + Sync + 'static>;

/// The operating system, as Oriel sees it.
pub trait Platform: Send + Sync + 'static {
    /// Called once, before any `send`. The platform keeps the sink and may call it from any
    /// thread. Soon after `start` it sends [`Event::TrustChanged`] and [`Event::ScreenChanged`].
    fn start(&self, sink: EventSink);

    /// Run one command. Must not block the caller: the UI thread calls this, sometimes 120
    /// times a second ([`Command::SetFrame`]). Callable from any thread.
    fn send(&self, command: Command);

    /// Block until every command sent before this call has been executed, or until `timeout`
    /// has passed. Oriel calls it before it exits, so that `ShowApp` is not lost. The default
    /// does nothing (a platform that runs commands inside `send` has nothing to wait for).
    fn flush(&self, timeout: std::time::Duration) {
        let _ = timeout;
    }
}

/// A platform with no OS behind it, for tests and for headless UI runs. It records commands
/// and answers the few that need an answer.
#[derive(Default)]
pub struct FakePlatform {
    sink: Mutex<Option<EventSink>>,
    commands: Mutex<Vec<Command>>,
}

impl FakePlatform {
    pub fn new() -> Self {
        Self::default()
    }

    /// Deliver an event as if the OS sent it.
    pub fn emit(&self, event: Event) {
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink(event);
        }
    }

    /// Take the commands received so far.
    pub fn take_commands(&self) -> Vec<Command> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }

    /// The apps `ScanApps` answers with.
    pub fn sample_apps() -> Vec<AppInfo> {
        [
            ("com.apple.Safari", "Safari", "/Applications/Safari.app"),
            ("com.apple.Notes", "Notes", "/System/Applications/Notes.app"),
            ("com.apple.Music", "Music", "/System/Applications/Music.app"),
            ("com.apple.mail", "Mail", "/System/Applications/Mail.app"),
            ("com.apple.iCal", "Calendar", "/System/Applications/Calendar.app"),
        ]
        .into_iter()
        .map(|(bundle_id, name, path)| AppInfo {
            bundle_id: bundle_id.into(),
            name: name.into(),
            path: path.into(),
        })
        .collect()
    }
}

impl Platform for FakePlatform {
    fn start(&self, sink: EventSink) {
        sink(Event::TrustChanged(true));
        sink(Event::ScreenChanged { visible: Rect::new(0.0, 25.0, 1440.0, 875.0) });
        *self.sink.lock().unwrap() = Some(sink);
    }

    fn send(&self, command: Command) {
        let answer = match &command {
            Command::ScanApps => Some(Event::AppsScanned(Self::sample_apps())),
            Command::RequestTrust => Some(Event::TrustChanged(true)),
            _ => None,
        };
        self.commands.lock().unwrap().push(command);
        if let Some(event) = answer {
            self.emit(event);
        }
    }
}
