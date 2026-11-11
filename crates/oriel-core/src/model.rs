//! The tab model: all of Oriel's decisions, with no OS and no UI.
//!
//! `Model::handle` takes one [`Input`] and returns the [`Effect`]s to perform, in order. It is
//! a pure state machine: the same inputs always give the same state and effects.
//!
//! The public items and their signatures in this file are the frozen contract. The bodies
//! belong to the oriel-core agent.
//!
//! # Rules
//!
//! Words: a *window tab* has `window: Some(..)`. A *placeholder* has `window: None`. An app is
//! *running* between `AppRunning` and `AppStopped`. `content` is `frame.content()`.
//!
//! ## Invariants (true after every `handle`)
//!
//! 1. Tabs of one app are contiguous in `tabs`.
//! 2. An app has window tabs or exactly one placeholder, never both, never two placeholders.
//! 3. `active` is `None` only when `tabs` is empty. Otherwise it names a tab in `tabs`.
//! 4. No two tabs have the same `WindowId`.
//! 5. When `split` is `Some`, both ids name existing window tabs, and they differ. A split
//!    stops existing in the same `handle` in which a member stops being a window tab (see
//!    "Split view").
//!
//! ## Presenting a tab (`present(tab)`, used by many rules below)
//!
//! Does nothing while `trusted` is false or `frame` is `None`. Otherwise:
//!
//! - Placeholder, in this order:
//!   1. `HideApp` for every *other* running app that has a tab, in tab order, once per app
//!      (the same step 1 as for a window tab). The old window must not stay under the bar.
//!   2. App not running: `Launch { app }`. App running: `Reopen { bundle_id }`.
//!
//!   No `Minimize`, no `Present`, no `RaiseBar`.
//! - Window tab, in this order (the new window first, so the content area is never empty: the
//!   platform holds the hides and minimizes until the presented window is on screen):
//!   1. `Present { bundle_id, window, frame: content }`.
//!   2. `Effect::RaiseBar`.
//!   3. `Minimize` for every other window tab of the same app, in tab order.
//!   4. `HideApp` for every *other* running app that has a tab, in tab order, once per app.
//!
//!   Every `present` first forgets the *shown* rects (see "Where the window is"). A window tab
//!   then has `shown = content` and no correction sent yet.
//! - Window tab that is a member of `split` (see "Split view"): both windows, in this order:
//!   1. `Present` for the other member, with its pane as `frame`.
//!   2. `Present` for `tab`, with its pane as `frame` (so the one asked for ends in front).
//!   3. `Effect::RaiseBar`.
//!   4. `Minimize` for every other window tab of the two apps that is not one of the two
//!      presented windows, in tab order.
//!   5. `HideApp` for every running app with a tab except the one or two apps of the pair, in
//!      tab order, once per app.
//!
//!   The two members can be two windows of one app. Both get `shown = pane`.
//!
//! ## Where the window is (`frame`, `shown`, *fit*)
//!
//! `frame` is the frame the USER chose: by a drag of the bar, or by a move or resize of the
//! glued window by hand. An app that refuses a size never changes `frame`, and the bar never
//! changes size because of a refusal.
//!
//! `shown` is the rect where each shown window really is, as far as the model knows: its
//! pane (`content` when there is no shown split) after a `present`, the actual frame after a
//! report. A shown split has two shown windows, each with its own pane as the "content" of
//! every rule below that talks about *fit*, a correction or a refusal. The *fit* of a window
//! of size `w x h` is the rect of that size, top-aligned under the bar (`y = content.y`) and
//! centered in `content` (`x = content.x + (content.w - w) / 2`); when `w > content.w` it is
//! left-aligned (`x = content.x`). A window that cannot take `content` is shown at its fit.
//! The model remembers at most one *correction* per window, per `present` and per `BarMoved`.
//!
//! ## Actions (from the UI)
//!
//! - `AddApp(app)`: the app already has a tab: same as `Activate` of its first tab. Else
//!   append a placeholder, make it active, emit `Watch { bundle_id }`, then `present` it (a
//!   `Launch`, since it is not known to run), then `Save`.
//! - `Activate(id)`: unknown id: nothing. Else forget the "closing by Oriel" mark of that
//!   tab's window (if it has one), cancel the expiring mark of the tab (it is a normal
//!   placeholder from now on: the user wants the app, so `present` launches or reopens it), make the tab active and `present` it, then `Save`.
//!   This also applies when the tab is already active (the user wants it back in front).
//! - `ExpireTab(id)`: the grace of an expiring placeholder has run out (see "Closed tabs are
//!   forgotten"). The tab is not an expiring placeholder (unknown id, or the mark is gone):
//!   nothing. Else remove the tab (see "Removing a tab"), after `Unwatch { bundle_id }`; then
//!   `Save`. Never `QuitApp` and never `ShowApp` (see there).
//! - `Close(id)`: unknown id: nothing.
//!   - Window tab: emit `CloseWindow { window }` and remember the window id as "closing by
//!     Oriel". Keep the tab until `WindowClosed` arrives (the app can ask to save, and the
//!     user can cancel). The mark must not stay for ever: see "The mark 'closing by Oriel'".
//!   - Placeholder (expiring or not): remove the tab (see "Removing a tab"). Emit `QuitApp` if
//!     the app runs, then `Unwatch`, then `Save`.
//! - `Move { from, to }`: move the tab at index `from` so it ends at index `to`, together
//!   with nothing else. Refuse (no change, no effects) when an index is out of range or the
//!   result breaks invariant 1. Emit `Save` when the order changed.
//! - `BarMoved(bar)`: the user moved or resized the bar window. `frame` is `None`: nothing.
//!   `bar` is `close_to` `frame.bar()`: nothing. Else `frame = Rect::from_bar(&bar, content
//!   height)`; for every shown window (the active window tab, or the two panes of a shown
//!   split, left first), emit `SetFrame { window, frame: target }` (and `shown = target`,
//!   correction forgotten), where `target` is the window's new pane (the new `content` for a
//!   single window) for a normal window, and the *fit* of the window's shown size in the new
//!   pane for a refusing window (`shown` is not `close_to` the old pane) when the bar only
//!   moved (the pane size is unchanged), so a fixed-size window follows the bar without a
//!   refused request on every drag step; if the pane size changed, the model asks for the full
//!   pane again (the window may take it now); then `Save`. Never emit `MoveBar` here.
//! - `Zoom(frame)`: the user asked for this whole Oriel frame (a double click on the bar's empty
//!   space zooms between the visible screen and the frame before). `frame` is `None`, or the new
//!   frame is `close_to` it: nothing. Else `frame = new`, emit `MoveBar(frame.bar())`; for every
//!   shown window emit `SetFrame { window, frame: pane }` (`shown = pane`, correction
//!   forgotten; a refusing window reports again and gets its fit); then `Save`.
//! - `RequestTrust`: emit `RequestTrust`.
//! - `OpenPicker`: emit `ScanApps`.
//! - `Split(tab)`, `Unsplit`, `SplitRatio(ratio)`: see "Split view".
//! - `Quit`: Oriel is about to exit. Emit `ShowApp` for every running app that has a tab, in
//!   tab order, once per app, then `Save`.
//!
//! ## Events (from the platform)
//!
//! - `TrustChanged(t)`: store it. On a change to true: `Watch` for every app that has a tab,
//!   in tab order, once per app. No change: nothing.
//! - `ScreenChanged { visible }`: store it. If `frame` is `None`: `frame = visible`, emit
//!   `MoveBar(frame.bar())`, `present` the active tab if there is one, then `Save`.
//!   Else, on the FIRST `ScreenChanged` only (the frame came from the saved state): when the
//!   frame's bar is not fully on `visible`, `frame = visible`; then emit
//!   `MoveBar(frame.bar())` so the bar window goes to its saved place. Later: nothing more.
//! - `AppsScanned(apps)`: `installed = apps`.
//! - `AppRunning { bundle_id, windows }`: no tab for this app: nothing. Else mark it running
//!   and make its tabs match `windows`: keep the tab (and its `TabId`) of every window id
//!   that already has one and update its title, add a tab for every new window (a
//!   placeholder becomes the first new window and keeps its `TabId`; more new windows go
//!   after the app's last tab, in the order given), remove the tabs of windows not in the
//!   list. With zero windows the app has one placeholder; when the app had window tabs
//!   before, that placeholder is *expiring* (the windows are gone: same as a native close of
//!   the last window, see "Closed tabs are forgotten"). A placeholder that was already
//!   expiring stays so and gets no new `ExpireTab`. A placeholder that becomes a window tab
//!   (the app has windows) is no longer expiring. Then: if the active tab belongs to this
//!   app, `present` the active tab (when the active tab itself was removed, the app's first
//!   tab becomes active; `present` does nothing for an expiring placeholder). Else emit
//!   `HideApp { bundle_id }`. Then `ExpireTab` for a newly expiring placeholder. Then `Save`
//!   when a tab became or stopped being expiring.
//! - `AppStopped { bundle_id }`: no tab for this app, or the app is not running (it never ran
//!   since Oriel began to watch it, or it was a placeholder at launch): nothing. Else mark it
//!   not running and replace its tabs with one *expiring* placeholder at the position of its
//!   first tab (it keeps that first tab's `TabId`). If one of its tabs was active, the
//!   placeholder is active. Forget its "closing by Oriel" window ids. No `present` (that would
//!   relaunch it). Then `ExpireTab { tab, after_ms: EXPIRE_GRACE_MS }` (also when the
//!   placeholder was expiring before: the grace starts again), then `Save`. The app stays
//!   watched during the grace.
//! - `WindowOpened { bundle_id, window }`: no tab for this app, or the window id already has
//!   a tab: nothing. Else mark the app running; a placeholder becomes this window (same
//!   `TabId`; an expiring mark is cancelled), or a new tab goes after the app's last tab. If
//!   the active tab belongs to this app, the new tab becomes active and is `present`ed. Else
//!   nothing more. Then `Save` if an expiring mark was cancelled.
//! - `WindowClosed { bundle_id, window }`: no tab for this window: nothing. Else:
//!   - The window id was "closing by Oriel" (forget it) and it is the app's last window
//!     tab: remove the tab, emit `QuitApp { bundle_id }` then `Unwatch { bundle_id }`.
//!   - It is the app's last window tab and Oriel did not close it (the user closed it in the
//!     app, by Cmd+W or the red button): the tab becomes an *expiring* placeholder (same
//!     `TabId`), then `ExpireTab { tab, after_ms: EXPIRE_GRACE_MS }`, then `Save`. The app is
//!     not quit and stays watched during the grace.
//!   - The app has other window tabs: remove the tab.
//!
//!   After a removal, see "Removing a tab". Then `Save` if a tab was removed.
//! - `WindowTitle { window, title }`: forget the "closing by Oriel" mark of that window, then
//!   update the title of that window's tab.
//! - `WindowFocused { bundle_id, window }`: no tab for this window: nothing. It is the
//!   active tab: emit only `Effect::RaiseBar`. It is a member of a *shown* split (the user
//!   clicked into the other pane): make it active, emit `RaiseBar` then `Save`; no `Present`,
//!   no hide, no minimize (the windows are in place). Else make it active, `present` it (a
//!   member of a split that is not shown brings the split back), then `Save`.
//! - `WindowFrame { window, frame: a, by_user }`: first forget the "closing by Oriel" mark of
//!   that window (always, even when nothing else happens). Then, when it is not a shown window
//!   (the active tab's window, or a pane of a shown split) or `frame` is `None`: nothing. In the
//!   rules below `content` is the window's pane. Else:
//!   - `by_user: false` (a refusal: the window is at `a`, not at what the model asked): `frame`
//!     does not change. If `a` is `close_to` `content`: `shown = a`, nothing else. Else
//!     compute `fit` from `content` and the size of `a`. If a correction was already sent to
//!     this window since the last `present`/`BarMoved`, or `a` is `close_to` `fit`: `shown = a`, nothing
//!     else (this stops a ping-pong when the OS clamps a position). Else `shown = fit`, mark
//!     the correction as sent, and emit `SetFrame { window, frame: fit }`. Never `MoveBar`, never
//!     `Save`.
//!   - `by_user: true` (a move or resize by hand) with `a` `close_to` the shown rect (or, with
//!     no shown rect, `close_to` `content`): nothing. Else, when the size of `a` is `close_to`
//!     the size of the shown rect, it is a *move*: translate `frame` by `a - shown` (the bar
//!     follows, the size stays), `shown = a`, correction forgotten. Else it is a *resize*:
//!     `frame = Rect::from_content(&a)`, `shown = a`, correction forgotten. Both emit
//!     `MoveBar(frame.bar())`, then `Save`. Never emit `SetFrame` here.
//!
//!   A normal window has `shown = content`, so for it a move gives the same `frame` as
//!   `from_content(a)`. For a pane of a shown split, `by_user: true` is ruled by "Split view".
//!
//! ## The mark "closing by Oriel"
//!
//! `Close` sets it for a window. The app can answer with a save dialog, and the user can
//! cancel it. Then the window stays open, and the mark would stay for ever. A later close of
//! that window by the user (as the app's last window) would then quit the app. So the mark of
//! a window is forgotten when the user shows that they still use the window:
//!
//! - `Activate` of the tab of that window.
//! - `WindowTitle` for that window.
//! - `WindowFrame` for that window (even when the frame is `close_to` the content).
//!
//! `WindowFocused` does NOT forget the mark. The platform sends it also when the app's main
//! window changes, and a close with a save dialog can do that for the window that is closing.
//! That echo would clear the mark of a window that is in fact closing. The mark is also
//! forgotten when its window is closed, when its tab is removed, and when its app stops or
//! reports a window list without it.
//!
//! ## Closed tabs are forgotten (the expiring placeholder)
//!
//! A tab that the user closed is gone, however it was closed, and Oriel does not bring it back
//! at the next start. Some closes are not real though: an app can close a splash window and
//! open its main window, restart itself (`AppStopped`, then `AppRunning` moments later), or the
//! platform can report `AppStopped` then `AppRunning` for an app that kept running. The model
//! has no clock, so it does not decide at once. It has two kinds of placeholder:
//!
//! - A *dormant* placeholder is a tab the user did not close: an app added with `AddApp` that
//!   did not run yet, or a saved app at launch. It never expires. The user removes it with "x".
//! - An *expiring* placeholder is a tab whose window or app went away after the app had run:
//!   the last window tab was closed in the app itself (`WindowClosed` that Oriel did not
//!   ask for), the app stopped (`AppStopped`), or `AppRunning` listed no window for an app that
//!   had window tabs. It is a placeholder (invariant 2 applies) with a mark in the model.
//!
//! Entering the mark always emits `Effect::ExpireTab { tab, after_ms: EXPIRE_GRACE_MS }`
//! (4000 ms). The app crate keeps one deadline per tab (a newer one for the same tab replaces
//! the older), and when it passes, dispatches `Action::ExpireTab(tab)`. During the grace:
//!
//! - `WindowOpened` or `AppRunning` with windows turns the placeholder into a window tab and
//!   cancels the mark (the app was only starting or restarting). `AppRunning` with no window
//!   changes nothing about the mark.
//! - `Activate` of the tab cancels the mark (the placeholder is then dormant, and `present`
//!   launches or reopens the app as before).
//! - `Close` ("x") removes the tab at once, as for any placeholder.
//! - `present` of an expiring placeholder does nothing: no `Launch`, no `Reopen` (that would
//!   bring back the window the user just closed).
//! - `Saved` leaves the tab out (`Model::saved`). While the active tab is expiring,
//!   `active_app` names the app that would become active if the tab were removed now. If Oriel
//!   quits during the grace, the tab is already forgotten.
//! - The UI shows the tab like a dormant one (`Model::is_expiring`).
//!
//! When `ExpireTab` arrives and the mark is still there, the tab is removed: `Unwatch` (the
//! app is no longer managed), then "Removing a tab" (the next tab becomes active and is
//! presented when the removed tab was active), then `Save`. The app is NOT quit: the user
//! closed a window (or the app is already gone). `ShowApp` is NOT emitted. If the removed tab
//! was the active one, its app was not hidden by Oriel. If it was not active, a hidden app that
//! closed its last window has no standard window to show, so `ShowApp` would show nothing but
//! pop other windows of the app up for no reason; macOS unhides an app by itself when the user
//! activates it.
//!
//! An `AddApp` placeholder is not marked while its app launches: its app is not running, so
//! `AppStopped` does nothing, and `WindowClosed` needs a window tab. Only a window tab or a
//! running app can enter the expiring state, so only an app that has run.
//!
//! `Action::Quit` (Oriel exits) is not a close: it forgets nothing.
//!
//! ## Removing a tab
//!
//! When the removed tab was a member of a *shown* split, the split is dissolved and the other
//! member becomes active and is `present`ed alone, at full size (this replaces the choice
//! below). Otherwise, when the removed tab was active: the tab now at the same index becomes
//! active, or the last tab when the index is past the end, or `None` when no tab is left.
//! `present` the new active tab.
//!
//! ## Split view
//!
//! `split` is `None` or `Split { left, right, ratio }`: two window tabs that are joined. They
//! need not be next to each other in the bar, and a split never reorders tabs (invariant 1
//! stays). `ratio` is the left pane's share of the usable content width. It is not saved: the
//! window ids do not survive a restart, so `Model::new` starts with `split: None`.
//!
//! The split is *shown* exactly when `split` is `Some` and `active` is one of its members. Then
//! two windows fill `content` below the one bar. Geometry (`types.rs`): `SPLIT_GAP` (8 pt) is
//! the divider's width; `MIN_PANE` (320 pt) is the narrowest pane. `ratio` is clamped so both
//! panes are at least `MIN_PANE` wide; when `content.w - SPLIT_GAP` is below `2 * MIN_PANE` it
//! is clamped to 0.2 to 0.8 instead. The left pane has width `round(usable * ratio)` with
//! `usable = content.w - SPLIT_GAP`, then come the divider and the right pane, which takes the
//! rest, so the three tile `content` exactly. All have the height of `content`.
//!
//! - `Split(tab)`: valid when `tab` and the active tab are different window tabs (else
//!   nothing). Forget the "closing by Oriel" mark of `tab`'s window, then `split = { left:
//!   active, right: tab, ratio: 0.5 }` (an older pair is replaced), `active = tab`, `present`
//!   `tab` (so both windows, see there), then `Save`.
//! - `Unsplit`: no split: nothing. Else `split = None`; when it was shown, `present` the active
//!   tab alone (full size). Nothing is saved.
//! - `SplitRatio(r)`: the user drags the divider. No shown split, no frame or not trusted:
//!   nothing. Else clamp `r`; when the left pane of the clamped ratio is `close_to` the left
//!   pane now: nothing. Else store the ratio and emit `SetFrame` for the left pane's window,
//!   then the right pane's, as for `BarMoved` (a refusing window gets its fit when the pane size
//!   did not change, else the full pane). No `Save`.
//! - `Activate(id)` of a member: it is active and `present`ed with its partner, also when the
//!   split is shown already. `Activate` of any other tab shows that tab at full size; `split`
//!   stays, so a later click on a member brings the pair back.
//! - `Effect::Divider`: [`Model::divider`] is the divider rect (global points) while the split
//!   is shown, Oriel is trusted and `frame` is known, else `None`. After every `handle`, when
//!   it is not what the last `Effect::Divider` said, `Effect::Divider(now)` is the LAST effect.
//! - `WindowFrame` with `by_user: true` for a pane of a shown split, `a` against the pane's
//!   shown rect (`close_to`: nothing):
//!   - A *move* (same size): `frame` is translated by `a - shown`; the ratio stays. Emit
//!     `MoveBar`, then `SetFrame` for the OTHER pane's window (as for `BarMoved`), then `Save`.
//!     The held window is never sent a `SetFrame`.
//!   - A *resize*: the frame's top and height come from `a` (`y = a.y - BAR_HEIGHT`, `h = a.h
//!     + BAR_HEIGHT`). Left pane: frame left = `a.x`, the right edge stays, the divider goes to
//!     `a.x + a.w`. Right pane: the left edge stays, the right edge = `a.x + a.w`, the divider
//!     goes to `a.x - SPLIT_GAP`. The ratio is the new left width over `usable`, clamped. If
//!     the frame would leave no room (`usable <= 0`) nothing happens. Emit `MoveBar`,
//!     `SetFrame` for the other pane (its full pane; a refusing window keeps its size if the
//!     pane size did not change), then `Save`. The held window is sent a `SetFrame` only when
//!     the clamp moved its pane away from `a` (`a` is not `close_to` the pane), and at most
//!     once per `present`/`BarMoved`/`Zoom` (the correction mark): so the model never fights a
//!     user who keeps pulling. After that the window stays where the user holds it.
//! - A member stops being a window tab (it was closed by "x" or in the app, its app stopped,
//!   `AppRunning` lists no such window, `Close` of a placeholder, `ExpireTab`): `split = None`
//!   in the same `handle`. When it was shown, the other member becomes active and is
//!   `present`ed alone, at full size, in place of the usual next active tab ("Removing a tab"
//!   and the `active` rules of `AppRunning`, `AppStopped` and `WindowClosed`). When the split
//!   was not shown, nothing else changes. If both members stop at once, the usual rules decide.
//! - `Quit` shows every running app, as always.
//!
//! `WindowFocused` for the windows of two `Present`s arrives twice, in order. The last one is
//! the frontmost, so `active` ends on it. A late echo of the first one finds a platform that
//! reads the real frontmost window, and the model never sends a command for a focus change
//! inside a shown split, so nothing can ping-pong.

