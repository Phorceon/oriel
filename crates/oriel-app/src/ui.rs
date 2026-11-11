//! The windows' state. Both windows build their [`Ui`] from the same [`Core`]; the `role`
//! tells the view, the keys and the poll which job this window has.

use crate::core::Core;
use crate::{bar, geometry, picker};
use oriel_core::model::{Action, Input};
use oriel_core::{AppInfo, TabId};
use rune::{AppHandle, Cx, El, KeyEvent, TextEdit, TextStyle, Tx, Waker, text_input};
use std::collections::HashSet;
use std::sync::Arc;

/// How long a picker focus loss must stand before the picker closes (seconds).
const FOCUS_LOSS_GRACE: f64 = 0.35;

/// Which window this state belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    /// The 40 pt tab strip.
    Bar,
    /// The app picker the "+" button opens.
    Picker,
}

/// A press on a tab, until it is released: where it began, and where the tab would drop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabDrag {
    pub id: TabId,
    pub from: usize,
    pub to: usize,
    /// Pointer travel since the press, in points.
    pub dx: f32,
    /// The press travelled far enough to be a drag (else it stays a click).
    pub moved: bool,
}

impl TabDrag {
    /// How far the dragged tab is drawn from its preview slot, so it stays under the pointer.
    pub fn offset(&self, width: f32) -> f32 {
        (self.from as f32 - self.to as f32) * width + self.dx
    }
}

/// One window's state: its role, the app it shows, and its own UI-only state.
pub(crate) struct Ui {
    pub(crate) role: Role,
    pub(crate) core: Arc<Core>,
    /// The bar only: the picker's handle, to open and reset it.
    pub(crate) picker: Option<AppHandle<Ui>>,
    /// The picker only: the search field's editor.
    pub(crate) query: TextEdit,
    /// The picker only: which match is highlighted.
    pub(crate) picked: usize,
    /// The picker only: the running apps when it opened (for the "Running" marks).
    pub(crate) running: HashSet<String>,
    /// The picker only: how many times it opened (a new root each time, so it animates in).
    pub(crate) opened: u32,
    /// The bar only: its laid-out width, as the layout reported it (0 before the first).
    pub(crate) bar_w: f32,
    /// The bar only: the "+" button's left edge in the window.
    pub(crate) plus_x: f32,
    /// The bar only: the tab the pointer is over, whose close button then shows.
    pub(crate) hover_tab: Option<TabId>,
    /// The bar only: the tab press in progress (a click or a reorder drag).
    pub(crate) tab_drag: Option<TabDrag>,
    /// The bar only: a pointer press on a tab acted already; its `on_press` must not again.
    pub(crate) pointer_pressed: bool,
    /// The picker only: when it last lost focus, until a gain or the close.
    pub(crate) lost_at: Option<f64>,
    /// The bar only: env-var debug hooks (`debug.rs`).
    pub(crate) debug: crate::debug::Hooks,
}

impl Ui {
    fn new(role: Role, core: Arc<Core>, picker: Option<AppHandle<Ui>>) -> Ui {
        Ui {
            role,
            core,
            picker,
            query: TextEdit::new(""),
            picked: 0,
            running: HashSet::new(),
            opened: 0,
            bar_w: 0.0,
            plus_x: 0.0,
            hover_tab: None,
            tab_drag: None,
            pointer_pressed: false,
            lost_at: None,
            debug: if role == Role::Bar { crate::debug::Hooks::from_env() } else { Default::default() },
        }
    }

    pub(crate) fn bar(core: Arc<Core>, picker: AppHandle<Ui>) -> Ui {
        Ui::new(Role::Bar, core, Some(picker))
    }

    pub(crate) fn picker(core: Arc<Core>) -> Ui {
        Ui::new(Role::Picker, core, None)
    }

    // ---- `AppSpec` hooks -----------------------------------------------------

    /// `AppSpec::setup`: connect this window's waker; whichever window comes up first
    /// starts the platform (on the main thread, once).
    pub(crate) fn setup(s: &mut Ui, waker: Waker) {
        crate::debug::mark(&format!("setup {:?}", s.role));
        s.core.add_waker(waker);
        let _ = s.core.icons();
        s.core.start_platform();
    }

