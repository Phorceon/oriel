//! The bar: Oriel's only always-visible surface, drawn as the title area of the window glued
//! under it. Chrome-like tabs, a "+" button, and the empty space that drags the window.
//!
//! - The active tab is one shape that slides between tabs (a layout spring) and runs into the
//!   window below through two concave "wings"; its color is the system window background.
//! - Inactive tabs show a soft pill under the pointer; thin separators sit between inactive
//!   tabs only.
//! - A press on a tab activates it at once (on the press, as Chrome does); dragging it
//!   reorders, with the other tabs making room live; a middle click closes it.
//! - Split view: the two tabs of a shown split both read as active (the one with the focus is
//!   the full active shape, the other a softer body without wings); a member of a split, shown
//!   or not, carries a small split glyph. A right click on a tab opens a native menu (Rune's
//!   `popup_menu`, an `NSMenu`: the bar is only 40 pt high, so a menu drawn inside the window
//!   would be clipped): "Split with Current Tab", "End Split View", "Close Tab".
//! - A press on empty space drags the whole Oriel frame (`Core::begin_bar_drag`); a double
//!   click zooms it.

use crate::geometry::{self, NEW_TAB, STRIP_PAD, TAB_WING};
use crate::style::{BAR_RADIUS, Palette, TAB_RADIUS, TAB_TOP};
use crate::ui::{TabDrag, Ui};
use oriel_core::model::{Action, Input};
use oriel_core::{BAR_HEIGHT, Model, Tab, TabId};
use rune::tok::motion;
use rune::{
    Color, Cx, Drag, El, LineCap, LoopSpec, Phase, PointerButton, PointerPress, PopupItem, PopupMenu, Shape, TextStyle,
    Transition, Tx, canvas, col, el, row, spacer, text,
};

const BAR_H: f32 = BAR_HEIGHT as f32;
/// The active tab's height: from its top inset to the bar's bottom edge.
const TAB_H: f32 = BAR_H - TAB_TOP;
const LABEL: TextStyle = TextStyle::new(12.0, 450.0);
const LABEL_ACTIVE: TextStyle = TextStyle::new(12.0, 560.0);
/// The label of the shown tab of a split that does not have the focus.
const LABEL_PAIR: TextStyle = TextStyle::new(12.0, 510.0);

/// The bar's view: tabs, "+", drag space — or the Accessibility request.
pub(crate) fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let dark = cx.system_dark();
    let p = s.core.palette(dark);
    s.core.with_model(|model| {
        if !model.trusted {
            return needs_access(model, &p);
        }
        // The width the layout gave the bar; before its first report, the model's frame.
        let bar_w = if s.bar_w > 0.0 { f64::from(s.bar_w) } else { model.frame.map_or(900.0, |f| f.w) };
        let strip = tab_strip(s, model, bar_w, &p);
        shell(&p)
            .child(strip)
            .child(new_tab(&p))
            // Everything the tabs leave: dragging here moves the window, at full height.
            .child(drag_area(spacer().key("drag-area").grow(1.0).min_w(geometry::DRAG_MIN as f32).h_full()))
    })
}

/// The bar's frame: the strip color, rounded top corners, square bottom (it meets the glued
/// window), a hairline on the top and the sides as a window's edge has.
fn shell(p: &Palette) -> El {
    row()
        .key("bar")
        .role("toolbar")
        .label("Oriel")
        .w_full()
        .h_full()
        .fill(p.strip)
        .corners([BAR_RADIUS, BAR_RADIUS, 0.0, 0.0])
        .continuous_corners()
        .border_sides([1.0, 1.0, 0.0, 1.0], p.hairline)
        .on_layout(|s: &mut Ui, rect: [f32; 4]| s.bar_w = rect[2])
        .hover_motion_within(motion::INSTANT, motion::SNAP_EXIT)
}

