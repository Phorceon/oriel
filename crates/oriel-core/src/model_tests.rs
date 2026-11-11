//! Unit tests for the tab model. Included from `model.rs` as a child module.

use super::{Action, Effect, Input, Model, EXPIRE_GRACE_MS};
use crate::types::{MIN_PANE, SPLIT_GAP, Split, clamp_ratio, split_rects};
use crate::platform::{Command, Event};
use crate::types::{AppInfo, Rect, Saved, Tab, TabId, WindowId, WindowInfo};
use std::collections::BTreeSet;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn app(bundle_id: &str) -> AppInfo {
    AppInfo {
        bundle_id: bundle_id.into(),
        name: format!("App {bundle_id}"),
        path: PathBuf::from(format!("/Applications/{bundle_id}.app")),
    }
}

fn win(id: u32, title: &str) -> WindowInfo {
    WindowInfo { id: WindowId(id), title: title.into() }
}

/// The Oriel frame used by most tests. Its content rect is `content()`.
fn screen() -> Rect {
    Rect::new(0.0, 25.0, 1440.0, 875.0)
}

fn content() -> Rect {
    screen().content()
}

/// A trusted model with the given apps as placeholders. The first app is active, and the
/// screen is known. The effects of this setup are dropped.
fn model_with(bundle_ids: &[&str]) -> Model {
    let saved = Saved {
        apps: bundle_ids.iter().map(|&b| app(b)).collect(),
        frame: None,
        active_app: None,
    };
    let mut m = Model::new(saved);
    ev(&mut m, Event::TrustChanged(true));
    ev(&mut m, Event::ScreenChanged { visible: screen() });
    m
}

fn act(m: &mut Model, action: Action) -> Vec<Effect> {
    m.handle(Input::Action(action))
}

fn ev(m: &mut Model, event: Event) -> Vec<Effect> {
    m.handle(Input::Event(event))
}

/// Click the first tab of an app.
fn activate_app(m: &mut Model, bundle_id: &str) -> Vec<Effect> {
    let id = app_tab(m, bundle_id);
    act(m, Action::Activate(id))
}

/// Click the tab that shows a window.
fn activate_window(m: &mut Model, window: u32) -> Vec<Effect> {
    let id = window_tab(m, window);
    act(m, Action::Activate(id))
}

/// Click the close button of the first tab of an app.
fn close_app(m: &mut Model, bundle_id: &str) -> Vec<Effect> {
    let id = app_tab(m, bundle_id);
    act(m, Action::Close(id))
}

/// Click the close button of the tab that shows a window.
fn close_window_tab(m: &mut Model, window: u32) -> Vec<Effect> {
    let id = window_tab(m, window);
    act(m, Action::Close(id))
}

fn running(m: &mut Model, bundle_id: &str, windows: Vec<WindowInfo>) -> Vec<Effect> {
    ev(m, Event::AppRunning { bundle_id: bundle_id.into(), windows })
}

fn opened(m: &mut Model, bundle_id: &str, window: WindowInfo) -> Vec<Effect> {
    ev(m, Event::WindowOpened { bundle_id: bundle_id.into(), window })
}

fn closed(m: &mut Model, bundle_id: &str, window: u32) -> Vec<Effect> {
    ev(m, Event::WindowClosed { bundle_id: bundle_id.into(), window: WindowId(window) })
}

fn none() -> Vec<Effect> {
    Vec::new()
}

fn expire(tab: TabId) -> Effect {
    Effect::ExpireTab { tab, after_ms: EXPIRE_GRACE_MS }
}

fn stopped(m: &mut Model, bundle_id: &str) -> Vec<Effect> {
    ev(m, Event::AppStopped { bundle_id: bundle_id.into() })
}

/// The grace of a tab has run out.
fn expired(m: &mut Model, tab: TabId) -> Vec<Effect> {
    act(m, Action::ExpireTab(tab))
}

fn cmd(command: Command) -> Effect {
    Effect::Platform(command)
}

fn watch(b: &str) -> Effect {
    cmd(Command::Watch { bundle_id: b.into() })
}

fn unwatch(b: &str) -> Effect {
    cmd(Command::Unwatch { bundle_id: b.into() })
}

fn launch(b: &str) -> Effect {
    cmd(Command::Launch { app: app(b) })
}

fn reopen(b: &str) -> Effect {
    cmd(Command::Reopen { bundle_id: b.into() })
}

fn hide(b: &str) -> Effect {
    cmd(Command::HideApp { bundle_id: b.into() })
}

fn show(b: &str) -> Effect {
    cmd(Command::ShowApp { bundle_id: b.into() })
}

fn quit_app(b: &str) -> Effect {
    cmd(Command::QuitApp { bundle_id: b.into() })
}

fn present(b: &str, w: u32) -> Effect {
    cmd(Command::Present { bundle_id: b.into(), window: WindowId(w), frame: content() })
}

fn minimize(w: u32) -> Effect {
    cmd(Command::Minimize { window: WindowId(w) })
}

fn close_window(w: u32) -> Effect {
    cmd(Command::CloseWindow { window: WindowId(w) })
}

fn set_frame(w: u32, frame: Rect) -> Effect {
    cmd(Command::SetFrame { window: WindowId(w), frame })
}

fn raise() -> Effect {
    Effect::RaiseBar
}

fn save() -> Effect {
    Effect::Save
}

fn move_bar(bar: Rect) -> Effect {
    Effect::MoveBar(bar)
}

/// The app of each tab, in tab order.
fn bundles(m: &Model) -> Vec<&str> {
    m.tabs.iter().map(|t| t.app.bundle_id.as_str()).collect()
}

/// The window of each tab, in tab order (`None` for a placeholder).
fn windows(m: &Model) -> Vec<Option<u32>> {
    m.tabs.iter().map(|t| t.window.as_ref().map(|w| w.id.0)).collect()
}

/// The id of the first tab of an app.
fn app_tab(m: &Model, bundle_id: &str) -> TabId {
    m.tabs.iter().find(|t| t.app.bundle_id == bundle_id).expect("app has a tab").id
}

/// The id of the tab that shows a window.
fn window_tab(m: &Model, window: u32) -> TabId {
    m.tabs
        .iter()
        .find(|t| t.window.as_ref().map(|w| w.id) == Some(WindowId(window)))
        .expect("window has a tab")
        .id
}

/// Everything the model holds, as text. Two snapshots are equal when the state is equal.
fn snapshot(m: &Model) -> String {
    format!("{m:?}")
}

/// The four invariants, plus: a running app always has a tab.
fn assert_invariants(m: &Model) {
    // Invariant 1: each app's tabs are contiguous.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut current: Option<&str> = None;
    for tab in &m.tabs {
        let bundle = tab.app.bundle_id.as_str();
        if current != Some(bundle) {
            assert!(seen.insert(bundle), "tabs of {bundle} are not contiguous");
            current = Some(bundle);
        }
    }
    // Invariant 2: an app has window tabs, or exactly one placeholder, never both.
    for bundle in &seen {
        let tabs: Vec<&Tab> = m.tabs.iter().filter(|t| t.app.bundle_id == *bundle).collect();
        let placeholders = tabs.iter().filter(|t| t.window.is_none()).count();
        assert!(placeholders <= 1, "{bundle} has {placeholders} placeholders");
        assert!(placeholders == 0 || tabs.len() == 1, "{bundle} has a placeholder and windows");
    }
    // Invariant 3: `active` names a tab, unless there are no tabs.
    match m.active {
        None => assert!(m.tabs.is_empty(), "no active tab, but tabs exist"),
        Some(id) => assert!(m.tabs.iter().any(|t| t.id == id), "active tab {id:?} is missing"),
    }
    // Invariant 4: no two tabs show the same window, and no two tabs share an id.
    let mut windows_shown = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for tab in &m.tabs {
        assert!(ids.insert(tab.id), "tab id {:?} used twice", tab.id);
        if let Some(w) = &tab.window {
            assert!(windows_shown.insert(w.id), "window {:?} shown twice", w.id);
        }
    }
    // Invariant 5: a split names two different window tabs.
    if let Some(split) = m.split {
        assert_ne!(split.left, split.right, "split joins a tab with itself");
        for id in [split.left, split.right] {
            let tab = m.tab(id).unwrap_or_else(|| panic!("split names a missing tab {id:?}"));
            assert!(tab.window.is_some(), "split names the placeholder {id:?}");
        }
    }
    // Extra: the divider the model last announced is the divider it computes now.
    assert_eq!(m.inner.divider, m.divider(), "the divider effect is out of date");
    // Extra: an expiring tab is a placeholder, and the saved state does not list its app
    // (an app has one placeholder, so it has no other tab).
    let saved = m.saved();
    for id in &m.inner.expiring {
        let tab = m.tab(*id).expect("an expiring tab exists");
        assert!(tab.window.is_none(), "expiring tab {id:?} has a window");
        assert!(saved.apps.iter().all(|a| a.bundle_id != tab.app.bundle_id));
    }
    // Extra: a running app always has a tab.
    for bundle in &m.inner.running {
        assert!(
            m.tabs.iter().any(|t| t.app.bundle_id == *bundle),
            "{bundle} runs but has no tab"
        );
    }
}

// ---------------------------------------------------------------------------
// Model::new and Model::saved
// ---------------------------------------------------------------------------

#[test]
fn new_makes_one_placeholder_per_saved_app() {
    let saved = Saved {
        apps: vec![app("a"), app("b")],
        frame: Some(screen()),
        active_app: Some("b".into()),
    };
    let m = Model::new(saved);
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(windows(&m), vec![None, None]);
    assert_eq!(m.active, Some(m.tabs[1].id));
    assert_eq!(m.frame, Some(screen()));
    assert!(!m.trusted);
    assert_eq!(m.screen, None);
    assert!(m.installed.is_empty());
    assert!(!m.is_running("a"));
}

#[test]
fn new_falls_back_to_first_tab_when_active_app_is_unknown() {
    let saved = Saved {
        apps: vec![app("a"), app("b")],
        frame: None,
        active_app: Some("zzz".into()),
    };
    let m = Model::new(saved);
    assert_eq!(m.active, Some(m.tabs[0].id));
}

#[test]
fn new_from_empty_saved_has_no_tabs_and_no_active_tab() {
    let m = Model::new(Saved::default());
    assert!(m.tabs.is_empty());
    assert_eq!(m.active, None);
    assert_eq!(m.frame, None);
    assert_eq!(m.saved(), Saved::default());
}

#[test]
fn new_ignores_duplicate_saved_apps() {
    let saved = Saved { apps: vec![app("a"), app("a"), app("b")], frame: None, active_app: None };
    let m = Model::new(saved);
    assert_eq!(bundles(&m), vec!["a", "b"]);
}

#[test]
fn saved_round_trips_apps_frame_and_active_app() {
    let saved = Saved {
        apps: vec![app("a"), app("b"), app("c")],
        frame: Some(screen()),
        active_app: Some("c".into()),
    };
    let m = Model::new(saved.clone());
    assert_eq!(m.saved(), saved);
}

#[test]
fn saved_lists_each_app_once_in_tab_order_and_follows_the_active_tab() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "one"), win(2, "two")]);
    running(&mut m, "b", vec![win(3, "three")]);
    let saved = m.saved();
    let apps: Vec<&str> = saved.apps.iter().map(|a| a.bundle_id.as_str()).collect();
    assert_eq!(apps, vec!["a", "b"]);
    assert_eq!(saved.active_app.as_deref(), Some("a"));

    activate_window(&mut m, 3);
    assert_eq!(m.saved().active_app.as_deref(), Some("b"));
}

// ---------------------------------------------------------------------------
// Trust, screen, scan
// ---------------------------------------------------------------------------

#[test]
fn trust_granted_watches_each_app_once_and_only_on_change_to_true() {
    let mut m = Model::new(Saved {
        apps: vec![app("a"), app("b")],
        frame: None,
        active_app: None,
    });
    assert_eq!(ev(&mut m, Event::TrustChanged(true)), vec![watch("a"), watch("b")]);
    assert!(m.trusted);
    assert_eq!(ev(&mut m, Event::TrustChanged(true)), none());
    assert_eq!(ev(&mut m, Event::TrustChanged(false)), none());
    assert!(!m.trusted);
    assert_eq!(ev(&mut m, Event::TrustChanged(true)), vec![watch("a"), watch("b")]);
}

#[test]
fn first_screen_sets_frame_and_moves_bar_while_untrusted() {
    let mut m = Model::new(Saved { apps: vec![app("a")], frame: None, active_app: None });
    let effects = ev(&mut m, Event::ScreenChanged { visible: screen() });
    assert_eq!(effects, vec![move_bar(screen().bar()), save()]);
    assert_eq!(m.frame, Some(screen()));
    assert_eq!(m.screen, Some(screen()));
}

#[test]
fn first_screen_presents_the_active_tab_when_trusted() {
    let mut m = Model::new(Saved { apps: vec![app("a")], frame: None, active_app: None });
    ev(&mut m, Event::TrustChanged(true));
    let effects = ev(&mut m, Event::ScreenChanged { visible: screen() });
    assert_eq!(effects, vec![move_bar(screen().bar()), launch("a"), save()]);
}

#[test]
fn later_screen_only_stores_the_screen() {
    let mut m = model_with(&["a"]);
    let bigger = Rect::new(0.0, 25.0, 1280.0, 800.0);
    let effects = ev(&mut m, Event::ScreenChanged { visible: bigger });
    assert_eq!(effects, none());
    assert_eq!(m.frame, Some(screen()));
    assert_eq!(m.screen, Some(bigger));
}