    /// `AppSpec::poll`: seeds the demo once, tracks the bar's frame for moves Oriel did not
    /// make, shows the bar at launch, and writes anything the save gate owes. Returns "state
    /// the view reads changed".
    pub(crate) fn poll(s: &mut Ui, now: f64, tx: &mut Tx) -> bool {
        s.core.observe_clock(now);
        let seeded = s.core.seed_demo(now);
        let mut changed = seeded;
        if s.debug.scene_pending(seeded) {
            changed |= s.apply_demo_scene();
        }
        match s.role {
            Role::Bar => {
                changed |= s.core.track_bar(now);
                changed |= s.core.tick_expiry(now, tx);
                changed |= crate::debug::tick(s, now, tx);
                s.core.tick_reveal(now, tx);
                s.core.sync_placement(now, tx);
            }
            Role::Picker => s.close_if_lost(now, tx),
        }
        changed | s.core.flush_saves(now, tx)
    }

    /// `AppSpec::on_key`: the bar takes ⌘Q as quit.
    pub(crate) fn on_key(s: &mut Ui, event: &KeyEvent, tx: &mut Tx) -> bool {
        if let Role::Bar = s.role
            && event.is_command('q')
        {
            s.quit(tx);
            return true;
        }
        false
    }

    /// `App::on_focus`: the picker closes when it loses focus (a click outside, another
    /// app activating). A loss is not acted on at once: opening the picker activates
    /// Oriel, and AppKit then reports `false, true` around the first key change (the
    /// window was seen closing itself right after it opened). The loss is noted here and
    /// `Ui::poll` closes the window if no gain follows within [`FOCUS_LOSS_GRACE`].
    pub(crate) fn on_focus(s: &mut Ui, focused: bool, tx: &mut Tx) {
        if let Role::Picker = s.role {
            if focused {
                s.lost_at = None;
            } else {
                let now = tx.clock();
                s.lost_at = Some(now);
                tx.wake_at(now + FOCUS_LOSS_GRACE);
            }
        }
    }

    /// The picker only: close once a focus loss has stood for the grace period.
    fn close_if_lost(&mut self, now: f64, tx: &mut Tx) {
        let Some(lost) = self.lost_at else { return };
        if now >= lost + FOCUS_LOSS_GRACE {
            self.lost_at = None;
            tx.close_window();
        } else {
            tx.wake_at(lost + FOCUS_LOSS_GRACE);
        }
    }

    /// `App::on_close`: the bar closing is Oriel quitting.
    pub(crate) fn on_close(s: &mut Ui, tx: &mut Tx) {
        if let Role::Bar = s.role {
            s.quit(tx);
        }
    }

    /// `App::on_quit`: runs when a quit the app asked for begins; idempotent.
    pub(crate) fn on_quit(s: &mut Ui, tx: &mut Tx) {
        s.quit(tx);
    }

    /// Perform `Action::Quit`'s effects and end the loop (see [`Core::quit`]).
    pub(crate) fn quit(&self, tx: &mut Tx) {
        self.core.quit(tx);
    }