/// The tabs, in the order a reorder drag previews, over the sliding active shape.
fn tab_strip(s: &Ui, model: &Model, bar_w: f64, p: &Palette) -> El {
    let count = model.tabs.len();
    let width = geometry::tab_width(bar_w, count) as f32;
    // A reorder drag shows the tabs in their new order while the pointer moves.
    let order: Vec<usize> = match s.tab_drag {
        Some(d) if d.moved => geometry::moved(&(0..count).collect::<Vec<_>>(), d.from, d.to),
        _ => (0..count).collect(),
    };
    let active_slot = order.iter().position(|&i| Some(model.tabs[i].id) == model.active);
    let dragging = s.tab_drag.filter(|d| d.moved);
    // The other tab of a shown split, which reads as shown too.
    let partner = partner_of_active(model);
    let partner_slot = partner.and_then(|id| order.iter().position(|&i| model.tabs[i].id == id));

    let mut tabs = row().key("tabs").role("tablist").label("Tabs").h_full();
    for (slot, &i) in order.iter().enumerate() {
        let tab = &model.tabs[i];
        let prev = slot.checked_sub(1).map(|k| &model.tabs[order[k]]);
        let lifted = dragging.is_some_and(|d| d.id == tab.id);
        tabs = tabs.child(tab_el(s, model, tab, i, prev, partner, width, lifted, p));
    }
    let total = width * count as f32;
    let room = geometry::tab_room(bar_w) as f32;

    // The active shape under the tabs, at its slot (following the pointer while dragged).
    let mut layer = el().key("strip").h_full().w(total + 2.0 * TAB_WING as f32);
    if let (Some(slot), Some(id)) = (partner_slot, partner) {
        let lift = dragging.filter(|d| d.id == id).map_or(0.0, |d| d.offset(width));
        let mut shape = pair_shape(width, p).absolute(slot as f32 * width + lift, TAB_TOP);
        if dragging.is_some() {
            shape = shape.instant_changes();
        }
        layer = layer.child(shape);
    }
    if let Some(slot) = active_slot {
        let lift = dragging.filter(|d| Some(d.id) == model.active).map_or(0.0, |d| d.offset(width));
        let mut shape = active_shape(width, p).absolute(slot as f32 * width + lift, TAB_TOP);
        if dragging.is_some() {
            shape = shape.instant_changes();
        }
        layer = layer.child(shape);
    }
    let layer = layer.child(tabs.absolute(TAB_WING as f32, 0.0));

    // The strip clips whatever the width cannot hold; with too many tabs it scrolls sideways.
    let viewport = el().key("tab-viewport").h_full().w(room.min(total + 2.0 * TAB_WING as f32)).shrink();
    let viewport = if geometry::tabs_overflow(bar_w, count) {
        viewport.scroll_x().scroll_edge_fade(20.0).child(layer)
    } else {
        viewport.clip_rounded(0.0).child(layer)
    };
    el().key("tab-area").h_full().pl((STRIP_PAD - TAB_WING) as f32).child(viewport)
}

/// The active tab's shape: rounded top corners, and at the bottom a concave curve on each
/// side that flows into the window under the bar (as a browser's active tab meets its page).
fn active_shape(width: f32, p: &Palette) -> El {
    let wing = TAB_WING as f32;
    // A wing is a square of the tab's color, partly covered by a square of the strip's color
    // whose one rounded corner draws the concave curve.
    let wing_el = |left: bool| {
        let cover = el().w_full().h_full().fill(p.strip).corners(if left { [0.0, 0.0, wing, 0.0] } else { [0.0, 0.0, 0.0, wing] });
        el().absolute(if left { 0.0 } else { wing + width }, TAB_H - wing).size(wing, wing).fill(p.tab).child(cover)
    };
    let body = el()
        .absolute(wing, 0.0)
        .size(width, TAB_H)
        .fill(p.tab)
        .corners([TAB_RADIUS, TAB_RADIUS, 0.0, 0.0])
        .continuous_corners();
    el()
        .key("active-tab")
        .size(width + 2.0 * wing, TAB_H)
        .motion(motion::SNAP)
        .child(wing_el(true))
        .child(wing_el(false))
        .child(body)
}