use crate::platform::{Command, Event};
use crate::types::{
    AppInfo, BAR_HEIGHT, Rect, Saved, SPLIT_GAP, Split, Tab, TabId, WindowId, WindowInfo, clamp_ratio,
    ratio_for_left, split_rects,
};
use std::collections::BTreeSet;
use std::ops::Range;

/// What the UI asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// The user picked an app in the "+" picker.
    AddApp(AppInfo),
    /// The user clicked a tab.
    Activate(TabId),
    /// The user clicked a tab's close button.
    Close(TabId),
    /// The user dragged a tab to a new index.
    Move { from: usize, to: usize },
    /// The bar window now has this rect (global points, top-left origin).
    BarMoved(Rect),
    /// The user asked for this whole Oriel frame: bar plus content (zoom).
    Zoom(Rect),
    /// The user clicked "Grant access".
    RequestTrust,
    /// The picker opened.
    OpenPicker,
    /// Oriel is about to exit.
    Quit,
    /// The grace of an expiring placeholder has run out (the app crate sends it after the
    /// delay of `Effect::ExpireTab`). Does nothing when the tab is no longer expiring.
    ExpireTab(TabId),
    /// The user chose "Split with current tab" on this tab: show it side by side with the
    /// active tab. Does nothing unless both are window tabs and they differ.
    Split(TabId),
    /// The user ended the split view. Does nothing when there is no split.
    Unsplit,
    /// The user dragged the divider: the left pane's share of the usable content width.
    SplitRatio(f64),
}