    pub(crate) fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
        match s.role {
            Role::Bar => bar::view(s, cx),
            Role::Picker => picker::view(s, cx),
        }
    }

    // ---- The picker -----------------------------------------------------------

    /// The "+" button: reset the picker's UI state, ask the model to scan, put the picker
    /// under the "+" button and open it.
    pub(crate) fn open_picker(&self, tx: &mut Tx) {
        let Some(picker) = &self.picker else { return };
        self.core.dispatch(Input::Action(Action::OpenPicker), tx.clock());
        let running = self.core.running_bundle_ids();
        picker.update(move |ui: &mut Ui, _tx: &mut Tx| {
            ui.picker_reset();
            ui.running = running;
            ui.opened += 1;
        });
        self.core.place_picker(self.plus_x);
        picker.open();
    }

    /// A fresh search: empty, focused, first match highlighted.
    pub(crate) fn picker_reset(&mut self) {
        self.picked = 0;
        self.query.clear();
        self.query.focused = true;
    }

    /// The current matches for the picker's query, in the order the list shows them
    /// (recomputed at press time, never carried by an index from build time). With no query,
    /// apps that run (and have no tab yet) come first.
    pub(crate) fn hits(&self) -> Vec<AppInfo> {
        self.core.with_model(|m| {
            let mut hits: Vec<AppInfo> = geometry::matches_query(&m.installed, &self.query.text).into_iter().cloned().collect();
            if self.query.text.trim().is_empty() {
                let in_bar = |a: &AppInfo| m.tabs.iter().any(|t| t.app.bundle_id == a.bundle_id);
                hits.sort_by_key(|a| !(self.running.contains(&a.bundle_id) && !in_bar(a)));
            }
            hits
        })
    }

    // ---- Scenes (headless runs) -------------------------------------------------

    /// After the demo is seeded and its windows reported: the UI state a scene asked for.
    /// True once applied.
    fn apply_demo_scene(&mut self) -> bool {
        use std::sync::atomic::Ordering;
        // The demo's window events arrive a turn after the seeding: wait for its tabs.
        let tabs: Vec<TabId> = self.core.with_model(|m| m.tabs.iter().map(|t| t.id).collect());
        if tabs.len() < 5 {
            return false;
        }
        self.debug.scene_applied();
        if self.role == Role::Picker {
            self.running = self.core.running_bundle_ids();
            return true;
        }
        if self.core.demo_hover.load(Ordering::Acquire)
            && let Some(id) = tabs.get(2)
        {
            self.hover_tab = Some(*id);
        }
        // The first tab is active; Notes (third tab) joins it. `split-away` then leaves the
        // pair for the second Safari window, so the pair shows as joined but not shown.
        if self.core.demo_split.load(Ordering::Acquire) || self.core.demo_split_away.load(Ordering::Acquire) {
            let now = self.core.clock();
            self.core.dispatch(Input::Action(Action::Split(tabs[2])), now);
            if self.core.demo_split_away.load(Ordering::Acquire) {
                self.core.dispatch(Input::Action(Action::Activate(tabs[1])), now);
            }
        }
        if self.core.demo_reorder.load(Ordering::Acquire)
            && let Some(id) = tabs.get(2)
        {
            self.tab_drag = Some(TabDrag { id: *id, from: 2, to: 3, dx: 150.0, moved: true });
        }
        true
    }

    /// `--scene not-trusted`: the bar asks for Accessibility.
    pub(crate) fn scene_untrusted(s: &mut Ui) {
        s.core.demo_untrusted();
    }

    /// `--scene many`: twenty tabs (the bar at its most crowded).
    pub(crate) fn scene_many(s: &mut Ui) {
        s.core.demo_many();
    }

    /// `--scene hover`: the pointer rests on the third tab.
    pub(crate) fn scene_hover(s: &mut Ui) {
        s.core.demo_hover();
    }

    /// `--scene split`: the first and the third tab are shown side by side.
    pub(crate) fn scene_split(s: &mut Ui) {
        s.core.demo_split(false);
    }

    /// `--scene split-away`: the same pair, with the second tab active.
    pub(crate) fn scene_split_away(s: &mut Ui) {
        s.core.demo_split(true);
    }

    /// `--scene reorder`: a tab is being dragged one place to the right.
    pub(crate) fn scene_reorder(s: &mut Ui) {
        s.core.demo_reorder();
    }

    /// `--scene query`: the picker searches.
    pub(crate) fn scene_query(s: &mut Ui) {
        s.query.replace_text("cal");
        s.query.focused = true;
    }

    /// `--scene empty`: the picker finds nothing.
    pub(crate) fn scene_empty(s: &mut Ui) {
        s.query.replace_text("zebra");
        s.query.focused = true;
    }

    /// `--scene loading`: the picker waits for the app scan.
    pub(crate) fn scene_loading(s: &mut Ui) {
        s.core.demo_no_scan();
    }
}

/// The picker's search field with its key handling: Up/Down pick (wrapping), Escape closes,
/// Enter adds the highlighted app.
pub(crate) fn search_field(s: &Ui, color: rune::Color, placeholder: rune::Color) -> El {
    text_input(&s.query, TextStyle::new(17.0, 400.0).lh(24.0))
        .key("search")
        .role("searchbox")
        .label("Search apps")
        .single_line()
        .grow(1.0)
        .color(color)
        .placeholder("Add an app", placeholder)
        .on_edit(|s: &mut Ui| &mut s.query)
        .on_input_change(|s: &mut Ui, _tx| {
            s.picked = 0;
        })
        .on_input_key(|s: &mut Ui, event: &KeyEvent, tx: &mut Tx| match event.key {
            rune::Key::Escape => {
                s.picker_reset();
                tx.close_window();
                true
            }
            rune::Key::Down | rune::Key::Up => {
                let count = s.hits().len();
                if count > 0 {
                    let step = if matches!(event.key, rune::Key::Down) { 1 } else { count - 1 };
                    s.picked = (s.picked + step) % count;
                    if let Some(app) = s.hits().get(s.picked) {
                        tx.scroll_into_view(row_key(app));
                    }
                }
                true
            }
            _ => false,
        })
        .on_submit(|s: &mut Ui, tx: &mut Tx| {
            picker::submit(s, tx);
        })
}

/// The key an app's virtual row is addressed by.
pub(crate) fn row_key(app: &AppInfo) -> String {
    format!("app-{}", app.bundle_id)
}