/// The other tab of the shown split: the tab that is not active but shown beside it.
fn partner_of_active(model: &Model) -> Option<TabId> {
    let split = model.shown_split()?;
    split.partner(model.active?)
}

/// The shape of the shown tab that does not have the focus: the same top corners as the active
/// tab, a softer color, no wings (it does not run into the window below as the active one does).
fn pair_shape(width: f32, p: &Palette) -> El {
    let wing = TAB_WING as f32;
    el().key("pair-tab")
        .size(width + 2.0 * wing, TAB_H)
        .motion(motion::SNAP)
        .child(
            el().absolute(wing, 0.0)
                .size(width, TAB_H)
                .fill(p.tab_pair)
                .corners([TAB_RADIUS, TAB_RADIUS, 0.0, 0.0])
                .continuous_corners(),
        )
}

/// A small split mark: two panes side by side.
fn split_glyph(ink: Color) -> El {
    let pane = || el().size(5.0, 9.0).radius(1.5).fill(ink);
    row().key("split-glyph").gap(2.0).center_y().child(pane()).child(pane())
}

/// One tab, clipped to its own width: icon, the label cut with an ellipsis in the space left,
/// and a close button fully inside the tab.
#[allow(clippy::too_many_arguments)]
fn tab_el(
    s: &Ui,
    model: &Model,
    tab: &Tab,
    index: usize,
    prev: Option<&Tab>,
    partner: Option<TabId>,
    width: f32,
    lifted: bool,
    p: &Palette,
) -> El {
    let id = tab.id;
    let active = Some(id) == model.active;
    // The shown tab beside the active one reads as shown too (but softer).
    let paired = partner == Some(id);
    let lit = active || paired;
    let joined = model.in_split(id);
    let hovered = s.hover_tab == Some(id) && s.tab_drag.is_none();
    let running = model.is_running(&tab.app.bundle_id);
    let placeholder = tab.window.is_none();
    // Activating a placeholder launches (or reopens) its app: it shows as loading.
    // A closed tab in its grace (expiring) is not launching: it looks dormant, running or not.
    let expiring = model.is_expiring(id);
    let loading = placeholder && active && !expiring;
    let dormant = placeholder && (expiring || (!running && !active));
    let parts = geometry::tab_parts(f64::from(width), lit, hovered);
    let key = format!("tab-{}", tab_slug(tab));
    let name = tab.label().to_string();

    // The soft pill under the pointer, for inactive tabs.
    let pill = el()
        .absolute(4.0, TAB_TOP + 3.0)
        .size((width - 8.0).max(0.0), TAB_H - 9.0)
        .radius(8.0)
        .continuous_corners()
        .fill(if lit || lifted { Color::TRANSPARENT } else { p.hover })
        .group_opacity(0.0, 1.0);
    // The line before this tab, between two inactive tabs that the pointer is not on.
    let quiet = lit
        || hovered
        || prev.is_none_or(|t| Some(t.id) == model.active || partner == Some(t.id) || s.hover_tab == Some(t.id));
    let separator = el()
        .absolute(0.0, TAB_TOP + 9.0)
        .size(1.0, TAB_H - 21.0)
        .fill(if quiet || prev.is_none() || s.tab_drag.is_some() { Color::TRANSPARENT } else { p.separator });

    let ink = if lit {
        p.text
    } else if dormant {
        p.text_dormant
    } else {
        p.text_secondary
    };
    let mut content = row().absolute(0.0, TAB_TOP).size(width, TAB_H).center_y().gap(7.0);
    content = if parts.label || (parts.icon && parts.close) {
        content.pl(12.0).pr(if parts.close { 6.0 } else { 12.0 })
    } else {
        content.center()
    };
    if parts.icon {
        content = content.child(icon(s, tab, &key, p, dormant, loading));
    }
    if joined && parts.label {
        content = content.child(split_glyph(if lit { p.text_secondary } else { p.text_dormant }));
    }
    if parts.label {
        content = content.child(
            text(name.clone())
                .key(format!("{key}-label"))
                .font(if active {
                    LABEL_ACTIVE
                } else if paired {
                    LABEL_PAIR
                } else {
                    LABEL
                })
                .color(ink)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        );
    } else if parts.icon && parts.close {
        content = content.child(spacer().grow(1.0));
    }
    if parts.close {
        content = content.child(close_button(&key, &name, id, lit, p));
    }

    let mut el = el()
        .key(key)
        .role("tab")
        .label(name)
        .checked(active)
        .w(width)
        .h(BAR_H)
        .hover_group()
        .pointer_buttons([PointerButton::Middle])
        .transition(Transition::opacity().combined(Transition::scale(0.92)))
        .motion(motion::SNAP)
        .child(pill)
        .child(separator)
        .child(content)
        .on_hover_change(move |s: &mut Ui, inside, _tx: &mut Tx| {
            if inside {
                s.hover_tab = Some(id);
            } else if s.hover_tab == Some(id) {
                s.hover_tab = None;
            }
        })
        .on_drag(move |s: &mut Ui, d: &mut Drag, tx: &mut Tx| tab_drag(s, id, index, width, d, tx))
        .on_secondary_press(move |s: &mut Ui, press: PointerPress, tx: &mut Tx| tab_menu(s, id, press, tx))
        // The keyboard (Space, Return) presses here; a pointer press already acted on Down.
        .on_press_tx(move |s: &mut Ui, tx: &mut Tx| {
            if !std::mem::take(&mut s.pointer_pressed) {
                s.core.dispatch(Input::Action(Action::Activate(id)), tx.clock());
            }
        });
    if lifted {
        // Under the pointer, at once; the others make room on their springs.
        el = el.offset(s.tab_drag.map_or(0.0, |d| d.offset(width)), 0.0).instant_changes();
    }
    el
}