#[test]
fn apps_scanned_sets_the_picker_list() {
    let mut m = Model::new(Saved::default());
    assert_eq!(ev(&mut m, Event::AppsScanned(vec![app("a")])), none());
    assert_eq!(m.installed, vec![app("a")]);
}

#[test]
fn open_picker_and_request_trust_emit_their_commands() {
    let mut m = Model::new(Saved::default());
    assert_eq!(act(&mut m, Action::RequestTrust), vec![cmd(Command::RequestTrust)]);
    assert_eq!(act(&mut m, Action::OpenPicker), vec![cmd(Command::ScanApps)]);
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

#[test]
fn add_new_app_watches_launches_and_saves() {
    let mut m = model_with(&["a"]);
    let effects = act(&mut m, Action::AddApp(app("b")));
    assert_eq!(effects, vec![watch("b"), launch("b"), save()]);
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(m.active, Some(app_tab(&m, "b")));
    assert!(!m.is_running("b"));
}

#[test]
fn add_existing_app_activates_its_first_tab() {
    let mut m = model_with(&["a", "b"]);
    assert_eq!(activate_app(&mut m, "b"), vec![launch("b"), save()]);
    let effects = act(&mut m, Action::AddApp(app("a")));
    assert_eq!(effects, vec![launch("a"), save()]);
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(m.active, Some(app_tab(&m, "a")));
}

#[test]
fn add_app_while_untrusted_only_watches_and_saves() {
    let mut m = Model::new(Saved::default());
    let effects = act(&mut m, Action::AddApp(app("a")));
    assert_eq!(effects, vec![watch("a"), save()]);
    assert_eq!(bundles(&m), vec!["a"]);
}

#[test]
fn activate_window_tab_hides_others_minimizes_siblings_in_order() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    // Tabs: a1 (active), a2, b3. Both apps run.

    let effects = activate_window(&mut m, 3);
    assert_eq!(effects, vec![present("b", 3), raise(), hide("a"), save()]);

    let effects = activate_window(&mut m, 2);
    assert_eq!(
        effects,
        vec![present("a", 2), raise(), minimize(1), hide("b"), save()]
    );
}

#[test]
fn activate_already_active_window_presents_it_again() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let effects = activate_window(&mut m, 1);
    assert_eq!(effects, vec![present("a", 1), raise(), save()]);
}

#[test]
fn activate_hides_each_other_app_once_even_with_several_windows() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "b", vec![win(3, "b3"), win(4, "b4")]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let effects = activate_window(&mut m, 1);
    assert_eq!(effects, vec![present("a", 1), raise(), hide("b"), save()]);
}

#[test]
fn activate_placeholder_of_app_not_running_launches_it() {
    let mut m = model_with(&["a", "b"]);
    let effects = activate_app(&mut m, "b");
    assert_eq!(effects, vec![launch("b"), save()]);
    assert_eq!(m.active, Some(app_tab(&m, "b")));
}

#[test]
fn activate_placeholder_of_running_app_reopens_it() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "b", vec![]);
    let effects = activate_app(&mut m, "b");
    assert_eq!(effects, vec![reopen("b"), save()]);
}

#[test]
fn activate_unknown_id_changes_nothing() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let before = snapshot(&m);
    assert_eq!(act(&mut m, Action::Activate(TabId(999))), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn activate_while_untrusted_only_saves() {
    let mut m = Model::new(Saved {
        apps: vec![app("a"), app("b")],
        frame: Some(screen()),
        active_app: None,
    });
    let effects = activate_app(&mut m, "b");
    assert_eq!(effects, vec![save()]);
    assert_eq!(m.active, Some(app_tab(&m, "b")));
}

#[test]
fn activate_without_frame_only_saves() {
    let mut m = Model::new(Saved { apps: vec![app("a"), app("b")], frame: None, active_app: None });
    ev(&mut m, Event::TrustChanged(true));
    let effects = activate_app(&mut m, "b");
    assert_eq!(effects, vec![save()]);
}

#[test]
fn close_window_tab_asks_the_window_to_close_and_keeps_the_tab() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let effects = close_window_tab(&mut m, 1);
    assert_eq!(effects, vec![close_window(1)]);
    assert_eq!(bundles(&m), vec!["a"]);
    assert_eq!(windows(&m), vec![Some(1)]);
}

#[test]
fn close_running_placeholder_quits_unwatches_and_saves() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "b", vec![]);
    let effects = close_app(&mut m, "b");
    assert_eq!(effects, vec![quit_app("b"), unwatch("b"), save()]);
    assert_eq!(bundles(&m), vec!["a"]);
    assert!(!m.is_running("b"));
}

#[test]
fn close_placeholder_of_app_not_running_only_unwatches() {
    let mut m = model_with(&["a", "b"]);
    let effects = close_app(&mut m, "b");
    assert_eq!(effects, vec![unwatch("b"), save()]);
    assert_eq!(bundles(&m), vec!["a"]);
}

#[test]
fn close_active_placeholder_presents_the_next_tab() {
    let mut m = model_with(&["a", "b", "c"]);
    let effects = close_app(&mut m, "a");
    assert_eq!(effects, vec![unwatch("a"), launch("b"), save()]);
    assert_eq!(bundles(&m), vec!["b", "c"]);
    assert_eq!(m.active, Some(app_tab(&m, "b")));
}

#[test]
fn close_last_tab_leaves_no_active_tab() {
    let mut m = model_with(&["a"]);
    let effects = close_app(&mut m, "a");
    assert_eq!(effects, vec![unwatch("a"), save()]);
    assert!(m.tabs.is_empty());
    assert_eq!(m.active, None);
    assert_eq!(m.saved().active_app, None);
}

#[test]
fn close_unknown_id_changes_nothing() {
    let mut m = model_with(&["a"]);
    let before = snapshot(&m);
    assert_eq!(act(&mut m, Action::Close(TabId(999))), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn move_to_a_valid_index_reorders_and_saves() {
    let mut m = model_with(&["a", "b", "c"]);
    assert_eq!(act(&mut m, Action::Move { from: 0, to: 2 }), vec![save()]);
    assert_eq!(bundles(&m), vec!["b", "c", "a"]);
    assert_eq!(act(&mut m, Action::Move { from: 2, to: 0 }), vec![save()]);
    assert_eq!(bundles(&m), vec!["a", "b", "c"]);
}

#[test]
fn move_out_of_range_or_to_the_same_index_changes_nothing() {
    let mut m = model_with(&["a", "b", "c"]);
    let before = snapshot(&m);
    assert_eq!(act(&mut m, Action::Move { from: 0, to: 3 }), none());
    assert_eq!(act(&mut m, Action::Move { from: 5, to: 0 }), none());
    assert_eq!(act(&mut m, Action::Move { from: 1, to: 1 }), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn move_that_splits_an_app_is_refused() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    // Tabs: a1, a2, b. Moving a1 behind b would split app a.
    let before = snapshot(&m);
    assert_eq!(act(&mut m, Action::Move { from: 0, to: 2 }), none());
    assert_eq!(snapshot(&m), before);
    // Moving b in front of a is fine.
    assert_eq!(act(&mut m, Action::Move { from: 2, to: 0 }), vec![save()]);
    assert_eq!(bundles(&m), vec!["b", "a", "a"]);
}

#[test]
fn bar_moved_moves_the_frame_and_the_window_and_saves() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let bar = Rect::new(100.0, 50.0, 1000.0, 40.0);
    let effects = act(&mut m, Action::BarMoved(bar));
    let new_frame = Rect::new(100.0, 50.0, 1000.0, 875.0);
    assert_eq!(m.frame, Some(new_frame));
    assert_eq!(effects, vec![set_frame(1, new_frame.content()), save()]);
}

#[test]
fn bar_moved_close_to_the_current_bar_changes_nothing() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let before = snapshot(&m);
    let nudge = Rect::new(0.4, 25.2, 1440.0, 40.0);
    assert_eq!(act(&mut m, Action::BarMoved(nudge)), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn bar_moved_without_frame_changes_nothing() {
    let mut m = Model::new(Saved::default());
    let effects = act(&mut m, Action::BarMoved(Rect::new(100.0, 50.0, 1000.0, 40.0)));
    assert_eq!(effects, none());
    assert_eq!(m.frame, None);
}

#[test]
fn bar_moved_with_placeholder_active_only_saves() {
    let mut m = model_with(&["a"]);
    let effects = act(&mut m, Action::BarMoved(Rect::new(100.0, 50.0, 1000.0, 40.0)));
    assert_eq!(effects, vec![save()]);
    assert_eq!(m.frame, Some(Rect::new(100.0, 50.0, 1000.0, 875.0)));
}

#[test]
fn zoom_sets_the_whole_frame_moves_bar_and_window_and_saves() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let zoomed = Rect::new(100.0, 200.0, 800.0, 500.0);
    let effects = act(&mut m, Action::Zoom(zoomed));
    assert_eq!(effects, vec![move_bar(zoomed.bar()), set_frame(1, zoomed.content()), save()]);
    assert_eq!(m.frame, Some(zoomed));
    // Zooming to the same frame again changes nothing.
    assert_eq!(act(&mut m, Action::Zoom(zoomed)), none());
}

#[test]
fn zoom_with_a_placeholder_moves_only_the_bar() {
    let mut m = model_with(&["a"]);
    let zoomed = Rect::new(0.0, 25.0, 700.0, 600.0);
    assert_eq!(act(&mut m, Action::Zoom(zoomed)), vec![move_bar(zoomed.bar()), save()]);
}

#[test]
fn zoom_without_frame_changes_nothing() {
    let mut m = Model::new(Saved::default());
    assert_eq!(act(&mut m, Action::Zoom(screen())), none());
    assert_eq!(m.frame, None);
}

#[test]
fn zoom_of_a_refusing_window_asks_for_the_full_content_again() {
    let mut m = model_with_refusing_calc();
    ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    let zoomed = Rect::new(0.0, 25.0, 1000.0, 700.0);
    let effects = act(&mut m, Action::Zoom(zoomed));
    assert_eq!(effects, vec![move_bar(zoomed.bar()), set_frame(2, zoomed.content()), save()]);
    // Its refusal (with the fit the platform put it at) needs no correction.
    let fit = zoomed.content().fit(230.0, 408.0);
    assert_eq!(ev(&mut m, reported(2, fit, false)), none());
}

#[test]
fn present_shows_the_new_window_before_it_minimizes_or_hides_anything() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    for window in [3, 2, 1, 3] {
        let effects = activate_window(&mut m, window);
        let present_at = effects.iter().position(|e| matches!(e, Effect::Platform(Command::Present { .. })));
        let first_off = effects.iter().position(|e| {
            matches!(e, Effect::Platform(Command::HideApp { .. } | Command::Minimize { .. }))
        });
        assert_eq!(present_at, Some(0), "{effects:?}");
        assert!(first_off.is_none_or(|i| i > 1), "{effects:?}");
        assert_eq!(effects[1], raise(), "{effects:?}");
    }
}

#[test]
fn rect_fit_centers_a_smaller_window_and_left_aligns_a_wider_one() {
    let content = Rect::new(100.0, 140.0, 900.0, 560.0);
    assert_eq!(content.fit(230.0, 408.0), Rect::new(435.0, 140.0, 230.0, 408.0));
    assert_eq!(content.fit(1200.0, 300.0), Rect::new(100.0, 140.0, 1200.0, 300.0));
}

#[test]
fn quit_shows_each_running_app_once_in_tab_order() {
    let mut m = model_with(&["a", "b", "c"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    running(&mut m, "c", vec![win(5, "c5")]);
    // Tabs: a1, a2, b (not running), c5.
    let effects = act(&mut m, Action::Quit);
    assert_eq!(effects, vec![show("a"), show("c"), save()]);
}

// ---------------------------------------------------------------------------
// AppRunning
// ---------------------------------------------------------------------------

#[test]
fn app_running_turns_placeholder_into_first_window_and_keeps_its_id() {
    let mut m = model_with(&["a", "b"]);
    let placeholder = app_tab(&m, "a");
    let effects = running(&mut m, "a", vec![win(1, "one"), win(2, "two")]);
    assert_eq!(window_tab(&m, 1), placeholder);
    assert_ne!(window_tab(&m, 2), placeholder);
    assert_eq!(bundles(&m), vec!["a", "a", "b"]);
    assert!(m.is_running("a"));
    assert_eq!(effects, vec![present("a", 1), raise(), minimize(2)]);
}

#[test]
fn app_running_puts_extra_windows_after_the_last_tab_in_the_given_order() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    let effects = running(&mut m, "a", vec![win(1, "a1"), win(4, "a4"), win(2, "a2")]);
    assert_eq!(bundles(&m), vec!["a", "a", "a", "b"]);
    assert_eq!(windows(&m), vec![Some(1), Some(4), Some(2), Some(3)]);
    assert_eq!(effects, vec![present("a", 1), raise(), minimize(4), minimize(2), hide("b")]);
}

#[test]
fn app_running_keeps_tab_ids_and_updates_titles() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "old")]);
    let id = window_tab(&m, 1);
    let effects = running(&mut m, "a", vec![win(1, "new title"), win(2, "second")]);
    assert_eq!(window_tab(&m, 1), id);
    assert_eq!(m.tab(id).expect("tab").label(), "new title");
    assert_eq!(windows(&m), vec![Some(1), Some(2)]);
    assert_eq!(effects, vec![present("a", 1), raise(), minimize(2)]);
}