/// One input to the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Action(Action),
    Event(Event),
}

/// How long an expiring placeholder waits before it is removed, in milliseconds.
pub const EXPIRE_GRACE_MS: u64 = 4000;

/// One thing for the app crate to do after `handle`.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Give this command to `Platform::send`.
    Platform(Command),
    /// Move and resize the bar window to this rect (global points, top-left origin).
    MoveBar(Rect),
    /// Put the bar window in front of other windows, without taking keyboard focus.
    RaiseBar,
    /// Write `Model::saved()` to disk.
    Save,
    /// Dispatch `Action::ExpireTab(tab)` after `after_ms` milliseconds. A newer `ExpireTab`
    /// effect for the same tab replaces the older one (the grace starts again).
    ExpireTab { tab: TabId, after_ms: u64 },
    /// Put the divider line of the split view at this rect (global points, top-left origin), or
    /// hide it (`None`). Sent whenever the answer of [`Model::divider`] changes, after the
    /// other effects of the same `handle`.
    Divider(Option<Rect>),
}

/// Oriel's whole state. The UI reads the public fields and never writes them.
#[derive(Clone, Debug, Default)]
pub struct Model {
    /// Does Oriel have Accessibility permission? The UI shows a "Grant access" bar when false.
    pub trusted: bool,
    /// The tabs, left to right.
    pub tabs: Vec<Tab>,
    /// The active tab.
    pub active: Option<TabId>,
    /// The Oriel frame: the bar plus the glued window. `None` until the screen is known.
    pub frame: Option<Rect>,
    /// The installed apps, for the picker. Empty until the first `AppsScanned`.
    pub installed: Vec<AppInfo>,
    /// The usable area of the main display.
    pub screen: Option<Rect>,
    /// The joined pair of the split view, if any (see "Split view"). Not saved.
    pub split: Option<Split>,
    // Private state of the implementation (running apps, closing windows, the id counter).
    pub(crate) inner: Inner,
}