/// A right click on a tab: a native menu at the pointer. Which items are enabled depends on the
/// model when the menu opens; the pick runs the action then.
fn tab_menu(s: &mut Ui, id: TabId, press: PointerPress, tx: &mut Tx) {
    let (can_split, joined) = s.core.with_model(|m| (m.can_split_with_active(id), m.in_split(id)));
    let mut menu = PopupMenu::new().item(PopupItem::new("split", "Split with Current Tab").enabled(can_split));
    if joined {
        menu = menu.item(PopupItem::new("unsplit", "End Split View"));
    }
    let menu = menu.separator().item(PopupItem::new("close", "Close Tab"));
    tx.popup_menu(menu, press, move |s: &mut Ui, picked, tx: &mut Tx| {
        let action = match picked {
            Some("split") => Action::Split(id),
            Some("unsplit") => Action::Unsplit,
            Some("close") => Action::Close(id),
            _ => return,
        };
        s.core.dispatch(Input::Action(action), tx.clock());
    });
}

/// A press on a tab: activate on Down (no wait for the release), reorder while it moves, and
/// a middle click closes it.
fn tab_drag(s: &mut Ui, id: TabId, index: usize, width: f32, d: &mut Drag, tx: &mut Tx) {
    let now = tx.clock();
    s.core.observe_clock(now);
    if tx.button() == PointerButton::Middle {
        if d.phase == Phase::Up && !d.cancelled() && d.dx.abs() < geometry::DRAG_SLOP as f32 {
            s.core.dispatch(Input::Action(Action::Close(id)), now);
        }
        return;
    }
    match d.phase {
        Phase::Down => {
            s.pointer_pressed = true;
            s.tab_drag = Some(TabDrag { id, from: index, to: index, dx: 0.0, moved: false });
            if s.core.with_model(|m| m.active != Some(id)) {
                s.core.dispatch(Input::Action(Action::Activate(id)), now);
            }
        }
        Phase::Move => {
            let Some(mut drag) = s.tab_drag.filter(|t| t.id == id) else { return };
            drag.dx = d.dx;
            drag.moved |= d.dx.abs() >= geometry::DRAG_SLOP as f32;
            if drag.moved {
                let to = s.core.with_model(|m| {
                    let wanted = geometry::drop_index(drag.from, f64::from(d.dx), f64::from(width), m.tabs.len());
                    geometry::valid_drop(&m.tabs, drag.from, wanted)
                });
                if to != drag.to {
                    drag.to = to;
                    // The others slide to make room.
                    tx.animated();
                }
            }
            s.tab_drag = Some(drag);
        }
        Phase::Up => {
            let drag = s.tab_drag.take().filter(|t| t.id == id);
            if let Some(drag) = drag
                && drag.moved
                && !d.cancelled()
                && drag.to != drag.from
            {
                tx.animated();
                s.core.dispatch(Input::Action(Action::Move { from: drag.from, to: drag.to }), now);
            }
        }
    }
}

