//! Oriel: a tab bar the platform glues real app windows under. The bar is Oriel's own
//! window; the tabs' windows belong to their apps and follow it.
//!
//! Fake runs: `cargo run -p oriel-app --no-default-features`, or `ORIEL_FAKE=1` with the
//! default features. Nothing real happens — a `FakePlatform` answers, a demo seeds tabs
//! and the picker, and no state file outside a temp folder is touched.

mod bar;
mod core;
mod debug;
mod geometry;
mod picker;
mod style;
mod ui;

#[cfg(target_os = "macos")]
mod macwin;

/// Off macOS the AppKit bridge is two no-ops (`macwin.rs` explains what they carry).
#[cfg(not(target_os = "macos"))]
mod macwin {
    use oriel_core::Rect;

    pub(crate) fn watch_moves(_window: &rune::WindowHandle, _on_move: Box<dyn Fn(Rect)>) -> bool {
        false
    }
    pub(crate) fn set_bar_frame(_window: &rune::WindowHandle, _rect: Rect) -> bool {
        false
    }
    pub(crate) fn pointer() -> Option<(f64, f64)> {
        None
    }
    pub(crate) fn start_drag_monitor(_on_move: Box<dyn Fn()>, _on_up: Box<dyn Fn()>) -> bool {
        false
    }
    pub(crate) fn stop_drag_monitor() {}
    pub(crate) fn set_bar_alpha(_window: &rune::WindowHandle, _alpha: f64, _fade: f64) -> bool {
        false
    }
    pub(crate) fn every_frame(_step: Box<dyn Fn() -> bool>) {}
    pub(crate) fn no_appear_animation(_window: &rune::WindowHandle) {}
    pub(crate) fn hide_in_mission_control(_window: &rune::WindowHandle) -> Option<usize> {
        None
    }
    pub(crate) fn bar_collection_behavior(_window: &rune::WindowHandle) -> Option<usize> {
        None
    }
    pub(crate) fn window_backgrounds() -> Option<([f64; 4], [f64; 4])> {
        None
    }
    pub(crate) fn running_bundle_ids() -> Vec<String> {
        Vec::new()
    }
    pub(crate) fn raise_bar(_window: &rune::WindowHandle, _above: &[u32]) -> bool {
        false
    }
    pub(crate) fn route_menu_quit(_quit: Box<dyn Fn()>) -> bool {
        false
    }
    pub(crate) fn is_dark() -> bool {
        false
    }
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) struct DividerLook {
        pub gap: [f64; 4],
        pub line: [f64; 4],
        pub line_width: f64,
    }
    pub(crate) struct DividerHooks {
        pub hover: Box<dyn Fn(bool)>,
        pub press: Box<dyn Fn()>,
    }
    pub(crate) fn divider_install(_bar: &rune::WindowHandle, _hooks: DividerHooks) -> bool {
        false
    }
    pub(crate) fn divider_set(_bar: &rune::WindowHandle, _rect: Option<Rect>) {}
    pub(crate) fn divider_look(_look: DividerLook) {}
    pub(crate) fn divider_raise(_bar: &rune::WindowHandle) {}
    pub(crate) fn divider_cursor() {}
    pub(crate) fn divider_debug(_bar: &rune::WindowHandle) -> Option<String> {
        None
    }
}

use crate::core::Core;
use crate::ui::Ui;
use oriel_core::{store, FakePlatform, Model, BAR_HEIGHT};
use rune::{App, AppSpec, WindowOptions, WindowSpec};
use std::sync::Arc;

/// Whether this run uses `FakePlatform`: the `mac` feature is off, or `ORIEL_FAKE` is set
/// and non-empty.
fn fake_run() -> bool {
    if !cfg!(feature = "mac") {
        return true;
    }
    std::env::var("ORIEL_FAKE").map(|value| !value.trim().is_empty()).unwrap_or(false)
}

fn main() {
    debug::mark("main");
    // The saved state — never the real path in a fake run.
    let path = if fake_run() {
        std::env::temp_dir().join("oriel-fake-state.json")
    } else {
        store::default_path()
    };
    // A fake run starts empty (the demo seeds it), so a headless render never depends on
    // what an earlier run saved.
    let saved = if fake_run() { oriel_core::Saved::default() } else { store::load(&path) };
    let model = Model::new(saved);

    let core = if fake_run() {
        let platform = Arc::new(FakePlatform::new());
        Core::new(model, path, platform.clone(), Some(platform))
    } else {
        #[cfg(feature = "mac")]
        {
            let platform: Arc<dyn oriel_core::Platform> = Arc::new(oriel_mac::MacPlatform::new());
            Core::new(model, path, platform, None)
        }
        #[cfg(not(feature = "mac"))]
        {
            unreachable!("a build without the mac feature always takes the fake run")
        }
    };
    run(core);
}