/// Private bookkeeping. The oriel-core agent owns this type and may change its fields.
#[derive(Clone, Debug, Default)]
pub(crate) struct Inner {
    pub(crate) next_tab: u64,
    pub(crate) running: std::collections::BTreeSet<String>,
    pub(crate) closing: std::collections::BTreeSet<crate::types::WindowId>,
    /// Where each shown window is now (one window, or two in a shown split; see "Where the
    /// window is").
    pub(crate) shown: Vec<(WindowId, Rect)>,
    /// The windows that got a corrective `SetFrame` since the last `present`/`BarMoved`.
    pub(crate) corrected: BTreeSet<WindowId>,
    /// The divider rect last sent as `Effect::Divider`.
    pub(crate) divider: Option<Rect>,
    /// The expiring placeholders (see "Closed tabs are forgotten").
    pub(crate) expiring: BTreeSet<TabId>,
}

impl Model {
    /// The state at launch: one placeholder per saved app, the saved frame, and the
    /// placeholder of `saved.active_app` active (else the first tab). Not trusted yet.
    pub fn new(saved: Saved) -> Self {
        let mut model = Self::default();
        let active_app = saved.active_app;
        for app in saved.apps {
            if model.tabs.iter().any(|t| t.app.bundle_id == app.bundle_id) {
                continue;
            }
            let id = model.fresh_tab_id();
            model.tabs.push(Tab { id, app, window: None });
        }
        model.frame = saved.frame;
        let saved_active = active_app.and_then(|bundle_id| model.first_tab_of(&bundle_id));
        model.active = saved_active.or_else(|| model.tabs.first().map(|t| t.id));
        model
    }

    /// Apply one input and return the effects to perform, in order.
    pub fn handle(&mut self, input: Input) -> Vec<Effect> {
        let mut out = Vec::new();
        match input {
            Input::Action(action) => self.on_action(action, &mut out),
            Input::Event(event) => self.on_event(event, &mut out),
        }
        self.sync_divider(&mut out);
        out
    }

    /// What to write to disk now.
    pub fn saved(&self) -> Saved {
        let mut apps: Vec<AppInfo> = Vec::new();
        for tab in self.tabs.iter().filter(|t| !self.is_expiring(t.id)) {
            if !apps.iter().any(|a| a.bundle_id == tab.app.bundle_id) {
                apps.push(tab.app.clone());
            }
        }
        Saved {
            apps,
            frame: self.frame,
            active_app: self.saved_active_app(),
        }
    }

    /// The active app for `Saved`: the active tab's app, or while that tab is expiring the app
    /// that would become active if it were removed now ("Removing a tab"), as a saved state
    /// has no expiring tab.
    fn saved_active_app(&self) -> Option<String> {
        let index = self.active.and_then(|id| self.index_of(id))?;
        let kept = |t: &&Tab| !self.is_expiring(t.id);
        let tab = if self.is_expiring(self.tabs[index].id) {
            self.tabs[index + 1..].iter().find(kept).or_else(|| self.tabs.iter().rev().find(kept))
        } else {
            Some(&self.tabs[index])
        };
        tab.map(|t| t.app.bundle_id.clone())
    }

    /// The tab with this id.
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// The active tab.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.tab(id))
    }

    /// Is this tab an expiring placeholder (a closed tab in its grace)? The UI shows it dormant.
    pub fn is_expiring(&self, id: TabId) -> bool {
        self.inner.expiring.contains(&id)
    }

    /// Is this app running (as far as the platform told us)?
    pub fn is_running(&self, bundle_id: &str) -> bool {
        self.inner.running.contains(bundle_id)
    }

    /// The split, when it is *shown*: `split` is `Some` and the active tab is one of its two.
    pub fn shown_split(&self) -> Option<Split> {
        let split = self.split?;
        split.has(self.active?).then_some(split)
    }

    /// Is this tab one of the joined pair (shown or not)?
    pub fn in_split(&self, id: TabId) -> bool {
        self.split.is_some_and(|s| s.has(id))
    }

    /// Can `id` be split with the active tab? Both must be different window tabs.
    pub fn can_split_with_active(&self, id: TabId) -> bool {
        let Some(active) = self.active else { return false };
        active != id && self.window_of(active).is_some() && self.window_of(id).is_some()
    }

    /// Where the divider line is now, or `None` when it is hidden: the split is shown, Oriel is
    /// trusted and has a frame.
    pub fn divider(&self) -> Option<Rect> {
        if !self.trusted {
            return None;
        }
        let split = self.shown_split()?;
        let frame = self.frame?;
        Some(split_rects(&frame.content(), split.ratio).1)
    }
}

impl Model {
    fn on_action(&mut self, action: Action, out: &mut Vec<Effect>) {
        match action {
            Action::AddApp(app) => self.add_app(app, out),
            Action::Activate(id) => self.activate(id, out),
            Action::Close(id) => self.close(id, out),
            Action::Move { from, to } => self.move_tab(from, to, out),
            Action::BarMoved(bar) => self.bar_moved(bar, out),
            Action::Zoom(frame) => self.zoom(frame, out),
            Action::RequestTrust => out.push(Effect::Platform(Command::RequestTrust)),
            Action::OpenPicker => out.push(Effect::Platform(Command::ScanApps)),
            Action::Quit => self.quit(out),
            Action::ExpireTab(id) => self.expire_tab(id, out),
            Action::Split(id) => self.split_with_active(id, out),
            Action::Unsplit => self.unsplit(out),
            Action::SplitRatio(ratio) => self.split_ratio(ratio, out),
        }
    }

    fn on_event(&mut self, event: Event, out: &mut Vec<Effect>) {
        match event {
            Event::TrustChanged(trusted) => self.trust_changed(trusted, out),
            Event::ScreenChanged { visible } => self.screen_changed(visible, out),
            Event::AppsScanned(apps) => self.installed = apps,
            Event::AppRunning { bundle_id, windows } => self.app_running(bundle_id, windows, out),
            Event::AppStopped { bundle_id } => self.app_stopped(&bundle_id, out),
            Event::WindowOpened { bundle_id, window } => self.window_opened(bundle_id, window, out),
            Event::WindowClosed { bundle_id, window } => self.window_closed(&bundle_id, window, out),
            Event::WindowTitle { window, title } => self.window_title(window, title),
            Event::WindowFocused { bundle_id, window } => {
                self.window_focused(&bundle_id, window, out)
            }
            Event::WindowFrame { window, frame, by_user } => {
                self.window_frame(window, frame, by_user, out)
            }
        }
    }