/// The tab's close button: a small round target with a crisp vector cross.
fn close_button(key: &str, name: &str, id: TabId, active: bool, p: &Palette) -> El {
    let ink = if active { p.text_secondary } else { p.text_secondary.alpha(0.85) };
    let cross = Shape::new()
        .line([5.0, 5.0], [11.0, 11.0], 1.4, LineCap::Round, ink)
        .line([11.0, 5.0], [5.0, 11.0], 1.4, LineCap::Round, ink);
    el()
        .key(format!("{key}-close"))
        .role("button")
        .label(format!("Close {name}"))
        .size(16.0, 16.0)
        .radius(8.0)
        .hover(p.press)
        .press(p.press)
        .child(canvas(cross).size(16.0, 16.0))
        .on_drag(|_s: &mut Ui, _d: &mut Drag, _tx: &mut Tx| {})
        .on_press_tx(move |s: &mut Ui, tx: &mut Tx| {
            s.pointer_pressed = false;
            s.core.dispatch(Input::Action(Action::Close(id)), tx.clock());
        })
}

/// The tab's app icon (crisp at 2x: rasterized for the window's scale), dimmed for an app
/// that is not running and gently pulsing while its app launches; a rounded initial for a
/// path without one.
fn icon(s: &Ui, tab: &Tab, key: &str, p: &Palette, dormant: bool, loading: bool) -> El {
    let image = s.core.icons().app_icon(&tab.app.path, 16.0);
    let key = format!("{key}-icon");
    let mut el = if image.error().is_some() {
        let initial = tab.app.name.chars().next().unwrap_or_default().to_string();
        el().key(key)
            .size(16.0, 16.0)
            .radius(4.0)
            .fill(p.hover)
            .center()
            .child(text(initial).font(TextStyle::new(10.0, 600.0)).color(p.text_secondary))
    } else {
        el().key(key).size(16.0, 16.0).pixels(image.pixels())
    };
    el = el.label(format!("{} icon", tab.app.name));
    if loading {
        el.looping(LoopSpec::opacity(1.1, 0.0, 0.35, 1.0))
    } else if dormant {
        el.opacity(0.5)
    } else {
        el
    }
}

/// The "+" button: opens the app picker under itself.
fn new_tab(p: &Palette) -> El {
    let ink = p.text_secondary;
    let plus = Shape::new()
        .line([6.0, 1.0], [6.0, 11.0], 1.5, LineCap::Round, ink)
        .line([1.0, 6.0], [11.0, 6.0], 1.5, LineCap::Round, ink);
    let button = el()
        .key("new-tab")
        .role("button")
        .label("New tab")
        .size(28.0, 28.0)
        .radius(8.0)
        .continuous_corners()
        .center()
        .hover(p.hover)
        .press(p.press)
        .child(canvas(plus).size(12.0, 12.0))
        .on_layout(|s: &mut Ui, rect: [f32; 4]| s.plus_x = rect[0])
        .on_press_tx(|s: &mut Ui, tx: &mut Tx| s.open_picker(tx));
    el().key("new-tab-slot").w(NEW_TAB as f32).h_full().center().child(button)
}