#[test]
fn app_running_removes_missing_windows_and_moves_active_to_first_tab() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "w1"), win(2, "w2"), win(3, "w3")]);
    let kept = window_tab(&m, 2);
    assert_eq!(m.active, Some(window_tab(&m, 1)));
    let effects = running(&mut m, "a", vec![win(2, "w2")]);
    assert_eq!(windows(&m), vec![Some(2)]);
    assert_eq!(m.tabs[0].id, kept);
    assert_eq!(m.active, Some(kept));
    assert_eq!(effects, vec![present("a", 2), raise()]);
}

#[test]
fn app_running_with_no_windows_leaves_one_placeholder_with_the_first_id() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "w1"), win(2, "w2")]);
    let first = window_tab(&m, 1);
    let effects = running(&mut m, "a", vec![]);
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(windows(&m), vec![None, None]);
    assert_eq!(m.tabs[0].id, first);
    assert_eq!(m.active, Some(first));
    assert!(m.is_running("a"));
    // The windows vanished: an expiring placeholder, and no Reopen (it would bring the
    // window back).
    assert!(m.is_expiring(first));
    assert_eq!(effects, vec![expire(first), save()]);
}

#[test]
fn app_running_for_a_background_app_hides_it() {
    let mut m = model_with(&["a", "b"]);
    let effects = running(&mut m, "b", vec![win(3, "b3")]);
    assert_eq!(effects, vec![hide("b")]);
    assert_eq!(windows(&m), vec![None, Some(3)]);
    assert_eq!(m.active, Some(app_tab(&m, "a")));
}

#[test]
fn app_running_for_an_unknown_app_is_ignored() {
    let mut m = model_with(&["a"]);
    let before = snapshot(&m);
    assert_eq!(running(&mut m, "zzz", vec![win(1, "x")]), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn app_running_skips_windows_of_other_apps_and_duplicates() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "b", vec![win(3, "b3")]);
    // Window 3 belongs to b. Window 1 is listed twice.
    let effects = running(&mut m, "a", vec![win(3, "x"), win(1, "y"), win(1, "z")]);
    assert_eq!(windows(&m), vec![Some(1), Some(3)]);
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(m.tab(app_tab(&m, "a")).expect("tab").label(), "y");
    assert_eq!(effects, vec![present("a", 1), raise(), hide("b")]);
}

// ---------------------------------------------------------------------------
// AppStopped
// ---------------------------------------------------------------------------

#[test]
fn app_stopped_collapses_to_one_placeholder_at_the_first_position() {
    let mut m = model_with(&["b", "a", "c"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    let first = window_tab(&m, 1);
    assert_eq!(bundles(&m), vec!["b", "a", "a", "c"]);
    let effects = ev(&mut m, Event::AppStopped { bundle_id: "a".into() });
    assert_eq!(effects, vec![expire(first), save()]);
    assert_eq!(bundles(&m), vec!["b", "a", "c"]);
    assert_eq!(windows(&m), vec![None, None, None]);
    assert_eq!(m.tabs[1].id, first);
    assert!(!m.is_running("a"));
    assert_eq!(m.active, Some(app_tab(&m, "b")));
}

#[test]
fn app_stopped_makes_the_placeholder_active_when_one_of_its_tabs_was() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    let first = window_tab(&m, 1);
    assert_eq!(m.active, Some(first));
    let effects = ev(&mut m, Event::AppStopped { bundle_id: "a".into() });
    assert_eq!(effects, vec![expire(first), save()]);
    assert_eq!(m.active, Some(first));
    assert_eq!(m.tab(first).expect("tab").window, None);
}

#[test]
fn app_stopped_keeps_an_active_tab_of_another_app() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    let active = m.active;
    let b = window_tab(&m, 3);
    let effects = ev(&mut m, Event::AppStopped { bundle_id: "b".into() });
    assert_eq!(effects, vec![expire(b), save()]);
    assert_eq!(m.active, active);
    assert_eq!(windows(&m), vec![Some(1), None]);
}

#[test]
fn app_stopped_for_unknown_app_changes_nothing() {
    let mut m = model_with(&["a"]);
    let before = snapshot(&m);
    assert_eq!(ev(&mut m, Event::AppStopped { bundle_id: "zzz".into() }), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn app_stopped_forgets_windows_closed_by_oriel() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    let id = app_tab(&m, "a");
    assert_eq!(ev(&mut m, Event::AppStopped { bundle_id: "a".into() }), vec![expire(id), save()]);
    running(&mut m, "a", vec![win(1, "a1")]);
    // The window is closed by the user now. It must not quit the app.
    assert_eq!(closed(&mut m, "a", 1), vec![expire(id), save()]);
    assert_eq!(windows(&m), vec![None]);
}

// ---------------------------------------------------------------------------
// WindowOpened
// ---------------------------------------------------------------------------

#[test]
fn window_opened_fills_the_placeholder_and_presents_it() {
    let mut m = model_with(&["a", "b"]);
    let placeholder = app_tab(&m, "a");
    let effects = opened(&mut m, "a", win(1, "a1"));
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(window_tab(&m, 1), placeholder);
    assert_eq!(m.active, Some(placeholder));
    assert!(m.is_running("a"));
    assert_eq!(effects, vec![present("a", 1), raise()]);
}

#[test]
fn window_opened_for_a_background_app_only_records_the_window() {
    let mut m = model_with(&["a", "b"]);
    let active = m.active;
    let effects = opened(&mut m, "b", win(3, "b3"));
    assert_eq!(effects, none());
    assert_eq!(windows(&m), vec![None, Some(3)]);
    assert_eq!(m.active, active);
    assert!(m.is_running("b"));
}

#[test]
fn window_opened_appends_after_the_last_tab_and_activates_when_the_app_is_active() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    let effects = opened(&mut m, "a", win(2, "a2"));
    assert_eq!(bundles(&m), vec!["a", "a", "b"]);
    assert_eq!(windows(&m), vec![Some(1), Some(2), Some(3)]);
    assert_eq!(m.active, Some(window_tab(&m, 2)));
    assert_eq!(effects, vec![present("a", 2), raise(), minimize(1), hide("b")]);
}

#[test]
fn window_opened_duplicate_or_unknown_app_changes_nothing() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let before = snapshot(&m);
    assert_eq!(opened(&mut m, "a", win(1, "again")), none());
    assert_eq!(opened(&mut m, "zzz", win(9, "x")), none());
    assert_eq!(snapshot(&m), before);
}

// ---------------------------------------------------------------------------
// WindowClosed
// ---------------------------------------------------------------------------

#[test]
fn window_closed_by_oriel_as_last_window_removes_the_tab_and_quits_the_app() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    let effects = closed(&mut m, "a", 1);
    // The active tab was removed, so the tab at the same index (b) becomes active.
    assert_eq!(effects, vec![quit_app("a"), unwatch("a"), launch("b"), save()]);
    assert_eq!(bundles(&m), vec!["b"]);
    assert_eq!(m.active, Some(app_tab(&m, "b")));
    assert!(!m.is_running("a"));
}

#[test]
fn window_closed_by_oriel_while_another_app_is_active_does_not_present() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    activate_app(&mut m, "b");
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    let effects = closed(&mut m, "a", 1);
    assert_eq!(effects, vec![quit_app("a"), unwatch("a"), save()]);
    assert_eq!(bundles(&m), vec!["b"]);
    assert_eq!(m.active, Some(app_tab(&m, "b")));
}

#[test]
fn window_closed_by_the_user_as_last_window_becomes_a_placeholder() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let id = window_tab(&m, 1);
    let effects = closed(&mut m, "a", 1);
    assert_eq!(effects, vec![expire(id), save()]);
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(m.tab(id).expect("tab").window, None);
    assert_eq!(m.active, Some(id));
    assert!(m.is_running("a"));
    assert!(m.is_expiring(id));
}

#[test]
fn window_closed_when_other_windows_remain_presents_the_next_tab_if_active() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    assert_eq!(m.active, Some(window_tab(&m, 1)));
    let effects = closed(&mut m, "a", 1);
    assert_eq!(windows(&m), vec![Some(2)]);
    assert_eq!(m.active, Some(window_tab(&m, 2)));
    assert_eq!(effects, vec![present("a", 2), raise(), save()]);
}

#[test]
fn window_closed_when_other_windows_remain_and_tab_inactive_only_saves() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(2, "b2"), win(3, "b3")]);
    let active = m.active;
    assert_eq!(closed(&mut m, "b", 2), vec![save()]);
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(windows(&m), vec![Some(1), Some(3)]);
    assert_eq!(m.active, active);
}

#[test]
fn window_closed_for_wrong_app_or_unknown_window_changes_nothing() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let before = snapshot(&m);
    assert_eq!(closed(&mut m, "b", 1), none());
    assert_eq!(closed(&mut m, "a", 9), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn window_closed_by_oriel_with_other_windows_left_removes_only_the_tab() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    assert_eq!(close_window_tab(&mut m, 2), vec![close_window(2)]);
    assert_eq!(closed(&mut m, "a", 2), vec![save()]);
    assert_eq!(windows(&m), vec![Some(1)]);
    assert!(m.is_running("a"));
}

// ---------------------------------------------------------------------------
// WindowTitle, WindowFocused, WindowFrame
// ---------------------------------------------------------------------------

#[test]
fn window_title_updates_the_label() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "old")]);
    assert_eq!(ev(&mut m, Event::WindowTitle { window: WindowId(1), title: "New".into() }), none());
    assert_eq!(m.tabs[0].label(), "New");
    assert_eq!(ev(&mut m, Event::WindowTitle { window: WindowId(9), title: "x".into() }), none());
}

#[test]
fn window_focused_on_the_active_tab_only_raises_the_bar() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let effects = ev(&mut m, Event::WindowFocused { bundle_id: "a".into(), window: WindowId(1) });
    assert_eq!(effects, vec![raise()]);
}

#[test]
fn window_focused_on_another_tab_switches_and_presents_it() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    let effects = ev(&mut m, Event::WindowFocused { bundle_id: "b".into(), window: WindowId(3) });
    assert_eq!(m.active, Some(window_tab(&m, 3)));
    assert_eq!(effects, vec![present("b", 3), raise(), hide("a"), Effect::Save]);
}

#[test]
fn window_focused_unknown_window_or_wrong_app_changes_nothing() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    let before = snapshot(&m);
    assert_eq!(ev(&mut m, Event::WindowFocused { bundle_id: "a".into(), window: WindowId(9) }), none());
    assert_eq!(ev(&mut m, Event::WindowFocused { bundle_id: "a".into(), window: WindowId(3) }), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn window_frame_of_the_active_window_moves_the_bar_and_saves() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let effects = ev(
        &mut m,
        Event::WindowFrame { window: WindowId(1), frame: Rect::new(10.0, 100.0, 800.0, 600.0), by_user: true },
    );
    let new_frame = Rect::new(10.0, 60.0, 800.0, 640.0);
    assert_eq!(m.frame, Some(new_frame));
    assert_eq!(effects, vec![move_bar(new_frame.bar()), save()]);
}