    fn fresh_tab_id(&mut self) -> TabId {
        let id = TabId(self.inner.next_tab);
        self.inner.next_tab += 1;
        id
    }

    fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn window_index(&self, window: WindowId) -> Option<usize> {
        self.tabs.iter().position(|t| t.window.as_ref().is_some_and(|w| w.id == window))
    }

    /// The window of a window tab.
    fn window_of(&self, id: TabId) -> Option<WindowId> {
        self.tab(id).and_then(|t| t.window.as_ref()).map(|w| w.id)
    }

    /// The windows that should be on screen for `frame`, with the pane each one fills: the
    /// two panes of a shown split (left first), else the active window tab with the content.
    fn panes_at(&self, frame: &Rect) -> Vec<(WindowId, Rect)> {
        let content = frame.content();
        if let Some(split) = self.shown_split() {
            let (left, _, right) = split_rects(&content, split.ratio);
            if let (Some(l), Some(r)) = (self.window_of(split.left), self.window_of(split.right)) {
                return vec![(l, left), (r, right)];
            }
        }
        match self.active.and_then(|id| self.window_of(id)) {
            Some(window) => vec![(window, content)],
            None => Vec::new(),
        }
    }

    fn shown_of(&self, window: WindowId) -> Option<Rect> {
        self.inner.shown.iter().find(|(w, _)| *w == window).map(|(_, r)| *r)
    }

    fn set_shown(&mut self, window: WindowId, rect: Rect) {
        match self.inner.shown.iter_mut().find(|(w, _)| *w == window) {
            Some(entry) => entry.1 = rect,
            None => self.inner.shown.push((window, rect)),
        }
    }

    /// Emit `Effect::Divider` when [`Model::divider`] is not what was sent last.
    fn sync_divider(&mut self, out: &mut Vec<Effect>) {
        let now = self.divider();
        if now != self.inner.divider {
            self.inner.divider = now;
            out.push(Effect::Divider(now));
        }
    }

    /// Rule "A member stops being a window tab". After the tabs changed: when a member of the
    /// split is no longer a window tab, drop the split. `shown_before` says whether it was
    /// shown before the change. Then returns the surviving member when it was shown, which the
    /// caller makes active and presents at full size.
    fn dissolve_broken(&mut self, shown_before: bool) -> Option<TabId> {
        let split = self.split?;
        let is_window = |id: TabId| self.window_of(id).is_some();
        let (left, right) = (is_window(split.left), is_window(split.right));
        if left && right {
            return None;
        }
        self.split = None;
        match (shown_before, left, right) {
            (true, true, false) => Some(split.left),
            (true, false, true) => Some(split.right),
            _ => None,
        }
    }

    /// Send each window of `new` its pane. A window that could not take its old pane (it is
    /// shown elsewhere) and whose pane keeps its size, keeps its own size too: it gets the fit
    /// of its shown size in the new pane. Else it gets the full pane (it may take it now).
    fn retarget(&mut self, old: &[(WindowId, Rect)], new: &[(WindowId, Rect)], out: &mut Vec<Effect>) {
        let mut shown = Vec::new();
        for (index, &(window, pane)) in new.iter().enumerate() {
            let old_pane = old.get(index).map(|o| o.1).unwrap_or(pane);
            let target = match self.shown_of(window) {
                Some(rect) if !rect.close_to(&old_pane) && same_size(&old_pane, &pane) => fit(&pane, rect.w, rect.h),
                _ => pane,
            };
            shown.push((window, target));
            self.inner.corrected.remove(&window);
            out.push(Effect::Platform(Command::SetFrame { window, frame: target }));
        }
        if !shown.is_empty() {
            self.inner.shown = shown;
        }
    }

    fn first_tab_of(&self, bundle_id: &str) -> Option<TabId> {
        self.tabs.iter().find(|t| t.app.bundle_id == bundle_id).map(|t| t.id)
    }

    /// The positions of one app's tabs, or `None` when the app has no tab.
    fn app_range(&self, bundle_id: &str) -> Option<Range<usize>> {
        let start = self.tabs.iter().position(|t| t.app.bundle_id == bundle_id)?;
        let run = self.tabs[start..].iter().take_while(|t| t.app.bundle_id == bundle_id).count();
        Some(start..start + run)
    }

    /// The apps that have a tab, once each, in tab order.
    fn tab_apps(&self) -> Vec<&AppInfo> {
        let mut apps: Vec<&AppInfo> = Vec::new();
        for tab in &self.tabs {
            if !apps.iter().any(|a| a.bundle_id == tab.app.bundle_id) {
                apps.push(&tab.app);
            }
        }
        apps
    }

    /// Put `with` in place of the tabs at `range`.
    fn replace_tabs(&mut self, range: Range<usize>, with: Vec<Tab>) {
        let tail = self.tabs.split_off(range.end);
        self.tabs.truncate(range.start);
        self.tabs.extend(with);
        self.tabs.extend(tail);
    }

    /// Rule "Presenting a tab". Does nothing without trust or a frame.
    fn present(&mut self, id: TabId, out: &mut Vec<Effect>) {
        self.inner.shown.clear();
        self.inner.corrected.clear();
        if !self.trusted {
            return;
        }
        let Some(frame) = self.frame else {
            return;
        };
        if self.tab(id).is_none() {
            return;
        }
        if let Some(split) = self.split.filter(|s| s.has(id)) {
            if self.present_pair(id, split, &frame, out) {
                return;
            }
        }
        let Some(tab) = self.tab(id) else {
            return;
        };
        let bundle_id = tab.app.bundle_id.as_str();
        let Some(window) = tab.window.as_ref() else {
            if self.is_expiring(id) {
                // The user closed it: do not bring the window back.
                return;
            }
            // A placeholder: no other app stays on screen under the bar, then launch the app,
            // or reopen its window when it runs without one.
            self.hide_others(&[bundle_id], out);
            let command = if self.is_running(bundle_id) {
                Command::Reopen { bundle_id: bundle_id.to_string() }
            } else {
                Command::Launch { app: tab.app.clone() }
            };
            out.push(Effect::Platform(command));
            return;
        };
        let window = window.id;
        out.push(Effect::Platform(Command::Present {
            bundle_id: bundle_id.to_string(),
            window,
            frame: frame.content(),
        }));
        out.push(Effect::RaiseBar);
        for other in &self.tabs {
            if other.id == id || other.app.bundle_id != bundle_id {
                continue;
            }
            if let Some(sibling) = &other.window {
                out.push(Effect::Platform(Command::Minimize { window: sibling.id }));
            }
        }
        self.hide_others(&[bundle_id], out);
        self.inner.shown = vec![(window, frame.content())];
    }

    /// Rule "Presenting a tab" for a member of the split: both windows. False when a member is
    /// not a window tab (the caller then presents `id` alone).
    fn present_pair(&mut self, id: TabId, split: Split, frame: &Rect, out: &mut Vec<Effect>) -> bool {
        let Some(other) = split.partner(id) else { return false };
        let (Some(window), Some(other_window)) = (self.window_of(id), self.window_of(other)) else {
            return false;
        };
        let (Some(tab), Some(other_tab)) = (self.tab(id), self.tab(other)) else { return false };
        let (bundle_id, other_bundle) = (tab.app.bundle_id.clone(), other_tab.app.bundle_id.clone());
        let (left, _, right) = split_rects(&frame.content(), split.ratio);
        let pane = |t: TabId| if t == split.left { left } else { right };
        // The other pane first, so the one that was asked for ends in front.
        out.push(Effect::Platform(Command::Present {
            bundle_id: other_bundle.clone(),
            window: other_window,
            frame: pane(other),
        }));
        out.push(Effect::Platform(Command::Present { bundle_id: bundle_id.clone(), window, frame: pane(id) }));
        out.push(Effect::RaiseBar);
        for tab in &self.tabs {
            if tab.id == id || tab.id == other {
                continue;
            }
            if let Some(sibling) = &tab.window
                && (tab.app.bundle_id == bundle_id || tab.app.bundle_id == other_bundle)
            {
                out.push(Effect::Platform(Command::Minimize { window: sibling.id }));
            }
        }
        self.hide_others(&[&bundle_id, &other_bundle], out);
        self.inner.shown = vec![(other_window, pane(other)), (window, pane(id))];
        true
    }