/// Empty bar space: a press drags the Oriel frame (bar and glued window together, driven by
/// Oriel, see `Core::begin_bar_drag`); a double click zooms it.
fn drag_area(el: El) -> El {
    el.on_drag(|s: &mut Ui, d: &mut Drag, tx: &mut Tx| {
        s.core.observe_clock(tx.clock());
        match d.phase {
            Phase::Down => s.core.begin_bar_drag(),
            Phase::Move => s.core.follow_pointer(),
            Phase::Up => {
                s.core.end_bar_drag();
                let still = d.dx.abs() < geometry::DRAG_SLOP as f32 && d.dy.abs() < geometry::DRAG_SLOP as f32;
                if d.clicks >= 2 && still && !d.cancelled() {
                    s.core.toggle_zoom(tx.clock());
                }
            }
        }
    })
}

/// What the bar shows while Oriel lacks Accessibility permission: one calm line and one
/// button. The tabs wait; they come back as soon as access is granted.
fn needs_access(model: &Model, p: &Palette) -> El {
    let held = match model.tabs.len() {
        0 => "Oriel arranges app windows under this bar.".to_string(),
        1 => "Your tab is kept until then.".to_string(),
        n => format!("Your {n} tabs are kept until then."),
    };
    let words = col()
        .gap(1.0)
        .shrink()
        .min_w(0.0)
        .child(text("Allow Oriel to arrange your windows").font(TextStyle::new(12.5, 600.0)).color(p.text).truncate())
        .child(text(held).font(TextStyle::new(11.0, 450.0)).color(p.text_secondary).truncate());
    let grant = el()
        .key("grant-access")
        .role("button")
        .label("Grant Access")
        .h(24.0)
        .px(11.0)
        .radius(7.0)
        .continuous_corners()
        .center()
        .fill(p.accent)
        .press_scale(0.97)
        .child(text("Grant Access…").font(TextStyle::new(12.0, 600.0)).color(Color::WHITE))
        .on_press_tx(|s: &mut Ui, tx: &mut Tx| {
            s.core.dispatch(Input::Action(Action::RequestTrust), tx.clock());
        });
    shell(p)
        .pl(12.0)
        .pr(8.0)
        .gap(10.0)
        .center_y()
        .child(lock_glyph(p))
        .child(words)
        .child(grant)
        // The rest of the strip drags the window, at full height.
        .child(drag_area(spacer().key("drag-area").grow(1.0).min_w(geometry::DRAG_MIN as f32).h_full()))
}

/// A small lock, drawn: the Accessibility request's mark.
fn lock_glyph(p: &Palette) -> El {
    let shackle = rune::Path::new().move_to([4.5, 8.0]).line_to([4.5, 5.5]).cubic_to([4.5, 1.5], [11.5, 1.5], [11.5, 5.5]).line_to([11.5, 8.0]);
    let mark = Shape::new()
        .stroke_path(&shackle, 1.6, LineCap::Round, p.accent)
        .fill(vec![[3.0, 8.0], [13.0, 8.0], [13.0, 15.0], [3.0, 15.0]], p.accent);
    el().key("lock").size(22.0, 22.0).radius(6.0).fill(p.accent.alpha(0.14)).center().child(canvas(mark).size(16.0, 16.0))
}

/// A readable, stable suffix for a tab's key: the app and, when glued, its window.
fn tab_slug(tab: &Tab) -> String {
    match &tab.window {
        Some(window) => format!("{}-{}", tab.app.bundle_id, window.id.0),
        None => tab.app.bundle_id.clone(),
    }
}