/// The bar window and the picker window the "+" button opens.
fn run(core: Arc<Core>) {
    rune::run_windows(move |_displays| {
        // The picker: hidden until the "+" opens it, its own retained state.
        let core_for_picker = core.clone();
        let picker_size = style::PICKER_SIZE;
        let picker_app = App::new(AppSpec {
            title: "Oriel — add an app",
            size: picker_size,
            min_size: Some(picker_size),
            init: unreachable_init,
            view: Ui::view,
            scenes: &[("query", Ui::scene_query), ("empty", Ui::scene_empty), ("loading", Ui::scene_loading)],
            setup: Some(Ui::setup),
            poll: Some(Ui::poll),
            on_key: None,
            ..spec_defaults()
        })
        // No window at launch. `WindowOptions::visible` alone does not hide a secondary
        // window: Rune's host opens every non-`windowless` app's window when its loop
        // starts. The "+" button opens it; Esc, a pick and losing focus close it.
        .windowless()
        .on_focus(Ui::on_focus)
        .init(move || Ui::picker(core_for_picker.clone()));
        let picker_options = WindowOptions {
            visible: false,
            transparent: true,
            borderless: true,
            resizable: false,
            min_size: Some(picker_size),
            ..Default::default()
        }
        .macos(rune::MacWindowOptions::default().corner_radius(style::PICKER_RADIUS));
        let picker = WindowSpec::from_app("picker", picker_app).configured_options(picker_options);

        // The bar: a regular app with a Dock icon, standard menus, and its own strip in
        // place of a title bar.
        let mut bar_app = App::new(AppSpec {
            title: "Oriel",
            size: (900.0, BAR_HEIGHT as f32),
            min_size: Some((44.0, BAR_HEIGHT as f32)),
            init: unreachable_init,
            view: Ui::view,
            scenes: &[
                ("not-trusted", Ui::scene_untrusted),
                ("many", Ui::scene_many),
                ("hover", Ui::scene_hover),
                ("reorder", Ui::scene_reorder),
                ("split", Ui::scene_split),
                ("split-away", Ui::scene_split_away),
            ],
            setup: Some(Ui::setup),
            poll: Some(Ui::poll),
            on_key: Some(Ui::on_key),
            ..spec_defaults()
        })
        .standard_menus()
        .on_close(Ui::on_close)
        .on_quit(Ui::on_quit);
        // The picker joins the bar's app; closing the bar closes Oriel, closing the picker
        // only hides it (a secondary window keeps the loop alive).
        // The bar's handle goes first: `Core` shows and routes through `apps.first()`.
        core.add_app_handle(bar_app.handle());
        let (picker_handle, picker_window) = bar_app.window(picker);
        core.add_app_handle(picker_handle.clone());
        core.set_picker_window(picker_window);

        let core_for_state = core.clone();
        let bar_app = bar_app.init(move || {
            let core = core_for_state.clone();
            let picker = picker_handle.clone();
            Ui::bar(core, picker)
        });

        // Hidden at creation: `Core::bind_bar_window` places it, sets alpha 0 and no system
        // zoom-in, then shows it; `Core::tick_reveal` fades it in with the first window.
        let bar_options = WindowOptions {
            visible: false,
            transparent: true,
            borderless: true,
            resizable: false,
            min_size: Some((44.0, BAR_HEIGHT as f32)),
            ..Default::default()
        }
        // No system rounding and no system shadow: the bar draws its own top corners and
        // outline (`bar::shell`), and a shadow would fall on the glued window's title bar,
        // which must read as the same window.
        .macos(rune::MacWindowOptions::default().shadow(false));

        let core_for_created = core.clone();
        let bar = WindowSpec::from_app("bar", bar_app)
            .configured_options(bar_options)
            .on_created(move |window| core_for_created.bind_bar_window(&window));

        vec![bar.boxed()]
    });
}

/// `AppSpec::init` takes a function pointer; the real init closures capture state, so this
/// placeholder stands in the struct literal and `App::init` replaces it before it runs.
fn unreachable_init() -> Ui {
    unreachable!("App::init replaces this placeholder")
}

/// The `AppSpec` fields the literals above do not name.
fn spec_defaults() -> AppSpec<Ui> {
    AppSpec::new("Oriel", (900.0, BAR_HEIGHT as f32), unreachable_init, Ui::view)
}