    /// `HideApp` for every running app with a tab other than those in `keep`, in tab order,
    /// once per app.
    fn hide_others(&self, keep: &[&str], out: &mut Vec<Effect>) {
        let mut hidden: BTreeSet<&str> = BTreeSet::new();
        for other in &self.tabs {
            let other_app = other.app.bundle_id.as_str();
            if !keep.contains(&other_app) && self.is_running(other_app) && hidden.insert(other_app) {
                out.push(Effect::Platform(Command::HideApp { bundle_id: other_app.to_string() }));
            }
        }
    }

    fn split_shown_now(&self) -> bool {
        self.shown_split().is_some()
    }

    /// Rule "Removing a tab". The caller emits its own effects first.
    fn remove_tab(&mut self, index: usize, out: &mut Vec<Effect>) {
        let shown_before = self.split_shown_now();
        let removed = self.tabs.remove(index);
        self.inner.expiring.remove(&removed.id);
        if let Some(window) = &removed.window {
            self.inner.closing.remove(&window.id);
        }
        // A member of a shown split is gone: the other one is shown alone, at full size.
        if let Some(survivor) = self.dissolve_broken(shown_before) {
            self.active = Some(survivor);
            self.present(survivor, out);
            return;
        }
        if self.active != Some(removed.id) {
            return;
        }
        if self.tabs.is_empty() {
            self.active = None;
            return;
        }
        let next = self.tabs[index.min(self.tabs.len() - 1)].id;
        self.active = Some(next);
        self.present(next, out);
    }

    fn add_app(&mut self, app: AppInfo, out: &mut Vec<Effect>) {
        if let Some(first) = self.first_tab_of(&app.bundle_id) {
            self.activate(first, out);
            return;
        }
        let id = self.fresh_tab_id();
        let bundle_id = app.bundle_id.clone();
        self.tabs.push(Tab { id, app, window: None });
        self.active = Some(id);
        // Not known to run, so the placeholder launches it.
        self.inner.running.remove(&bundle_id);
        out.push(Effect::Platform(Command::Watch { bundle_id }));
        self.present(id, out);
        out.push(Effect::Save);
    }

    fn activate(&mut self, id: TabId, out: &mut Vec<Effect>) {
        if self.index_of(id).is_none() {
            return;
        }
        // The user clicked the tab: the window is still in use, so it is not closing.
        if let Some(window) = self.tabs.iter().find(|t| t.id == id).and_then(|t| t.window.as_ref()) {
            self.inner.closing.remove(&window.id);
        }
        self.inner.expiring.remove(&id);
        self.active = Some(id);
        self.present(id, out);
        out.push(Effect::Save);
    }

    /// Rule `Split`.
    fn split_with_active(&mut self, id: TabId, out: &mut Vec<Effect>) {
        if !self.can_split_with_active(id) {
            return;
        }
        let Some(active) = self.active else { return };
        // The user used the window: it is not closing.
        if let Some(window) = self.window_of(id) {
            self.inner.closing.remove(&window);
        }
        self.split = Some(Split { left: active, right: id, ratio: 0.5 });
        self.active = Some(id);
        self.present(id, out);
        out.push(Effect::Save);
    }

    /// Rule `Unsplit`.
    fn unsplit(&mut self, out: &mut Vec<Effect>) {
        let Some(_) = self.split else { return };
        let was_shown = self.shown_split().is_some();
        self.split = None;
        if was_shown && let Some(id) = self.active {
            self.present(id, out);
        }
    }

    /// Rule `SplitRatio`.
    fn split_ratio(&mut self, ratio: f64, out: &mut Vec<Effect>) {
        let (Some(frame), true) = (self.frame, self.trusted) else { return };
        let Some(split) = self.shown_split() else { return };
        let old = self.panes_at(&frame);
        let ratio = clamp_ratio(frame.content().w, ratio);
        let (new_left, _, _) = split_rects(&frame.content(), ratio);
        let (old_left, _, _) = split_rects(&frame.content(), split.ratio);
        if new_left.close_to(&old_left) {
            return;
        }
        if let Some(s) = self.split.as_mut() {
            s.ratio = ratio;
        }
        let new = self.panes_at(&frame);
        self.retarget(&old, &new, out);
    }

    /// Rule `ExpireTab`.
    fn expire_tab(&mut self, id: TabId, out: &mut Vec<Effect>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        if !self.is_expiring(id) {
            return;
        }
        let bundle_id = self.tabs[index].app.bundle_id.clone();
        out.push(Effect::Platform(Command::Unwatch { bundle_id: bundle_id.clone() }));
        self.inner.running.remove(&bundle_id);
        self.remove_tab(index, out);
        out.push(Effect::Save);
    }

    /// Turn the placeholder `id` into an expiring one and start its grace.
    fn start_grace(&mut self, id: TabId, out: &mut Vec<Effect>) {
        self.inner.expiring.insert(id);
        out.push(Effect::ExpireTab { tab: id, after_ms: EXPIRE_GRACE_MS });
    }

    fn close(&mut self, id: TabId, out: &mut Vec<Effect>) {
        let Some(index) = self.index_of(id) else {
            return;
        };
        let bundle_id = self.tabs[index].app.bundle_id.clone();
        if let Some(window) = self.tabs[index].window.as_ref().map(|w| w.id) {
            // The tab stays until WindowClosed arrives.
            self.inner.closing.insert(window);
            out.push(Effect::Platform(Command::CloseWindow { window }));
            return;
        }
        if self.is_running(&bundle_id) {
            out.push(Effect::Platform(Command::QuitApp { bundle_id: bundle_id.clone() }));
        }
        out.push(Effect::Platform(Command::Unwatch { bundle_id: bundle_id.clone() }));
        self.inner.running.remove(&bundle_id);
        self.remove_tab(index, out);
        out.push(Effect::Save);
    }

    fn move_tab(&mut self, from: usize, to: usize, out: &mut Vec<Effect>) {
        let len = self.tabs.len();
        if from == to || from >= len || to >= len {
            return;
        }
        let mut order = self.tabs.clone();
        let moved = order.remove(from);
        order.insert(to, moved);
        if !apps_are_contiguous(&order) {
            return;
        }
        self.tabs = order;
        out.push(Effect::Save);
    }

    fn bar_moved(&mut self, bar: Rect, out: &mut Vec<Effect>) {
        let Some(current) = self.frame else {
            return;
        };
        if bar.close_to(&current.bar()) {
            return;
        }
        let frame = Rect::from_bar(&bar, current.content().h);
        let old = self.panes_at(&current);
        self.frame = Some(frame);
        let new = self.panes_at(&frame);
        self.retarget(&old, &new, out);
        out.push(Effect::Save);
    }

    fn zoom(&mut self, frame: Rect, out: &mut Vec<Effect>) {
        let Some(current) = self.frame else {
            return;
        };
        if frame.close_to(&current) {
            return;
        }
        self.frame = Some(frame);
        out.push(Effect::MoveBar(frame.bar()));
        let panes = self.panes_at(&frame);
        for &(window, pane) in &panes {
            self.inner.corrected.remove(&window);
            out.push(Effect::Platform(Command::SetFrame { window, frame: pane }));
        }
        if !panes.is_empty() {
            self.inner.shown = panes;
        }
        out.push(Effect::Save);
    }