#[test]
fn window_frame_close_to_the_content_changes_nothing() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let before = snapshot(&m);
    let nudge = Rect::new(0.2, 65.3, 1440.0, 835.0);
    assert_eq!(ev(&mut m, Event::WindowFrame { window: WindowId(1), frame: nudge, by_user: true }), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn window_frame_of_a_window_that_is_not_active_changes_nothing() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let before = snapshot(&m);
    let far = Rect::new(10.0, 100.0, 800.0, 600.0);
    assert_eq!(ev(&mut m, Event::WindowFrame { window: WindowId(9), frame: far, by_user: true }), none());
    assert_eq!(snapshot(&m), before);

    // A placeholder is active: no window to move.
    activate_app(&mut m, "b");
    let before = snapshot(&m);
    assert_eq!(ev(&mut m, Event::WindowFrame { window: WindowId(1), frame: far, by_user: true }), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn window_frame_without_frame_changes_nothing() {
    let mut m = Model::new(Saved { apps: vec![app("a")], frame: None, active_app: None });
    let effects = ev(
        &mut m,
        Event::WindowFrame { window: WindowId(1), frame: Rect::new(10.0, 100.0, 800.0, 600.0), by_user: true },
    );
    assert_eq!(effects, none());
    assert_eq!(m.frame, None);
}

// ---------------------------------------------------------------------------
// A window that refuses the frame
// ---------------------------------------------------------------------------

fn reported(window: u32, frame: Rect, by_user: bool) -> Event {
    Event::WindowFrame { window: WindowId(window), frame, by_user }
}

/// The place of a `w x h` window that cannot take `content()`.
fn fit_of(w: f64, h: f64) -> Rect {
    Rect::new(content().x + (content().w - w) / 2.0, content().y, w, h)
}

/// Apps a (normal window 1) and calc (fixed 230x408, window 2). Calc is active and refused.
fn model_with_refusing_calc() -> Model {
    let mut m = model_with(&["a", "calc"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "calc", vec![win(2, "Calculator")]);
    activate_window(&mut m, 2);
    m
}

#[test]
fn a_refusal_keeps_the_frame_and_moves_the_window_to_its_fit() {
    let mut m = model_with_refusing_calc();
    let actual = Rect::new(0.0, 65.0, 230.0, 408.0);
    let effects = ev(&mut m, reported(2, actual, false));
    assert_eq!(m.frame, Some(screen()));
    assert_eq!(effects, vec![set_frame(2, fit_of(230.0, 408.0))]);
    assert_eq!(fit_of(230.0, 408.0).x, 605.0);
}

#[test]
fn a_second_refusal_with_the_same_size_does_nothing() {
    let mut m = model_with_refusing_calc();
    ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    // The OS clamped the position: the read-back is not the fit. No second correction.
    let clamped = Rect::new(600.0, 70.0, 230.0, 408.0);
    assert_eq!(ev(&mut m, reported(2, clamped, false)), none());
    assert_eq!(ev(&mut m, reported(2, clamped, false)), none());
    assert_eq!(m.frame, Some(screen()));
}

#[test]
fn a_refusal_already_at_its_fit_does_nothing() {
    let mut m = model_with_refusing_calc();
    assert_eq!(ev(&mut m, reported(2, fit_of(230.0, 408.0), false)), none());
    assert_eq!(m.frame, Some(screen()));
}

#[test]
fn switching_from_a_refusing_window_presents_the_normal_one_at_full_content() {
    let mut m = model_with_refusing_calc();
    ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    let effects = activate_window(&mut m, 1);
    assert_eq!(effects, vec![present("a", 1), raise(), hide("calc"), save()]);
    assert_eq!(m.frame, Some(screen()));
    // And back: the refusal is reported again after the new `present`, and is corrected again.
    activate_window(&mut m, 2);
    let effects = ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    assert_eq!(effects, vec![set_frame(2, fit_of(230.0, 408.0))]);
}

#[test]
fn a_user_drag_of_a_refusing_window_moves_the_whole_frame() {
    let mut m = model_with_refusing_calc();
    ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    let fit = fit_of(230.0, 408.0);
    let dragged = Rect::new(fit.x + 100.0, fit.y + 50.0, 230.0, 408.0);
    let effects = ev(&mut m, reported(2, dragged, true));
    let moved = Rect::new(100.0, 75.0, 1440.0, 875.0);
    assert_eq!(m.frame, Some(moved));
    assert_eq!(effects, vec![move_bar(moved.bar()), save()]);
    // The next drag step is relative to the new place.
    let again = Rect::new(dragged.x - 10.0, dragged.y, 230.0, 408.0);
    ev(&mut m, reported(2, again, true));
    assert_eq!(m.frame, Some(Rect::new(90.0, 75.0, 1440.0, 875.0)));
}

#[test]
fn a_user_drag_of_a_normal_window_moves_the_frame_by_the_same_delta() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let moved_content = Rect::new(30.0, 90.0, 1440.0, 835.0);
    let effects = ev(&mut m, reported(1, moved_content, true));
    let frame = Rect::new(30.0, 50.0, 1440.0, 875.0);
    assert_eq!(m.frame, Some(frame));
    assert_eq!(effects, vec![move_bar(frame.bar()), save()]);
}

#[test]
fn a_user_resize_of_a_refusing_window_adopts_the_window_as_the_frame() {
    let mut m = model_with_refusing_calc();
    ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    let resized = Rect::new(605.0, 65.0, 400.0, 500.0);
    let effects = ev(&mut m, reported(2, resized, true));
    let frame = Rect::from_content(&resized);
    assert_eq!(m.frame, Some(frame));
    assert_eq!(effects, vec![move_bar(frame.bar()), save()]);
}

#[test]
fn a_user_report_at_the_shown_place_changes_nothing() {
    let mut m = model_with_refusing_calc();
    ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    let before = snapshot(&m);
    assert_eq!(ev(&mut m, reported(2, fit_of(230.0, 408.0), true)), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn bar_moved_with_a_refusing_window_sends_its_fit_in_the_new_content() {
    let mut m = model_with_refusing_calc();
    ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    let bar = Rect::new(50.0, 125.0, 1440.0, 40.0);
    let effects = act(&mut m, Action::BarMoved(bar));
    let frame = Rect::new(50.0, 125.0, 1440.0, 875.0);
    assert_eq!(m.frame, Some(frame));
    let fit = Rect::new(50.0 + 605.0, 165.0, 230.0, 408.0);
    assert_eq!(effects, vec![set_frame(2, fit), save()]);
    // The window reports no refusal (it is at the requested place): nothing happens.
    assert_eq!(ev(&mut m, reported(2, fit, false)), none());
    // Another move follows the same rule.
    let effects = act(&mut m, Action::BarMoved(Rect::new(60.0, 125.0, 1440.0, 40.0)));
    assert_eq!(effects[0], set_frame(2, Rect::new(60.0 + 605.0, 165.0, 230.0, 408.0)));
}

#[test]
fn bar_moved_resized_asks_for_the_full_content_again() {
    let mut m = model_with_refusing_calc();
    ev(&mut m, reported(2, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    let bar = Rect::new(0.0, 25.0, 1000.0, 40.0);
    let effects = act(&mut m, Action::BarMoved(bar));
    let frame = Rect::new(0.0, 25.0, 1000.0, 875.0);
    assert_eq!(effects, vec![set_frame(2, frame.content()), save()]);
}

#[test]
fn bar_moved_with_a_normal_window_sends_the_new_content() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let bar = Rect::new(20.0, 45.0, 1440.0, 40.0);
    let effects = act(&mut m, Action::BarMoved(bar));
    let frame = Rect::new(20.0, 45.0, 1440.0, 875.0);
    assert_eq!(effects, vec![set_frame(1, frame.content()), save()]);
}

#[test]
fn a_window_wider_than_the_content_is_left_aligned() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let wide = Rect::new(0.0, 65.0, 1600.0, 300.0);
    // Already at its fit (left-aligned under the bar): nothing to do.
    assert_eq!(ev(&mut m, reported(1, wide, false)), none());
    assert_eq!(m.frame, Some(screen()));
    // Same size, other place (wider than content, so the fit is at content.x): corrected once.
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let wide = Rect::new(-80.0, 100.0, 1600.0, 300.0);
    let effects = ev(&mut m, reported(1, wide, false));
    assert_eq!(effects, vec![set_frame(1, Rect::new(0.0, 65.0, 1600.0, 300.0))]);
}

#[test]
fn a_refusal_of_a_window_that_is_not_active_changes_nothing() {
    let mut m = model_with_refusing_calc();
    let before = snapshot(&m);
    assert_eq!(ev(&mut m, reported(1, Rect::new(0.0, 65.0, 230.0, 408.0), false)), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn a_refusal_forgets_the_closing_mark_like_any_frame_report() {
    let mut m = model_with_cancelled_close();
    ev(&mut m, reported(1, Rect::new(0.0, 65.0, 230.0, 408.0), false));
    assert_user_close_keeps_the_app(&mut m);
}

// ---------------------------------------------------------------------------
// Randomized sequences
// ---------------------------------------------------------------------------

/// A small deterministic generator (xorshift64). No crates needed.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number in `0..n`. `n` must be above zero.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

const APPS: [&str; 4] = ["com.a", "com.b", "com.c", "com.d"];

fn random_tab(rng: &mut Rng, m: &Model) -> TabId {
    if !m.tabs.is_empty() && rng.below(10) < 8 {
        m.tabs[rng.below(m.tabs.len())].id
    } else {
        TabId(rng.below(40) as u64)
    }
}

fn random_window(rng: &mut Rng, m: &Model) -> WindowId {
    let shown: Vec<WindowId> = m.tabs.iter().filter_map(|t| t.window.as_ref().map(|w| w.id)).collect();
    if !shown.is_empty() && rng.below(10) < 7 {
        shown[rng.below(shown.len())]
    } else {
        WindowId(rng.below(12) as u32 + 1)
    }
}

fn random_windows(rng: &mut Rng) -> Vec<WindowInfo> {
    let count = rng.below(4);
    (0..count)
        .map(|_| WindowInfo { id: WindowId(rng.below(12) as u32 + 1), title: "t".into() })
        .collect()
}

fn random_rect(rng: &mut Rng, m: &Model) -> Rect {
    // Often the exact current frame or content, so the "no change" paths run too.
    match rng.below(4) {
        0 => m.frame.unwrap_or(screen()),
        1 => m.frame.map(|f| f.content()).unwrap_or(content()),
        _ => {
            let c = [0.0, 0.5, 25.0, 40.0, 65.0, 100.0, 835.0, 875.0, 1440.0];
            Rect::new(
                c[rng.below(c.len())],
                c[rng.below(c.len())],
                c[rng.below(c.len())],
                c[rng.below(c.len())],
            )
        }
    }
}

fn random_input(rng: &mut Rng, m: &Model) -> Input {
    let bundle_id = APPS[rng.below(APPS.len())];
    let window = random_window(rng, m);
    let tab = random_tab(rng, m);
    let title = format!("title {}", rng.below(5));
    match rng.below(27) {
        0 => Input::Event(Event::TrustChanged(rng.below(3) != 0)),
        1 => Input::Event(Event::ScreenChanged { visible: random_rect(rng, m) }),
        2 => Input::Event(Event::AppsScanned(vec![app(bundle_id)])),
        3 | 4 => Input::Event(Event::AppRunning {
            bundle_id: bundle_id.into(),
            windows: random_windows(rng),
        }),
        5 => Input::Event(Event::AppStopped { bundle_id: bundle_id.into() }),
        6 => Input::Event(Event::WindowOpened {
            bundle_id: bundle_id.into(),
            window: WindowInfo { id: window, title },
        }),
        7 => Input::Event(Event::WindowClosed { bundle_id: bundle_id.into(), window }),
        8 => Input::Event(Event::WindowTitle { window, title }),
        9 => Input::Event(Event::WindowFocused { bundle_id: bundle_id.into(), window }),
        10 => Input::Event(Event::WindowFrame {
            window,
            frame: random_rect(rng, m),
            by_user: rng.below(2) == 0,
        }),
        11 => Input::Action(Action::AddApp(app(bundle_id))),
        12 | 13 => Input::Action(Action::Activate(tab)),
        14 => Input::Action(Action::Close(tab)),
        15 => Input::Action(Action::Move { from: rng.below(5), to: rng.below(5) }),
        16 => Input::Action(Action::BarMoved(random_rect(rng, m))),
        17 => Input::Action(Action::RequestTrust),
        18 => Input::Action(Action::OpenPicker),
        19 => Input::Action(Action::Zoom(random_rect(rng, m))),
        20 | 21 => Input::Action(Action::ExpireTab(tab)),
        23 | 24 => Input::Action(Action::Split(tab)),
        25 => match rng.below(3) {
            0 => Input::Action(Action::Unsplit),
            _ => Input::Action(Action::SplitRatio(rng.below(120) as f64 / 100.0 - 0.1)),
        },
        _ => Input::Action(Action::Quit),
    }
}

/// Every window or app that a `Present`, `Launch` or `Reopen` names must be the active tab.
fn assert_presents_the_active_tab(m: &Model, effects: &[Effect]) {
    for effect in effects {
        match effect {
            Effect::Platform(Command::Present { bundle_id, window, .. }) => {
                let active = m.active_tab().expect("present needs an active tab");
                let partner = m
                    .shown_split()
                    .and_then(|s| s.partner(active.id))
                    .and_then(|id| m.tab(id));
                let named = |tab: &Tab| {
                    &tab.app.bundle_id == bundle_id && tab.window.as_ref().map(|w| w.id) == Some(*window)
                };
                // A present names the active tab, or the other pane of a shown split.
                assert!(named(active) || partner.is_some_and(named), "present of {bundle_id} {window:?}");
            }
            Effect::Platform(Command::Launch { app }) => {
                let active = m.active_tab().expect("launch needs an active tab");
                assert_eq!(&active.app, app);
                assert!(active.window.is_none());
            }
            Effect::Platform(Command::Reopen { bundle_id }) => {
                let active = m.active_tab().expect("reopen needs an active tab");
                assert_eq!(&active.app.bundle_id, bundle_id);
                assert!(active.window.is_none());
            }
            _ => {}
        }
    }
}

#[test]
fn random_sequences_keep_the_invariants_after_every_step() {
    let mut shown_steps = 0;
    for seed in [1_u64, 7, 42, 2026, 99_991] {
        let mut rng = Rng(seed);
        let mut m = Model::new(Saved {
            apps: vec![app(APPS[0]), app(APPS[1])],
            frame: None,
            active_app: None,
        });
        // Every tab id ever seen. A new tab must never reuse one of them.
        let mut observed: BTreeSet<TabId> = m.tabs.iter().map(|t| t.id).collect();
        for step in 0..3000 {
            let before: BTreeSet<TabId> = m.tabs.iter().map(|t| t.id).collect();
            let input = random_input(&mut rng, &m);
            let description = format!("seed {seed}, step {step}: {input:?}");
            let frame_before = m.frame;
            let refusal = matches!(&input, Input::Event(Event::WindowFrame { by_user: false, .. }));
            let effects = m.handle(input);

            assert_invariants(&m);
            shown_steps += usize::from(m.shown_split().is_some());
            if refusal {
                // A refusal never moves or resizes the Oriel frame and never saves.
                assert_eq!(m.frame, frame_before, "after {description}");
                assert!(
                    effects.iter().all(|e| matches!(e, Effect::Platform(Command::SetFrame { .. }))),
                    "after {description}: {effects:?}"
                );
            }
            assert_presents_the_active_tab(&m, &effects);
            for tab in &m.tabs {
                if !before.contains(&tab.id) {
                    assert!(!observed.contains(&tab.id), "reused tab id after {description}");
                }
            }
            observed.extend(m.tabs.iter().map(|t| t.id));
            // The saved state must load back to the same saved state.
            let saved = m.saved();
            assert_eq!(Model::new(saved.clone()).saved(), saved, "after {description}");
        }
    }
    assert!(shown_steps > 100, "the random run must exercise a shown split ({shown_steps} steps)");
}

// ---------------------------------------------------------------------------
// A placeholder also hides the other running apps
// ---------------------------------------------------------------------------

/// Apps a and c run with one window each. App b is a placeholder. Tabs: a1 (active), b, c5.
fn model_with_two_running_apps() -> Model {
    let mut m = model_with(&["a", "b", "c"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "c", vec![win(5, "c5")]);
    m
}

#[test]
fn activate_placeholder_hides_the_other_running_apps_then_launches() {
    let mut m = model_with_two_running_apps();
    let effects = activate_app(&mut m, "b");
    assert_eq!(effects, vec![hide("a"), hide("c"), launch("b"), save()]);
}

#[test]
fn activate_placeholder_of_running_app_hides_the_others_then_reopens() {
    let mut m = model_with_two_running_apps();
    running(&mut m, "b", vec![]);
    let effects = activate_app(&mut m, "b");
    assert_eq!(effects, vec![hide("a"), hide("c"), reopen("b"), save()]);
}

#[test]
fn activate_placeholder_with_no_other_running_app_only_launches() {
    let mut m = model_with(&["a", "b"]);
    assert_eq!(activate_app(&mut m, "b"), vec![launch("b"), save()]);
    // An app that runs without a window is hidden too, once, and a stopped app is not.
    let mut m = model_with(&["a", "b", "c"]);
    running(&mut m, "a", vec![]);
    assert_eq!(activate_app(&mut m, "c"), vec![hide("a"), launch("c"), save()]);
}

#[test]
fn activate_placeholder_hides_an_app_with_several_windows_once() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    assert_eq!(activate_app(&mut m, "b"), vec![hide("a"), launch("b"), save()]);
}

#[test]
fn activate_placeholder_while_untrusted_or_without_frame_hides_nothing() {
    let mut m = Model::new(Saved { apps: vec![app("a"), app("b")], frame: None, active_app: None });
    ev(&mut m, Event::TrustChanged(true));
    running(&mut m, "a", vec![win(1, "a1")]);
    assert_eq!(activate_app(&mut m, "b"), vec![save()]);

    let mut m = model_with_two_running_apps();
    ev(&mut m, Event::TrustChanged(false));
    assert_eq!(activate_app(&mut m, "b"), vec![save()]);
}

#[test]
fn add_app_hides_the_running_apps_then_launches() {
    let mut m = model_with(&["a", "c"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "c", vec![win(5, "c5")]);
    let effects = act(&mut m, Action::AddApp(app("b")));
    assert_eq!(effects, vec![watch("b"), hide("a"), hide("c"), launch("b"), save()]);
    assert_eq!(bundles(&m), vec!["a", "c", "b"]);
}

#[test]
fn add_existing_app_with_a_placeholder_hides_the_running_apps() {
    let mut m = model_with_two_running_apps();
    let effects = act(&mut m, Action::AddApp(app("b")));
    assert_eq!(effects, vec![hide("a"), hide("c"), launch("b"), save()]);
}

#[test]
fn closing_the_active_placeholder_hides_the_apps_before_presenting_the_next_placeholder() {
    // Tabs: a1, b (active), c (placeholder, not running), d5.
    let mut m = model_with(&["a", "b", "c", "d"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "d", vec![win(5, "d5")]);
    activate_app(&mut m, "b");
    let effects = close_app(&mut m, "b");
    assert_eq!(effects, vec![unwatch("b"), hide("a"), hide("d"), launch("c"), save()]);
    assert_eq!(m.active, Some(app_tab(&m, "c")));
}

#[test]
fn closing_a_running_active_placeholder_quits_it_then_hides_and_reopens_the_next() {
    // Tabs: a1, b (running, no window, active), c (running, no window).
    let mut m = model_with(&["a", "b", "c"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![]);
    running(&mut m, "c", vec![]);
    activate_app(&mut m, "b");
    let effects = close_app(&mut m, "b");
    assert_eq!(effects, vec![quit_app("b"), unwatch("b"), hide("a"), reopen("c"), save()]);
}

#[test]
fn window_closed_by_oriel_hides_the_other_apps_before_presenting_a_placeholder() {
    // Tabs: a1 (active), b (placeholder), c5.
    let mut m = model_with_two_running_apps();
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    let effects = closed(&mut m, "a", 1);
    assert_eq!(effects, vec![quit_app("a"), unwatch("a"), hide("c"), launch("b"), save()]);
}

#[test]
fn app_running_with_no_windows_for_an_app_that_had_windows_expires_instead_of_reopening() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    activate_window(&mut m, 3);
    let b = window_tab(&m, 3);
    let effects = running(&mut m, "b", vec![]);
    // b lost its windows: expiring, nothing is reopened.
    assert_eq!(effects, vec![expire(b), save()]);
}

// ---------------------------------------------------------------------------
// The mark "closing by Oriel" does not go stale
// ---------------------------------------------------------------------------

/// App a runs with window 1. The user clicks close and the app shows a save dialog. The user
/// cancels it, so no `WindowClosed` arrives and the window stays open.
fn model_with_cancelled_close() -> Model {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    m
}

/// The user closes the app's last window. The tab must become an expiring placeholder, with no quit.
fn assert_user_close_keeps_the_app(m: &mut Model) {
    let id = app_tab(m, "a");
    let effects = closed(m, "a", 1);
    assert_eq!(effects, vec![expire(id), save()]);
    assert!(!effects.contains(&quit_app("a")));
    assert_eq!(windows(m), vec![None, None]);
    assert_eq!(m.tab(id).expect("tab").window, None);
    assert!(m.is_running("a"));
    assert_invariants(m);
}

#[test]
fn cancelled_close_then_activate_then_user_close_leaves_a_placeholder() {
    let mut m = model_with_cancelled_close();
    assert_eq!(activate_window(&mut m, 1), vec![present("a", 1), raise(), save()]);
    assert_user_close_keeps_the_app(&mut m);
}

#[test]
fn cancelled_close_then_window_frame_then_user_close_leaves_a_placeholder() {
    let mut m = model_with_cancelled_close();
    // Even a frame close to the content (no other effect) shows that the window lives.
    let same = content();
    assert_eq!(ev(&mut m, Event::WindowFrame { window: WindowId(1), frame: same, by_user: true }), none());
    assert_user_close_keeps_the_app(&mut m);
}

#[test]
fn cancelled_close_then_a_moved_window_then_user_close_leaves_a_placeholder() {
    let mut m = model_with_cancelled_close();
    let moved = Rect::new(10.0, 100.0, 800.0, 600.0);
    let effects = ev(&mut m, Event::WindowFrame { window: WindowId(1), frame: moved, by_user: true });
    assert_eq!(effects.len(), 2);
    assert_user_close_keeps_the_app(&mut m);
}

#[test]
fn cancelled_close_then_title_change_then_user_close_leaves_a_placeholder() {
    let mut m = model_with_cancelled_close();
    let title = Event::WindowTitle { window: WindowId(1), title: "edited".into() };
    assert_eq!(ev(&mut m, title), none());
    assert_user_close_keeps_the_app(&mut m);
}

#[test]
fn a_title_or_frame_of_another_window_keeps_the_mark() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    ev(&mut m, Event::WindowTitle { window: WindowId(2), title: "other".into() });
    ev(&mut m, Event::WindowFrame { window: WindowId(2), frame: Rect::new(1.0, 99.0, 700.0, 500.0), by_user: true });
    activate_window(&mut m, 2);
    // Window 1 is still closing: closing window 2 afterwards leaves window 1 as the last one.
    assert_eq!(closed(&mut m, "a", 2), vec![present("a", 1), raise(), save()]);
    assert_eq!(closed(&mut m, "a", 1)[..2], [quit_app("a"), unwatch("a")]);
}

#[test]
fn a_focus_echo_after_close_does_not_forget_the_mark() {
    // The platform can send WindowFocused for a window that is closing (a save dialog changes
    // the main window). That must not turn the close into a "user close".
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    let focus = Event::WindowFocused { bundle_id: "a".into(), window: WindowId(1) };
    assert_eq!(ev(&mut m, focus), vec![raise()]);
    let effects = closed(&mut m, "a", 1);
    assert_eq!(effects, vec![quit_app("a"), unwatch("a"), launch("b"), save()]);
    assert_eq!(bundles(&m), vec!["b"]);
}

#[test]
fn a_focus_echo_after_close_of_an_inactive_tab_does_not_forget_the_mark() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    activate_window(&mut m, 3);
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    // The echo switches to the tab (the user sees the dialog there). Still closing.
    let focus = Event::WindowFocused { bundle_id: "a".into(), window: WindowId(1) };
    ev(&mut m, focus);
    let effects = closed(&mut m, "a", 1);
    assert_eq!(effects[..2], [quit_app("a"), unwatch("a")]);
    assert!(m.tabs.iter().all(|t| t.app.bundle_id != "a"));
}

#[test]
fn normal_close_still_quits_the_app_and_unwatches() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    let effects = closed(&mut m, "a", 1);
    assert_eq!(effects, vec![quit_app("a"), unwatch("a"), save()]);
    assert!(m.tabs.is_empty());
    assert!(!m.is_running("a"));
}

// ---------------------------------------------------------------------------
// Closed tabs are forgotten (expiring placeholders)
// ---------------------------------------------------------------------------

/// Apps a (window 1, active) and b (window 3, hidden) run. Tabs: a1, b3.
fn model_with_a_and_b() -> Model {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    m
}

#[test]
fn native_close_of_the_last_window_of_the_active_app_expires_the_tab_after_the_grace() {
    let mut m = model_with_a_and_b();
    let a = window_tab(&m, 1);
    assert_eq!(closed(&mut m, "a", 1), vec![expire(a), save()]);
    assert!(m.is_expiring(a));
    assert!(m.is_running("a"));
    // During the grace the tab stays, dormant, and is not saved.
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(m.saved().apps, vec![app("b")]);
    assert_eq!(m.saved().active_app.as_deref(), Some("b"));
    // The grace runs out: unwatch (no quit, no show), the next tab is active and presented.
    let effects = expired(&mut m, a);
    assert_eq!(effects, vec![unwatch("a"), present("b", 3), raise(), save()]);
    assert_eq!(bundles(&m), vec!["b"]);
    assert_eq!(m.active, Some(window_tab(&m, 3)));
    assert!(!m.is_running("a"));
    assert!(!m.is_expiring(a));
    assert_invariants(&m);
}

#[test]
fn native_close_of_the_last_window_of_a_hidden_app_expires_without_show_app() {
    let mut m = model_with_a_and_b();
    activate_window(&mut m, 3);
    let a = window_tab(&m, 1);
    assert_eq!(closed(&mut m, "a", 1), vec![expire(a), save()]);
    let effects = expired(&mut m, a);
    // The app is not shown (it would pop up for no reason) and not quit; the active tab stays.
    assert_eq!(effects, vec![unwatch("a"), save()]);
    assert_eq!(m.active, Some(window_tab(&m, 3)));
    assert_eq!(bundles(&m), vec!["b"]);
}

#[test]
fn closing_an_expiring_placeholder_with_x_removes_it_at_once_and_quits_a_running_app() {
    let mut m = model_with_a_and_b();
    let a = window_tab(&m, 1);
    closed(&mut m, "a", 1);
    let effects = act(&mut m, Action::Close(a));
    assert_eq!(effects, vec![quit_app("a"), unwatch("a"), present("b", 3), raise(), save()]);
    assert!(!m.is_expiring(a));
    // The timer fires later: nothing.
    assert_eq!(expired(&mut m, a), none());
}

#[test]
fn x_close_of_the_last_window_still_quits_and_forgets_at_once() {
    let mut m = model_with_a_and_b();
    assert_eq!(close_window_tab(&mut m, 1), vec![close_window(1)]);
    let effects = closed(&mut m, "a", 1);
    assert_eq!(effects, vec![quit_app("a"), unwatch("a"), present("b", 3), raise(), save()]);
    assert_eq!(bundles(&m), vec!["b"]);
    assert!(m.inner.expiring.is_empty());
}

#[test]
fn app_stopped_makes_one_expiring_placeholder_and_expiry_removes_it() {
    let mut m = model_with(&["a", "b"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    let first = window_tab(&m, 1);
    assert_eq!(stopped(&mut m, "a"), vec![expire(first), save()]);
    assert_eq!(windows(&m), vec![None, Some(3)]);
    assert!(m.is_expiring(first));
    assert_eq!(m.active, Some(first));
    assert!(!m.is_running("a"));
    assert_eq!(m.saved().apps, vec![app("b")]);
    let effects = expired(&mut m, first);
    assert_eq!(effects, vec![unwatch("a"), present("b", 3), raise(), save()]);
    assert_eq!(bundles(&m), vec!["b"]);
    assert_invariants(&m);
}

#[test]
fn app_stopped_of_a_background_app_expires_without_touching_the_active_tab() {
    let mut m = model_with_a_and_b();
    let b = window_tab(&m, 3);
    stopped(&mut m, "b");
    let effects = expired(&mut m, b);
    assert_eq!(effects, vec![unwatch("b"), save()]);
    assert_eq!(bundles(&m), vec!["a"]);
    assert_eq!(m.active, Some(window_tab(&m, 1)));
}

#[test]
fn app_stopped_of_an_app_that_never_ran_changes_nothing() {
    let mut m = model_with(&["a"]);
    let before = snapshot(&m);
    assert_eq!(stopped(&mut m, "a"), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn the_grace_is_cancelled_by_window_opened() {
    let mut m = model_with_a_and_b();
    let a = window_tab(&m, 1);
    closed(&mut m, "a", 1);
    // A splash window closed, the main window opens: the tab is the new window.
    let effects = opened(&mut m, "a", win(5, "main"));
    assert_eq!(effects, vec![present("a", 5), raise(), hide("b"), save()]);
    assert!(!m.is_expiring(a));
    assert_eq!(m.tab(a).expect("tab").window, Some(win(5, "main")));
    assert_eq!(expired(&mut m, a), none());
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(m.saved().apps, vec![app("a"), app("b")]);
}

#[test]
fn window_opened_for_a_background_expiring_app_only_cancels_and_saves() {
    let mut m = model_with_a_and_b();
    activate_window(&mut m, 3);
    let a = window_tab(&m, 1);
    closed(&mut m, "a", 1);
    assert_eq!(opened(&mut m, "a", win(5, "main")), vec![save()]);
    assert!(!m.is_expiring(a));
    assert_eq!(expired(&mut m, a), none());
}

#[test]
fn the_grace_is_cancelled_by_app_running_with_windows() {
    // An updater: the app stops and runs again with a window.
    let mut m = model_with_a_and_b();
    let a = window_tab(&m, 1);
    stopped(&mut m, "a");
    let effects = running(&mut m, "a", vec![win(1, "a1")]);
    assert_eq!(effects, vec![present("a", 1), raise(), hide("b"), save()]);
    assert!(!m.is_expiring(a));
    assert!(m.is_running("a"));
    assert_eq!(expired(&mut m, a), none());
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(m.saved().apps, vec![app("a"), app("b")]);
}

#[test]
fn a_relaunch_blip_keeps_the_app_in_the_bar() {
    // The platform reports AppStopped then AppRunning for an app that kept running.
    let mut m = model_with_a_and_b();
    activate_window(&mut m, 3);
    let a = window_tab(&m, 1);
    stopped(&mut m, "a");
    assert_eq!(running(&mut m, "a", vec![win(1, "a1")]), vec![hide("a"), save()]);
    assert_eq!(expired(&mut m, a), none());
    assert_eq!(windows(&m), vec![Some(1), Some(3)]);
    assert_invariants(&m);
}

#[test]
fn app_running_without_windows_keeps_the_expiring_mark_and_does_not_reopen() {
    let mut m = model_with_a_and_b();
    let a = window_tab(&m, 1);
    closed(&mut m, "a", 1);
    // A second Watch reports the app again, still without a window.
    assert_eq!(running(&mut m, "a", vec![]), none());
    assert!(m.is_expiring(a));
    assert_eq!(expired(&mut m, a)[0], unwatch("a"));
}

#[test]
fn activate_cancels_the_grace_and_launches_a_stopped_app() {
    let mut m = model_with_a_and_b();
    let a = window_tab(&m, 1);
    stopped(&mut m, "a");
    let effects = activate_app(&mut m, "a");
    assert_eq!(effects, vec![hide("b"), launch("a"), save()]);
    assert!(!m.is_expiring(a));
    assert_eq!(expired(&mut m, a), none());
    // It is a normal dormant placeholder again, and it is saved.
    assert_eq!(m.saved().apps, vec![app("a"), app("b")]);
}

#[test]
fn activate_cancels_the_grace_and_reopens_a_running_app() {
    let mut m = model_with_a_and_b();
    let a = window_tab(&m, 1);
    closed(&mut m, "a", 1);
    activate_window(&mut m, 3);
    let effects = act(&mut m, Action::Activate(a));
    assert_eq!(effects, vec![hide("b"), reopen("a"), save()]);
    assert!(!m.is_expiring(a));
    assert_eq!(expired(&mut m, a), none());
    assert_eq!(bundles(&m), vec!["a", "b"]);
}

#[test]
fn a_new_expiry_starts_the_grace_again() {
    let mut m = model_with_a_and_b();
    let a = window_tab(&m, 1);
    closed(&mut m, "a", 1);
    // The app quits during the grace: the grace restarts (the app crate replaces the deadline).
    assert_eq!(stopped(&mut m, "a"), vec![expire(a), save()]);
    assert!(m.is_expiring(a));
}

#[test]
fn add_app_never_expires_while_it_launches() {
    let mut m = model_with(&["a"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    let effects = act(&mut m, Action::AddApp(app("c")));
    assert_eq!(effects, vec![watch("c"), hide("a"), launch("c"), save()]);
    let c = app_tab(&m, "c");
    assert!(!m.is_expiring(c));
    // A stray ExpireTab, or an AppStopped before the app ran, does nothing.
    assert_eq!(expired(&mut m, c), none());
    assert_eq!(stopped(&mut m, "c"), none());
    // The launch is slow: the app runs without a window for a while.
    assert_eq!(running(&mut m, "c", vec![]), vec![hide("a"), reopen("c")]);
    assert!(!m.is_expiring(c));
    assert_eq!(expired(&mut m, c), none());
    assert_eq!(bundles(&m), vec!["a", "c"]);
    // Its window opens. Only now is it subject to the closing rules.
    opened(&mut m, "c", win(9, "c9"));
    assert_eq!(closed(&mut m, "c", 9), vec![expire(c), save()]);
}

#[test]
fn a_saved_placeholder_at_start_stays_dormant() {
    let mut m = model_with(&["a", "b"]);
    let b = app_tab(&m, "b");
    assert!(m.inner.expiring.is_empty());
    assert_eq!(expired(&mut m, b), none());
    assert_eq!(bundles(&m), vec!["a", "b"]);
    assert_eq!(m.saved().apps, vec![app("a"), app("b")]);
}

#[test]
fn expire_tab_for_an_unknown_or_no_longer_expiring_tab_does_nothing() {
    let mut m = model_with_a_and_b();
    let before = snapshot(&m);
    assert_eq!(expired(&mut m, TabId(999)), none());
    let open = window_tab(&m, 1);
    assert_eq!(expired(&mut m, open), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn saved_leaves_expiring_placeholders_out_and_round_trips() {
    let mut m = model_with(&["a", "b", "c"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    running(&mut m, "c", vec![win(5, "c5")]);
    activate_window(&mut m, 3);
    stopped(&mut m, "b");
    let saved = m.saved();
    assert_eq!(saved.apps, vec![app("a"), app("c")]);
    // The active tab is expiring: the saved active app is the one that would take over.
    assert_eq!(saved.active_app.as_deref(), Some("c"));
    assert_eq!(Model::new(saved.clone()).saved(), saved);
}

#[test]
fn saved_active_app_falls_back_to_the_last_tab_when_no_tab_follows() {
    let mut m = model_with_a_and_b();
    activate_window(&mut m, 3);
    stopped(&mut m, "b");
    assert_eq!(m.saved().active_app.as_deref(), Some("a"));
    stopped(&mut m, "a");
    let saved = m.saved();
    assert!(saved.apps.is_empty());
    assert_eq!(saved.active_app, None);
}

#[test]
fn quit_during_the_grace_keeps_open_tabs_and_forgets_the_expiring_one() {
    let mut m = model_with_a_and_b();
    closed(&mut m, "a", 1);
    // Oriel exits: it is not a close. Running apps are shown, and the save omits tab a.
    assert_eq!(act(&mut m, Action::Quit), vec![show("a"), show("b"), save()]);
    assert_eq!(m.saved().apps, vec![app("b")]);
}

#[test]
fn quit_forgets_nothing_that_is_open() {
    let mut m = model_with_a_and_b();
    act(&mut m, Action::Quit);
    assert_eq!(m.saved().apps, vec![app("a"), app("b")]);
}

#[test]
fn a_dormant_placeholder_of_a_running_app_reopens_when_the_app_reports_no_window() {
    // A saved app at launch: it was never closed by the user, so it is not expiring.
    let mut m = model_with(&["a"]);
    assert_eq!(running(&mut m, "a", vec![]), vec![reopen("a")]);
    assert!(m.inner.expiring.is_empty());
}

// ---------------------------------------------------------------------------
// A saved frame and the first screen event
// ---------------------------------------------------------------------------

#[test]
fn first_screen_moves_the_bar_to_a_saved_frame_once() {
    let saved_frame = Rect::new(100.0, 100.0, 900.0, 600.0);
    let mut m = Model::new(Saved { apps: vec![app("a")], frame: Some(saved_frame), active_app: None });
    let effects = ev(&mut m, Event::ScreenChanged { visible: screen() });
    assert_eq!(effects, vec![Effect::MoveBar(saved_frame.bar())]);
    assert_eq!(m.frame, Some(saved_frame));
    let later = ev(&mut m, Event::ScreenChanged { visible: screen() });
    assert!(later.is_empty());
}

#[test]
fn first_screen_resets_a_saved_frame_that_is_off_screen() {
    let off = Rect::new(3000.0, 100.0, 900.0, 600.0);
    let mut m = Model::new(Saved { apps: vec![app("a")], frame: Some(off), active_app: None });
    let effects = ev(&mut m, Event::ScreenChanged { visible: screen() });
    assert_eq!(effects, vec![Effect::MoveBar(screen().bar())]);
    assert_eq!(m.frame, Some(screen()));
}

// ---------------------------------------------------------------------------
// Split view
// ---------------------------------------------------------------------------

fn divider_effect(rect: Option<Rect>) -> Effect {
    Effect::Divider(rect)
}

fn present_in(b: &str, w: u32, frame: Rect) -> Effect {
    cmd(Command::Present { bundle_id: b.into(), window: WindowId(w), frame })
}

/// The panes, the divider and the right pane of the default frame at ratio one half.
fn halves() -> (Rect, Rect, Rect) {
    split_rects(&content(), 0.5)
}

/// Apps a (windows 1 and 2), b (window 3) and c (window 4), all running. Tab a1 is active.
fn split_model() -> Model {
    let mut m = model_with(&["a", "b", "c"]);
    running(&mut m, "a", vec![win(1, "a1"), win(2, "a2")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    running(&mut m, "c", vec![win(4, "c4")]);
    assert_eq!(m.active, Some(window_tab(&m, 1)));
    m
}

fn split_tab(m: &mut Model, window: u32) -> Vec<Effect> {
    let id = window_tab(m, window);
    act(m, Action::Split(id))
}

/// a1 and b3 are split, b3 is active. The effects of the setup are dropped.
fn split_a1_b3() -> Model {
    let mut m = split_model();
    split_tab(&mut m, 3);
    m
}

fn tab_of(m: &Model, window: u32) -> TabId {
    window_tab(m, window)
}

fn focused(m: &mut Model, bundle: &str, window: u32) -> Vec<Effect> {
    ev(m, Event::WindowFocused { bundle_id: bundle.into(), window: WindowId(window) })
}

#[test]
fn split_geometry_tiles_the_content_exactly() {
    for width in [300.0, 640.0, 700.0, 1000.0, 1440.0, 1440.5, 2560.0] {
        for ratio in [-1.0, 0.0, 0.1, 0.3333, 0.5, 0.77, 1.0, 5.0, f64::NAN] {
            let content = Rect::new(12.5, 65.0, width, 835.0);
            let (l, d, r) = split_rects(&content, ratio);
            assert_eq!(l.x, content.x);
            assert_eq!(l.x + l.w, d.x, "the divider follows the left pane");
            assert_eq!(d.w, SPLIT_GAP);
            assert_eq!(d.x + d.w, r.x, "the right pane follows the divider");
            assert!((r.x + r.w - (content.x + content.w)).abs() < 1e-9, "the panes end where the content ends");
            for rect in [l, d, r] {
                assert_eq!((rect.y, rect.h), (content.y, content.h));
            }
            assert_eq!(l.w, l.w.round(), "the left width is a whole number of points");
        }
    }
}

#[test]
fn split_ratio_keeps_both_panes_at_the_minimum_when_there_is_room() {
    let content = Rect::new(0.0, 65.0, 1440.0, 835.0);
    let (l, _, r) = split_rects(&content, 0.0);
    assert_eq!(l.w, MIN_PANE);
    assert!(r.w >= MIN_PANE);
    let (l, _, r) = split_rects(&content, 1.0);
    assert_eq!(r.w, MIN_PANE);
    assert!(l.w >= MIN_PANE);
}

#[test]
fn split_ratio_falls_back_to_a_fifth_to_four_fifths_in_a_narrow_content() {
    // 600 - 8 = 592 is below two minimum panes.
    assert_eq!(clamp_ratio(600.0, 0.0), 0.2);
    assert_eq!(clamp_ratio(600.0, 1.0), 0.8);
    assert_eq!(clamp_ratio(600.0, 0.5), 0.5);
    assert_eq!(clamp_ratio(600.0, f64::NAN), 0.5);
}

#[test]
fn a_new_model_has_no_split_and_saved_does_not_hold_one() {
    let m = split_a1_b3();
    assert!(m.split.is_some());
    let saved = m.saved();
    assert_eq!(saved.apps.len(), 3);
    assert!(Model::new(saved).split.is_none());
}

#[test]
fn split_presents_both_windows_the_asked_one_last_and_shows_the_divider() {
    let mut m = split_model();
    let (l, d, r) = halves();
    let effects = split_tab(&mut m, 3);
    assert_eq!(
        effects,
        vec![
            present_in("a", 1, l),
            present_in("b", 3, r),
            raise(),
            minimize(2),
            hide("c"),
            save(),
            divider_effect(Some(d)),
        ]
    );
    assert_eq!(m.split, Some(Split { left: tab_of(&m, 1), right: tab_of(&m, 3), ratio: 0.5 }));
    assert_eq!(m.active, Some(tab_of(&m, 3)));
    assert_eq!(m.divider(), Some(d));
    assert_eq!(d.w, SPLIT_GAP);
    assert_invariants(&m);
}

#[test]
fn split_of_two_windows_of_one_app_minimizes_nothing_of_the_pair() {
    let mut m = split_model();
    let (l, d, r) = halves();
    let effects = split_tab(&mut m, 2);
    assert_eq!(
        effects,
        vec![
            present_in("a", 1, l),
            present_in("a", 2, r),
            raise(),
            hide("b"),
            hide("c"),
            save(),
            divider_effect(Some(d)),
        ]
    );
}

#[test]
fn split_minimizes_the_other_windows_of_both_apps() {
    let mut m = split_model();
    running(&mut m, "b", vec![win(3, "b3"), win(5, "b5")]);
    let (l, d, r) = halves();
    let effects = split_tab(&mut m, 3);
    assert_eq!(
        effects,
        vec![
            present_in("a", 1, l),
            present_in("b", 3, r),
            raise(),
            minimize(2),
            minimize(5),
            hide("c"),
            save(),
            divider_effect(Some(d)),
        ]
    );
}

#[test]
fn split_does_not_reorder_tabs() {
    let mut m = split_model();
    let before = windows(&m);
    split_tab(&mut m, 4);
    assert_eq!(windows(&m), before);
}

#[test]
fn split_with_the_active_tab_itself_a_placeholder_or_an_unknown_tab_does_nothing() {
    let mut m = model_with(&["a", "b", "c"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![]);
    running(&mut m, "c", vec![win(4, "c4")]);
    let before = snapshot(&m);
    let active = m.active.unwrap();
    assert_eq!(act(&mut m, Action::Split(active)), none(), "the active tab itself");
    let placeholder = app_tab(&m, "b");
    assert_eq!(act(&mut m, Action::Split(placeholder)), none(), "a placeholder");
    assert_eq!(act(&mut m, Action::Split(TabId(999))), none(), "an unknown tab");
    assert_eq!(snapshot(&m), before);
    // The active tab is a placeholder.
    activate_app(&mut m, "b");
    let c4 = tab_of(&m, 4);
    let before = snapshot(&m);
    assert_eq!(act(&mut m, Action::Split(c4)), none());
    assert_eq!(snapshot(&m), before);
    assert_eq!(m.split, None);
}

#[test]
fn a_new_split_replaces_the_old_pair() {
    let mut m = split_a1_b3();
    // b3 is active; split it with c4: the pair becomes b3 and c4.
    split_tab(&mut m, 4);
    assert_eq!(m.split, Some(Split { left: tab_of(&m, 3), right: tab_of(&m, 4), ratio: 0.5 }));
    assert_eq!(m.active, Some(tab_of(&m, 4)));
    assert_invariants(&m);
}

#[test]
fn split_while_untrusted_or_without_frame_still_joins_but_sends_no_command() {
    let mut m = split_model();
    ev(&mut m, Event::TrustChanged(false));
    let effects = split_tab(&mut m, 3);
    assert_eq!(effects, vec![save()]);
    assert!(m.split.is_some());
    assert_eq!(m.divider(), None, "no divider without trust");
}

#[test]
fn unsplit_without_a_split_does_nothing() {
    let mut m = split_model();
    let before = snapshot(&m);
    assert_eq!(act(&mut m, Action::Unsplit), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn unsplit_of_a_shown_split_presents_the_active_tab_alone() {
    let mut m = split_a1_b3();
    let effects = act(&mut m, Action::Unsplit);
    assert_eq!(
        effects,
        vec![present("b", 3), raise(), hide("a"), hide("c"), divider_effect(None)]
    );
    assert_eq!(m.split, None);
    assert_eq!(m.divider(), None);
    // The pair's other window is gone from the screen: a1 is hidden with its app.
    assert_invariants(&m);
}

#[test]
fn unsplit_while_a_third_tab_is_active_only_forgets_the_pair() {
    let mut m = split_a1_b3();
    activate_window(&mut m, 4);
    let effects = act(&mut m, Action::Unsplit);
    assert_eq!(effects, none());
    assert_eq!(m.split, None);
    assert_eq!(m.active, Some(tab_of(&m, 4)));
}

#[test]
fn split_ratio_moves_both_windows_and_the_divider() {
    let mut m = split_a1_b3();
    let (l, d, r) = split_rects(&content(), 0.25);
    let effects = act(&mut m, Action::SplitRatio(0.25));
    assert_eq!(effects, vec![set_frame(1, l), set_frame(3, r), divider_effect(Some(d))]);
    assert_eq!(m.split.unwrap().ratio, 0.25);
    assert_eq!(d.x, l.w);
}

#[test]
fn split_ratio_is_clamped_to_the_minimum_pane() {
    let mut m = split_a1_b3();
    let effects = act(&mut m, Action::SplitRatio(0.0));
    let (l, d, r) = split_rects(&content(), 0.0);
    assert_eq!(l.w, MIN_PANE);
    assert_eq!(effects, vec![set_frame(1, l), set_frame(3, r), divider_effect(Some(d))]);
    // Asking for the same pixels again changes nothing.
    assert_eq!(act(&mut m, Action::SplitRatio(-3.0)), none());
    assert_eq!(act(&mut m, Action::SplitRatio(f64::NAN)).len(), 3, "not a number gives one half");
}

#[test]
fn split_ratio_of_nearly_the_same_pixel_does_nothing() {
    let mut m = split_a1_b3();
    assert_eq!(act(&mut m, Action::SplitRatio(0.5001)), none());
    assert_eq!(m.split.unwrap().ratio, 0.5);
}

#[test]
fn split_ratio_without_a_shown_split_does_nothing() {
    let mut m = split_model();
    assert_eq!(act(&mut m, Action::SplitRatio(0.3)), none(), "no split");
    split_tab(&mut m, 3);
    activate_window(&mut m, 4);
    let before = snapshot(&m);
    assert_eq!(act(&mut m, Action::SplitRatio(0.3)), none(), "a third tab is active");
    assert_eq!(snapshot(&m), before);
}

#[test]
fn a_third_tab_shows_alone_and_the_pair_stays_joined() {
    let mut m = split_a1_b3();
    let effects = activate_window(&mut m, 4);
    assert_eq!(
        effects,
        vec![present("c", 4), raise(), hide("a"), hide("b"), save(), divider_effect(None)]
    );
    assert!(m.split.is_some(), "the pair stays joined");
    assert_eq!(m.divider(), None);
    // A click on either member brings the two windows back side by side.
    let (l, d, r) = halves();
    let effects = activate_window(&mut m, 1);
    assert_eq!(
        effects,
        vec![
            present_in("b", 3, r),
            present_in("a", 1, l),
            raise(),
            minimize(2),
            hide("c"),
            save(),
            divider_effect(Some(d)),
        ]
    );
    activate_window(&mut m, 4);
    let effects = activate_window(&mut m, 3);
    assert_eq!(effects[0], present_in("a", 1, l));
    assert_eq!(effects[1], present_in("b", 3, r));
}

#[test]
fn activating_a_member_of_a_shown_split_presents_the_pair_again() {
    let mut m = split_a1_b3();
    let (l, d, r) = halves();
    let effects = activate_window(&mut m, 3);
    assert_eq!(effects[0], present_in("a", 1, l));
    assert_eq!(effects[1], present_in("b", 3, r));
    assert!(!effects.contains(&divider_effect(Some(d))), "the divider did not change");
    assert_eq!(m.active, Some(tab_of(&m, 3)));
}

#[test]
fn a_click_into_the_other_pane_only_makes_it_active_and_raises_the_bar() {
    let mut m = split_a1_b3();
    let effects = focused(&mut m, "a", 1);
    assert_eq!(effects, vec![raise(), save()]);
    assert_eq!(m.active, Some(tab_of(&m, 1)));
    assert!(m.split.is_some());
    // On the active tab: only RaiseBar.
    assert_eq!(focused(&mut m, "a", 1), vec![raise()]);
}

#[test]
fn the_focus_echoes_of_two_presents_converge_on_the_last_one() {
    let mut m = split_model();
    split_tab(&mut m, 3);
    // The platform echoes each Present, in order.
    assert_eq!(focused(&mut m, "a", 1), vec![raise(), save()]);
    assert_eq!(focused(&mut m, "b", 3), vec![raise(), save()]);
    assert_eq!(m.active, Some(tab_of(&m, 3)));
    // Repeats change nothing but the raise.
    assert_eq!(focused(&mut m, "b", 3), vec![raise()]);
    assert_invariants(&m);
}

#[test]
fn a_member_brought_forward_while_a_third_tab_is_active_brings_the_split_back() {
    let mut m = split_a1_b3();
    activate_window(&mut m, 4);
    let (l, d, r) = halves();
    let effects = focused(&mut m, "a", 1);
    assert_eq!(effects[0], present_in("b", 3, r));
    assert_eq!(effects[1], present_in("a", 1, l));
    assert_eq!(effects.last(), Some(&divider_effect(Some(d))));
    assert_eq!(m.active, Some(tab_of(&m, 1)));
}

#[test]
fn bar_moved_moves_both_windows_and_the_divider() {
    let mut m = split_a1_b3();
    let moved = Rect::new(100.0, 125.0, 1440.0, BAR_H);
    let effects = act(&mut m, Action::BarMoved(moved));
    let frame = m.frame.unwrap();
    assert_eq!((frame.x, frame.y, frame.w, frame.h), (100.0, 125.0, 1440.0, 875.0));
    let (l, d, r) = split_rects(&frame.content(), 0.5);
    assert_eq!(effects, vec![set_frame(1, l), set_frame(3, r), save(), divider_effect(Some(d))]);
}

const BAR_H: f64 = crate::types::BAR_HEIGHT;

#[test]
fn bar_resized_in_width_asks_both_windows_for_their_new_panes() {
    let mut m = split_a1_b3();
    let effects = act(&mut m, Action::BarMoved(Rect::new(0.0, 25.0, 1000.0, BAR_H)));
    let frame = m.frame.unwrap();
    let (l, d, r) = split_rects(&frame.content(), 0.5);
    assert_eq!(effects, vec![set_frame(1, l), set_frame(3, r), save(), divider_effect(Some(d))]);
    assert_eq!(l.w + SPLIT_GAP + r.w, 1000.0);
}

#[test]
fn zoom_sends_both_windows_their_panes() {
    let mut m = split_a1_b3();
    let target = Rect::new(50.0, 50.0, 1000.0, 700.0);
    let effects = act(&mut m, Action::Zoom(target));
    let (l, d, r) = split_rects(&target.content(), 0.5);
    assert_eq!(
        effects,
        vec![move_bar(target.bar()), set_frame(1, l), set_frame(3, r), save(), divider_effect(Some(d))]
    );
}

#[test]
fn bar_moved_while_a_third_tab_is_active_moves_only_its_window_and_the_divider_stays_hidden() {
    let mut m = split_a1_b3();
    activate_window(&mut m, 4);
    let effects = act(&mut m, Action::BarMoved(Rect::new(10.0, 25.0, 1440.0, BAR_H)));
    let content = m.frame.unwrap().content();
    assert_eq!(effects, vec![set_frame(4, content), save()]);
}

#[test]
fn a_refusal_in_a_pane_puts_the_window_at_its_fit_in_that_pane_once() {
    let mut m = split_a1_b3();
    let (_, _, r) = halves();
    let actual = Rect::new(r.x, r.y, 230.0, 408.0);
    let fit = r.fit(230.0, 408.0);
    assert_eq!(ev(&mut m, reported(3, actual, false)), vec![set_frame(3, fit)]);
    // The OS clamped the position: no second correction.
    assert_eq!(ev(&mut m, reported(3, Rect::new(r.x + 4.0, r.y + 5.0, 230.0, 408.0), false)), none());
    // The left window refuses on its own, with its own correction mark.
    let (l, _, _) = halves();
    let actual = Rect::new(l.x, l.y, 300.0, 300.0);
    assert_eq!(ev(&mut m, reported(1, actual, false)), vec![set_frame(1, l.fit(300.0, 300.0))]);
    assert_eq!(m.frame, Some(screen()), "a refusal never changes the frame");
}

#[test]
fn a_fixed_size_window_in_a_pane_follows_the_bar_at_its_fit() {
    let mut m = split_a1_b3();
    let (_, _, r) = halves();
    let actual = Rect::new(r.x, r.y, 230.0, 408.0);
    ev(&mut m, reported(3, actual, false));
    let effects = act(&mut m, Action::BarMoved(Rect::new(40.0, 25.0, 1440.0, BAR_H)));
    let frame = m.frame.unwrap();
    let (l, _, r) = split_rects(&frame.content(), 0.5);
    assert_eq!(effects[0], set_frame(1, l));
    assert_eq!(effects[1], set_frame(3, r.fit(230.0, 408.0)));
}

#[test]
fn a_user_move_of_the_left_pane_moves_the_whole_frame_and_the_other_window() {
    let mut m = split_a1_b3();
    let (l, _, _) = halves();
    let held = Rect::new(l.x + 30.0, l.y + 20.0, l.w, l.h);
    let effects = ev(&mut m, reported(1, held, true));
    let frame = m.frame.unwrap();
    assert_eq!((frame.x, frame.y, frame.w, frame.h), (30.0, 45.0, 1440.0, 875.0));
    let (_, d, r) = split_rects(&frame.content(), 0.5);
    assert_eq!(effects, vec![move_bar(frame.bar()), set_frame(3, r), save(), divider_effect(Some(d))]);
    assert!(!effects.contains(&set_frame(1, held)), "never a SetFrame for the window the user holds");
    assert_eq!(m.split.unwrap().ratio, 0.5);
}

#[test]
fn a_user_move_of_the_right_pane_moves_the_frame_and_the_left_window() {
    let mut m = split_a1_b3();
    let (_, _, r) = halves();
    let held = Rect::new(r.x - 10.0, r.y, r.w, r.h);
    let effects = ev(&mut m, reported(3, held, true));
    let frame = m.frame.unwrap();
    assert_eq!(frame.x, -10.0);
    let (l, d, _) = split_rects(&frame.content(), 0.5);
    assert_eq!(effects, vec![move_bar(frame.bar()), set_frame(1, l), save(), divider_effect(Some(d))]);
}

#[test]
fn a_user_resize_of_the_left_pane_sets_the_left_edge_and_the_divider() {
    let mut m = split_a1_b3();
    let (l, _, _) = halves();
    // The user pulled the left pane's right edge by 100 points and its bottom edge by 20.
    let held = Rect::new(l.x, l.y, l.w + 100.0, l.h + 20.0);
    let effects = ev(&mut m, reported(1, held, true));
    let frame = m.frame.unwrap();
    assert_eq!((frame.x, frame.y, frame.w, frame.h), (0.0, 25.0, 1440.0, 895.0));
    let usable = 1440.0 - SPLIT_GAP;
    let ratio = (l.w + 100.0) / usable;
    assert!((m.split.unwrap().ratio - ratio).abs() < 1e-9);
    let (nl, d, nr) = split_rects(&frame.content(), m.split.unwrap().ratio);
    assert_eq!(d.x, held.x + held.w, "the divider sits at the pane's right edge");
    assert_eq!(effects, vec![move_bar(frame.bar()), set_frame(3, nr), save(), divider_effect(Some(d))]);
    assert!(nl.close_to(&held));
}

#[test]
fn a_user_resize_of_the_left_pane_from_its_left_edge_moves_the_frame_left_edge() {
    let mut m = split_a1_b3();
    let (l, _, r) = halves();
    let held = Rect::new(l.x + 60.0, l.y, l.w - 60.0, l.h);
    let effects = ev(&mut m, reported(1, held, true));
    let frame = m.frame.unwrap();
    assert_eq!((frame.x, frame.w), (60.0, 1380.0), "left edge moves, the right edge stays");
    assert_eq!(frame.x + frame.w, 1440.0);
    let (_, d, nr) = split_rects(&frame.content(), m.split.unwrap().ratio);
    assert_eq!(d.x, held.x + held.w);
    assert_eq!(d, halves().1, "the divider did not move, so no divider effect");
    assert!(nr.close_to(&r), "the right pane did not move");
    assert_eq!(effects, vec![move_bar(frame.bar()), set_frame(3, nr), save()]);
}

#[test]
fn a_user_resize_of_the_right_pane_sets_the_right_edge_and_the_divider() {
    let mut m = split_a1_b3();
    let (_, d0, r) = halves();
    // The user pulled the right pane's left edge 80 points to the left.
    let held = Rect::new(r.x - 80.0, r.y, r.w + 80.0, r.h);
    let effects = ev(&mut m, reported(3, held, true));
    let frame = m.frame.unwrap();
    assert_eq!((frame.x, frame.w), (0.0, 1440.0), "the left edge stays, the right edge is the pane's");
    let (l, d, nr) = split_rects(&frame.content(), m.split.unwrap().ratio);
    assert_eq!(d.x, held.x - SPLIT_GAP, "the divider sits just left of the pane");
    assert!(d.x < d0.x);
    assert!(nr.close_to(&held));
    assert_eq!(effects, vec![move_bar(frame.bar()), set_frame(1, l), save(), divider_effect(Some(d))]);
}

#[test]
fn a_user_resize_of_the_right_pane_past_the_right_edge_grows_the_frame() {
    let mut m = split_a1_b3();
    let (_, _, r) = halves();
    let held = Rect::new(r.x, r.y, r.w + 200.0, r.h);
    ev(&mut m, reported(3, held, true));
    let frame = m.frame.unwrap();
    assert_eq!((frame.x, frame.w), (0.0, 1640.0));
    assert_eq!(frame.x + frame.w, held.x + held.w);
}

#[test]
fn a_clamped_user_resize_corrects_the_held_window_once_and_never_fights_again() {
    let mut m = split_a1_b3();
    let (l, _, _) = halves();
    // The user squeezes the left pane to 100 points: below the minimum pane.
    let held = Rect::new(l.x, l.y, 100.0, l.h);
    let effects = ev(&mut m, reported(1, held, true));
    let frame = m.frame.unwrap();
    let (pane, d, right) = split_rects(&frame.content(), m.split.unwrap().ratio);
    assert_eq!(pane.w, MIN_PANE);
    assert_eq!(
        effects,
        vec![move_bar(frame.bar()), set_frame(1, pane), set_frame(3, right), save(), divider_effect(Some(d))]
    );
    // The user keeps pulling: no second SetFrame for the held window.
    let held = Rect::new(l.x, l.y, 90.0, l.h);
    let effects = ev(&mut m, reported(1, held, true));
    assert!(!effects.iter().any(|e| matches!(e, Effect::Platform(Command::SetFrame { window, .. }) if *window == WindowId(1))));
    assert_invariants(&m);
}

#[test]
fn a_user_report_at_the_pane_changes_nothing() {
    let mut m = split_a1_b3();
    let (l, _, r) = halves();
    assert_eq!(ev(&mut m, reported(1, l, true)), none());
    assert_eq!(ev(&mut m, reported(3, r, true)), none());
}

#[test]
fn a_frame_report_of_a_window_outside_the_shown_pair_changes_nothing() {
    let mut m = split_a1_b3();
    let before = snapshot(&m);
    assert_eq!(ev(&mut m, reported(4, Rect::new(5.0, 5.0, 100.0, 100.0), true)), none());
    assert_eq!(ev(&mut m, reported(2, Rect::new(5.0, 5.0, 100.0, 100.0), false)), none());
    assert_eq!(snapshot(&m), before);
}

#[test]
fn closing_the_active_member_with_x_shows_the_other_one_alone() {
    let mut m = split_a1_b3();
    let b3 = tab_of(&m, 3);
    let a1 = tab_of(&m, 1);
    assert_eq!(act(&mut m, Action::Close(b3)), vec![close_window(3)]);
    assert!(m.split.is_some(), "the tab stays until the window is gone");
    let effects = closed(&mut m, "b", 3);
    assert_eq!(
        effects,
        vec![
            quit_app("b"),
            unwatch("b"),
            present("a", 1),
            raise(),
            minimize(2),
            hide("c"),
            save(),
            divider_effect(None),
        ]
    );
    assert_eq!(m.split, None);
    assert_eq!(m.active, Some(a1), "the survivor, not the next tab in the bar");
    assert_invariants(&m);
}

#[test]
fn closing_the_non_active_member_in_the_app_shows_the_active_one_alone() {
    let mut m = split_a1_b3();
    // a1 is closed in its app; a has a second window, so only the tab goes.
    let effects = closed(&mut m, "a", 1);
    assert_eq!(
        effects,
        vec![present("b", 3), raise(), hide("a"), hide("c"), save(), divider_effect(None)]
    );
    assert_eq!(m.split, None);
    assert_eq!(m.active, Some(tab_of(&m, 3)));
    assert_invariants(&m);
}

#[test]
fn the_last_window_of_a_member_closed_in_the_app_leaves_an_expiring_tab_and_the_other_alone() {
    let mut m = split_a1_b3();
    let b3 = tab_of(&m, 3);
    let effects = closed(&mut m, "b", 3);
    let a1 = tab_of(&m, 1);
    assert_eq!(
        effects,
        vec![present("a", 1), raise(), minimize(2), hide("b"), hide("c"), expire(b3), save(), divider_effect(None)]
    );
    assert_eq!(m.split, None);
    assert_eq!(m.active, Some(a1));
    assert!(m.is_expiring(b3));
    assert_invariants(&m);
}

#[test]
fn a_member_app_that_stops_leaves_the_other_member_alone() {
    let mut m = split_a1_b3();
    let b3 = tab_of(&m, 3);
    let effects = stopped(&mut m, "b");
    assert_eq!(
        effects,
        vec![present("a", 1), raise(), minimize(2), hide("c"), expire(b3), save(), divider_effect(None)]
    );
    assert_eq!(m.split, None);
    assert_eq!(m.active, Some(tab_of(&m, 1)));
    assert_invariants(&m);
}

#[test]
fn an_app_running_report_without_a_member_window_dissolves_the_split() {
    let mut m = split_a1_b3();
    // b reports another window: b3 is gone, b4 is new and does not join the pair.
    let effects = running(&mut m, "b", vec![win(7, "b7")]);
    assert_eq!(m.split, None);
    assert_eq!(m.active, Some(tab_of(&m, 1)), "the survivor is active");
    assert_eq!(effects[0], present("a", 1));
    assert_eq!(effects.last(), Some(&divider_effect(None)));
    assert_invariants(&m);
}

#[test]
fn both_members_in_one_app_that_stops_dissolve_the_split_with_the_usual_rules() {
    let mut m = split_model();
    split_tab(&mut m, 2);
    assert!(m.split.is_some());
    let a_first = tab_of(&m, 1);
    let effects = stopped(&mut m, "a");
    assert_eq!(m.split, None);
    assert_eq!(m.active, Some(a_first), "the placeholder that replaces the app is active");
    assert_eq!(effects.last(), Some(&divider_effect(None)));
    assert_invariants(&m);
}

#[test]
fn losing_a_member_while_a_third_tab_is_active_changes_nothing_else() {
    let mut m = split_a1_b3();
    activate_window(&mut m, 4);
    let b3 = tab_of(&m, 3);
    let effects = closed(&mut m, "b", 3);
    assert_eq!(effects, vec![expire(b3), save()]);
    assert_eq!(m.split, None);
    assert_eq!(m.active, Some(tab_of(&m, 4)));
}

#[test]
fn removing_a_tab_that_is_not_a_member_keeps_the_split() {
    let mut m = split_a1_b3();
    let c4 = tab_of(&m, 4);
    act(&mut m, Action::Close(c4));
    closed(&mut m, "c", 4);
    assert!(m.split.is_some());
    assert_eq!(m.divider(), Some(halves().1));
}

#[test]
fn an_expiry_of_another_tab_keeps_the_split() {
    let mut m = split_a1_b3();
    let c4 = tab_of(&m, 4);
    closed(&mut m, "c", 4);
    expired(&mut m, c4);
    assert!(m.split.is_some());
}

#[test]
fn quit_shows_every_app_of_a_split() {
    let mut m = split_a1_b3();
    assert_eq!(act(&mut m, Action::Quit), vec![show("a"), show("b"), show("c"), save()]);
}

#[test]
fn the_screen_change_that_gives_the_first_frame_presents_the_pair() {
    let mut m = Model::new(Saved { apps: vec![app("a"), app("b")], frame: None, active_app: None });
    ev(&mut m, Event::TrustChanged(true));
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![win(3, "b3")]);
    let b3 = tab_of(&m, 3);
    let a1 = tab_of(&m, 1);
    act(&mut m, Action::Activate(a1));
    act(&mut m, Action::Split(b3));
    assert_eq!(m.divider(), None, "no frame yet");
    let effects = ev(&mut m, Event::ScreenChanged { visible: screen() });
    let (l, d, r) = halves();
    assert_eq!(effects[1], present_in("a", 1, l));
    assert_eq!(effects[2], present_in("b", 3, r));
    assert_eq!(effects.last(), Some(&divider_effect(Some(d))));
}

#[test]
fn closing_by_x_the_window_of_a_split_target_forgets_the_mark_when_it_is_split() {
    // The user cancelled a close, then used the window in a split: the mark is gone.
    let mut m = split_model();
    let b3 = tab_of(&m, 3);
    act(&mut m, Action::Close(b3));
    split_tab(&mut m, 3);
    let effects = closed(&mut m, "b", 3);
    // Not closed by Oriel any more: the user closed the app's last window, so b3 expires.
    assert!(effects.contains(&expire(b3)));
    assert!(!effects.contains(&quit_app("b")));
}

#[test]
fn can_split_with_active_needs_two_different_window_tabs() {
    let mut m = model_with(&["a", "b", "c"]);
    running(&mut m, "a", vec![win(1, "a1")]);
    running(&mut m, "b", vec![]);
    running(&mut m, "c", vec![win(4, "c4")]);
    let (a1, b, c4) = (tab_of(&m, 1), app_tab(&m, "b"), tab_of(&m, 4));
    assert!(!m.can_split_with_active(a1), "the active tab itself");
    assert!(!m.can_split_with_active(b), "a placeholder");
    assert!(!m.can_split_with_active(TabId(999)), "an unknown tab");
    assert!(m.can_split_with_active(c4));
    activate_app(&mut m, "b");
    assert!(!m.can_split_with_active(c4), "the active tab is a placeholder");
}