    fn quit(&self, out: &mut Vec<Effect>) {
        for app in self.tab_apps() {
            if self.is_running(&app.bundle_id) {
                out.push(Effect::Platform(Command::ShowApp { bundle_id: app.bundle_id.clone() }));
            }
        }
        out.push(Effect::Save);
    }

    fn trust_changed(&mut self, trusted: bool, out: &mut Vec<Effect>) {
        let was_trusted = std::mem::replace(&mut self.trusted, trusted);
        if trusted && !was_trusted {
            for app in self.tab_apps() {
                out.push(Effect::Platform(Command::Watch { bundle_id: app.bundle_id.clone() }));
            }
        }
    }

    fn screen_changed(&mut self, visible: Rect, out: &mut Vec<Effect>) {
        let first = self.screen.replace(visible).is_none();
        if let Some(saved) = self.frame {
            if first {
                // The frame came from the saved state: put the bar there, or reset a frame
                // that is off this screen.
                let bar = saved.bar();
                let on_screen = bar.x >= visible.x - 1.0
                    && bar.y >= visible.y - 1.0
                    && bar.x + bar.w <= visible.x + visible.w + 1.0
                    && bar.y + bar.h <= visible.y + visible.h + 1.0;
                let frame = if on_screen { saved } else { visible };
                self.frame = Some(frame);
                out.push(Effect::MoveBar(frame.bar()));
            }
            return;
        }
        self.frame = Some(visible);
        out.push(Effect::MoveBar(visible.bar()));
        if let Some(id) = self.active {
            self.present(id, out);
        }
        out.push(Effect::Save);
    }

    fn app_running(&mut self, bundle_id: String, windows: Vec<WindowInfo>, out: &mut Vec<Effect>) {
        let Some(range) = self.app_range(&bundle_id) else {
            return;
        };
        let Some(first) = self.tabs.get(range.start).cloned() else {
            return;
        };
        let old: Vec<Tab> = self.tabs[range.clone()].to_vec();
        let shown_before = self.split_shown_now();
        self.inner.running.insert(bundle_id.clone());
        let active_here = self.active.is_some_and(|id| old.iter().any(|t| t.id == id));

        // Distinct windows. A window that another app's tab already shows is left out.
        let mut wanted: Vec<WindowInfo> = Vec::new();
        for window in windows {
            let shown_elsewhere = self.tabs.iter().any(|t| {
                t.app.bundle_id != bundle_id && t.window.as_ref().is_some_and(|w| w.id == window.id)
            });
            if !shown_elsewhere && !wanted.iter().any(|w| w.id == window.id) {
                wanted.push(window);
            }
        }

        // Kept tabs: window tabs whose window is still listed, with the new title.
        let mut rebuilt: Vec<Tab> = old
            .iter()
            .filter_map(|tab| {
                let window = tab.window.as_ref()?;
                let current = wanted.iter().find(|w| w.id == window.id)?;
                Some(Tab { id: tab.id, app: tab.app.clone(), window: Some(current.clone()) })
            })
            .collect();

        // New windows go after the kept tabs. A placeholder's id goes to the first of them.
        let placeholder = old.iter().find(|t| t.window.is_none()).map(|t| t.id);
        let mut spare = placeholder;
        for window in &wanted {
            if old.iter().any(|t| t.window.as_ref().is_some_and(|w| w.id == window.id)) {
                continue;
            }
            let id = match spare.take() {
                Some(id) => id,
                None => self.fresh_tab_id(),
            };
            rebuilt.push(Tab { id, app: first.app.clone(), window: Some(window.clone()) });
        }

        // No window: the app keeps one placeholder, with the old placeholder's id or else the
        // id of its first tab.
        if rebuilt.is_empty() {
            let id = placeholder.unwrap_or(first.id);
            rebuilt.push(Tab { id, app: first.app.clone(), window: None });
        }

        // A window that is gone no longer counts as closed by Oriel.
        for window in old.iter().filter_map(|t| t.window.as_ref()) {
            if !rebuilt.iter().any(|t| t.window.as_ref().is_some_and(|w| w.id == window.id)) {
                self.inner.closing.remove(&window.id);
            }
        }

        // The expiring marks: a tab that shows a window or is gone is not expiring. Windows
        // that vanished from an app that had window tabs make an expiring placeholder.
        let mut changed = false;
        for tab in &old {
            let has_window = rebuilt.iter().any(|r| r.id == tab.id && r.window.is_some());
            let gone = !rebuilt.iter().any(|r| r.id == tab.id);
            if (has_window || gone) && self.inner.expiring.remove(&tab.id) {
                changed = true;
            }
        }
        let lost_windows = rebuilt.len() == 1
            && rebuilt[0].window.is_none()
            && old.iter().any(|t| t.window.is_some());
        let new_expiring = lost_windows.then(|| rebuilt[0].id);

        let active_kept = self.active.is_some_and(|id| rebuilt.iter().any(|t| t.id == id));
        let new_first = rebuilt.first().map(|t| t.id);
        self.replace_tabs(range, rebuilt);
        if let Some(id) = new_expiring {
            self.inner.expiring.insert(id);
        }

        if let Some(survivor) = self.dissolve_broken(shown_before) {
            // A member of the shown split is gone: the other one alone, at full size.
            self.active = Some(survivor);
            self.present(survivor, out);
        } else if !active_here {
            out.push(Effect::Platform(Command::HideApp { bundle_id }));
        } else {
            if !active_kept {
                self.active = new_first;
            }
            if let Some(id) = self.active {
                self.present(id, out);
            }
        }
        if let Some(id) = new_expiring {
            out.push(Effect::ExpireTab { tab: id, after_ms: EXPIRE_GRACE_MS });
        }
        if changed || new_expiring.is_some() {
            out.push(Effect::Save);
        }
    }

    fn app_stopped(&mut self, bundle_id: &str, out: &mut Vec<Effect>) {
        let Some(range) = self.app_range(bundle_id) else {
            return;
        };
        if !self.is_running(bundle_id) {
            // A dormant placeholder: the app never ran, so nothing was closed.
            return;
        }
        self.inner.running.remove(bundle_id);
        let shown_before = self.split_shown_now();
        let old: Vec<Tab> = self.tabs[range.clone()].to_vec();
        let Some(first) = old.first().cloned() else {
            return;
        };
        for tab in &old {
            self.inner.expiring.remove(&tab.id);
            if let Some(window) = &tab.window {
                self.inner.closing.remove(&window.id);
            }
        }
        let active_here = self.active.is_some_and(|id| old.iter().any(|t| t.id == id));
        let placeholder = Tab { id: first.id, app: first.app, window: None };
        let id = placeholder.id;
        self.replace_tabs(range, vec![placeholder]);
        if active_here {
            self.active = Some(id);
        }
        if let Some(survivor) = self.dissolve_broken(shown_before) {
            self.active = Some(survivor);
            self.present(survivor, out);
        }
        self.start_grace(id, out);
        out.push(Effect::Save);
    }

    fn window_opened(&mut self, bundle_id: String, window: WindowInfo, out: &mut Vec<Effect>) {
        let Some(range) = self.app_range(&bundle_id) else {
            return;
        };
        if self.window_index(window.id).is_some() {
            return;
        }
        self.inner.running.insert(bundle_id);
        let active_here = self.active.is_some_and(|id| self.tabs[range.clone()].iter().any(|t| t.id == id));
        let placeholder = self.tabs[range.clone()]
            .iter()
            .position(|t| t.window.is_none())
            .map(|p| range.start + p);
        let mut cancelled = false;
        let id = match placeholder {
            Some(index) => {
                self.tabs[index].window = Some(window);
                cancelled = self.inner.expiring.remove(&self.tabs[index].id);
                self.tabs[index].id
            }
            None => {
                let id = self.fresh_tab_id();
                let app = self.tabs[range.start].app.clone();
                self.tabs.insert(range.end, Tab { id, app, window: Some(window) });
                id
            }
        };
        if active_here {
            self.active = Some(id);
            self.present(id, out);
        }
        if cancelled {
            out.push(Effect::Save);
        }
    }

    fn window_closed(&mut self, bundle_id: &str, window: WindowId, out: &mut Vec<Effect>) {
        let Some(index) = self.window_index(window) else {
            return;
        };
        if self.tabs[index].app.bundle_id != bundle_id {
            return;
        }
        let was_closing = self.inner.closing.remove(&window);
        let shown_before = self.split_shown_now();
        let window_tabs = self
            .tabs
            .iter()
            .filter(|t| t.app.bundle_id == bundle_id && t.window.is_some())
            .count();
        if window_tabs > 1 {
            self.remove_tab(index, out);
        } else if was_closing {
            out.push(Effect::Platform(Command::QuitApp { bundle_id: bundle_id.to_string() }));
            out.push(Effect::Platform(Command::Unwatch { bundle_id: bundle_id.to_string() }));
            self.inner.running.remove(bundle_id);
            self.remove_tab(index, out);
        } else {
            // The user closed the app's last window in the app: an expiring placeholder.
            self.tabs[index].window = None;
            let id = self.tabs[index].id;
            if let Some(survivor) = self.dissolve_broken(shown_before) {
                self.active = Some(survivor);
                self.present(survivor, out);
            }
            self.start_grace(id, out);
        }
        out.push(Effect::Save);
    }

    fn window_title(&mut self, window: WindowId, title: String) {
        self.inner.closing.remove(&window);
        let Some(index) = self.window_index(window) else {
            return;
        };
        if let Some(info) = self.tabs[index].window.as_mut() {
            info.title = title;
        }
    }

    fn window_focused(&mut self, bundle_id: &str, window: WindowId, out: &mut Vec<Effect>) {
        let Some(index) = self.window_index(window) else {
            return;
        };
        if self.tabs[index].app.bundle_id != bundle_id {
            return;
        }
        let id = self.tabs[index].id;
        if self.active == Some(id) {
            out.push(Effect::RaiseBar);
            return;
        }
        if self.shown_split().is_some_and(|s| s.has(id)) {
            // The user clicked into the other pane: the windows are in place already.
            self.active = Some(id);
            out.push(Effect::RaiseBar);
            out.push(Effect::Save);
            return;
        }
        self.active = Some(id);
        self.present(id, out);
        out.push(Effect::Save);
    }

    fn window_frame(&mut self, window: WindowId, a: Rect, by_user: bool, out: &mut Vec<Effect>) {
        self.inner.closing.remove(&window);
        let Some(current) = self.frame else {
            return;
        };
        let panes = self.panes_at(&current);
        let Some(pane) = panes.iter().find(|(w, _)| *w == window).map(|(_, p)| *p) else {
            return;
        };
        if !by_user {
            self.refused(window, a, pane, out);
            return;
        }
        let shown = self.shown_of(window).unwrap_or(pane);
        if a.close_to(&shown) {
            return;
        }
        if self.shown_split().is_some() {
            self.split_user_frame(window, a, shown, current, &panes, out);
            return;
        }
        let new_frame = if same_size(&a, &shown) {
            // A move: the whole Oriel frame follows by the same delta.
            Rect::new(current.x + (a.x - shown.x), current.y + (a.y - shown.y), current.w, current.h)
        } else {
            Rect::from_content(&a)
        };
        self.frame = Some(new_frame);
        self.set_shown(window, a);
        self.inner.corrected.remove(&window);
        out.push(Effect::MoveBar(new_frame.bar()));
        out.push(Effect::Save);
    }

    /// Rule `WindowFrame` with `by_user: true` for a pane of a shown split. `a` is where the
    /// held window is, `shown` where the model thought it was.
    fn split_user_frame(
        &mut self,
        window: WindowId,
        a: Rect,
        shown: Rect,
        current: Rect,
        old: &[(WindowId, Rect)],
        out: &mut Vec<Effect>,
    ) {
        let Some(split) = self.split else { return };
        let moved = same_size(&a, &shown);
        let (new_frame, ratio) = if moved {
            // A move: the whole Oriel frame follows by the same delta, the panes keep their sizes.
            let frame = Rect::new(current.x + (a.x - shown.x), current.y + (a.y - shown.y), current.w, current.h);
            (frame, split.ratio)
        } else {
            let is_left = self.window_of(split.left) == Some(window);
            let right_edge = current.x + current.w;
            let top = a.y - BAR_HEIGHT;
            let height = a.h + BAR_HEIGHT;
            let (x, w, left_w) = if is_left {
                // The left pane sets the frame's left edge and the divider. The right edge stays.
                (a.x, right_edge - a.x, a.w)
            } else {
                // The right pane sets the frame's right edge and the divider. The left edge stays.
                (current.x, a.x + a.w - current.x, a.x - SPLIT_GAP - current.x)
            };
            if w - SPLIT_GAP <= 0.0 {
                return;
            }
            (Rect::new(x, top, w, height), clamp_ratio(w, ratio_for_left(w, left_w)))
        };
        self.frame = Some(new_frame);
        if let Some(s) = self.split.as_mut() {
            s.ratio = ratio;
        }
        self.set_shown(window, a);
        let new = self.panes_at(&new_frame);
        out.push(Effect::MoveBar(new_frame.bar()));
        for (index, &(w, pane)) in new.iter().enumerate() {
            if w == window {
                // Never fight the user: the held window is only corrected when the clamp moved
                // its pane, and once (until the next present, bar move or zoom).
                if !moved && !pane.close_to(&a) && self.inner.corrected.insert(w) {
                    self.set_shown(w, pane);
                    out.push(Effect::Platform(Command::SetFrame { window: w, frame: pane }));
                }
                continue;
            }
            let old_pane = old.get(index).map(|o| o.1).unwrap_or(pane);
            let target = match self.shown_of(w) {
                Some(rect) if !rect.close_to(&old_pane) && same_size(&old_pane, &pane) => fit(&pane, rect.w, rect.h),
                _ => pane,
            };
            self.set_shown(w, target);
            self.inner.corrected.remove(&w);
            out.push(Effect::Platform(Command::SetFrame { window: w, frame: target }));
        }
        out.push(Effect::Save);
    }

    /// Rule `WindowFrame` with `by_user: false`: the window took `a`, not what we asked for.
    fn refused(&mut self, window: WindowId, a: Rect, content: Rect, out: &mut Vec<Effect>) {
        if a.close_to(&content) {
            self.set_shown(window, a);
            return;
        }
        let target = fit(&content, a.w, a.h);
        if self.inner.corrected.contains(&window) || a.close_to(&target) {
            self.set_shown(window, a);
            return;
        }
        self.set_shown(window, target);
        self.inner.corrected.insert(window);
        out.push(Effect::Platform(Command::SetFrame { window, frame: target }));
    }
}

/// Do the two rects have the same size (within [`RECT_EPSILON`](crate::types::RECT_EPSILON))?
fn same_size(a: &Rect, b: &Rect) -> bool {
    (a.w - b.w).abs() < crate::types::RECT_EPSILON && (a.h - b.h).abs() < crate::types::RECT_EPSILON
}

/// The *fit* of a window of size `w x h` in `content` ([`Rect::fit`]).
fn fit(content: &Rect, w: f64, h: f64) -> Rect {
    content.fit(w, h)
}

/// Invariant 1: each app's tabs form one run.
fn apps_are_contiguous(tabs: &[Tab]) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut current: Option<&str> = None;
    for tab in tabs {
        let bundle_id = tab.app.bundle_id.as_str();
        if current != Some(bundle_id) {
            if !seen.insert(bundle_id) {
                return false;
            }
            current = Some(bundle_id);
        }
    }
    true
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
