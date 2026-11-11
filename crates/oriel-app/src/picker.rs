//! The picker: a Spotlight-like panel under the "+" button. Type to narrow the installed
//! apps; Up/Down choose, Return (or a click) adds the chosen app as a tab, Esc closes. Apps
//! that already have a tab, and apps that run, are marked.

use crate::style::{PICKER_RADIUS, Palette};
use crate::ui::{self, Ui};
use oriel_core::AppInfo;
use oriel_core::model::{Action, Input};
use rune::tok::motion;
use rune::{
    Color, Cx, El, GrowthSpec, LineCap, LoopSpec, ScrollSpec, Shape, SystemMaterial, TextStyle, Transition, canvas, col,
    el, row, spacer, text,
};
use std::rc::Rc;

const ROW_H: f32 = 44.0;

/// The picker's view: the search field above a virtual list of matches, a hint line below.
pub(crate) fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let dark = cx.system_dark();
    let p = s.core.palette(dark);
    let (scanned, in_bar): (bool, Vec<String>) =
        s.core.with_model(|m| (!m.installed.is_empty(), m.tabs.iter().map(|t| t.app.bundle_id.clone()).collect()));

    let header = row()
        .key("search-row")
        .h(56.0)
        .pl(18.0)
        .pr(16.0)
        .gap(12.0)
        .center_y()
        .child(magnifier(&p))
        .child(ui::search_field(s, p.text, p.text_secondary.alpha(0.8)));

    let body = if !scanned {
        loading(&p)
    } else {
        let hits = Rc::new(s.hits());
        if hits.is_empty() { empty(&s.query.text, &p) } else { list(s, &hits, &in_bar, &p) }
    };

    col()
        // A new root on each open, so the panel eases in every time it appears.
        .key(format!("app-picker-{}", s.opened))
        .label("Choose an application")
        .w_full()
        .h_full()
        .system_material(SystemMaterial::Popover)
        .fill(p.panel)
        .radius(PICKER_RADIUS)
        .continuous_corners()
        .border(1.0, p.hairline)
        .transition(Transition::opacity().combined(Transition::scale(0.97)))
        .motion(motion::SNAP)
        .child(header)
        .child(el().h(1.0).w_full().fill(p.hairline))
        .child(body)
        .child(footer(&p))
}

/// Enter: add the highlighted app as a tab and close the picker.
pub(crate) fn submit(s: &mut Ui, tx: &mut rune::Tx) {
    let Some(app) = s.hits().get(s.picked).cloned() else { return };
    s.core.dispatch(Input::Action(Action::AddApp(app)), tx.clock());
    s.query.clear();
    tx.close_window();
}

/// The scroll viewport with the windowed list inside.
fn list(s: &Ui, hits: &Rc<Vec<AppInfo>>, in_bar: &[String], p: &Palette) -> El {
    let count = hits.len();
    let keys = hits.clone();
    let rows_hits = hits.clone();
    let in_bar: Rc<Vec<String>> = Rc::new(in_bar.to_vec());
    let p = *p;
    let rows = col()
        .key("app-rows")
        .role("listbox")
        .label("Applications")
        .growth(GrowthSpec::default())
        .py(6.0)
        .virtual_rows(
            count,
            ROW_H,
            move |_s: &Ui, i: usize| ui::row_key(&keys[i]).into(),
            move |s: &Ui, i: usize| {
                let app = &rows_hits[i];
                let mark = if in_bar.contains(&app.bundle_id) {
                    Some("In Oriel")
                } else if s.running.contains(&app.bundle_id) {
                    Some("Running")
                } else {
                    None
                };
                app_row(s, app, i, mark, &p)
            },
        )
        .virtual_rows_version((s.query.text.clone(), count, s.picked, s.opened));

    col()
        .key("results")
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(rows)
}

/// One app row: icon, name, a mark, and a press that adds the app and closes the picker. The
/// pointer chooses as it moves, as in Spotlight.
fn app_row(s: &Ui, app: &AppInfo, i: usize, mark: Option<&str>, p: &Palette) -> El {
    let selected = i == s.picked;
    let image = s.core.icons().app_icon(&app.path, 28.0);
    let icon = if image.error().is_some() {
        let initial = app.name.chars().next().unwrap_or_default().to_string();
        el().size(28.0, 28.0)
            .radius(7.0)
            .fill(p.hover)
            .center()
            .child(text(initial).font(TextStyle::new(13.0, 600.0)).color(p.text_secondary))
    } else {
        el().size(28.0, 28.0).pixels(image.pixels())
    };
    let (ink, sub) = if selected { (Color::WHITE, Color::WHITE.alpha(0.82)) } else { (p.text, p.text_secondary) };
    let mut inner = row()
        .w_full()
        .h(ROW_H - 4.0)
        .px(10.0)
        .gap(11.0)
        .center_y()
        .radius(9.0)
        .continuous_corners()
        .fill(if selected { p.accent } else { Color::TRANSPARENT })
        .child(icon)
        .child(text(app.name.clone()).font(TextStyle::new(14.0, 500.0)).color(ink).shrink().min_w(0.0).truncate())
        .child(spacer().grow(1.0));
    if let Some(mark) = mark {
        inner = inner.child(
            el().h(18.0)
                .px(7.0)
                .radius(9.0)
                .center()
                .fill(if selected { Color::WHITE.alpha(0.18) } else { p.hover })
                .child(text(mark).font(TextStyle::new(10.5, 600.0)).color(sub)),
        );
    }
    let app_for_press = app.clone();
    el().key(ui::row_key(app))
        .role("option")
        .label(app.name.clone())
        .checked(selected)
        .w_full()
        .h(ROW_H)
        .px(8.0)
        .py(2.0)
        .child(inner)
        .on_hover_change(move |s: &mut Ui, inside, _tx| {
            if inside {
                s.picked = i;
            }
        })
        .on_press_tx(move |s: &mut Ui, tx: &mut rune::Tx| {
            s.core.dispatch(Input::Action(Action::AddApp(app_for_press.clone())), tx.clock());
            s.query.clear();
            tx.close_window();
        })
}

/// A magnifying glass, drawn.
fn magnifier(p: &Palette) -> El {
    let ring = rune::Path::new()
        .move_to([13.0, 8.0])
        .cubic_to([13.0, 10.8], [10.8, 13.0], [8.0, 13.0])
        .cubic_to([5.2, 13.0], [3.0, 10.8], [3.0, 8.0])
        .cubic_to([3.0, 5.2], [5.2, 3.0], [8.0, 3.0])
        .cubic_to([10.8, 3.0], [13.0, 5.2], [13.0, 8.0])
        .close();
    let glass = Shape::new()
        .stroke_path(&ring, 1.8, LineCap::Round, p.text_secondary)
        .line([11.8, 11.8], [16.5, 16.5], 1.8, LineCap::Round, p.text_secondary);
    canvas(glass).key("magnifier").size(18.0, 18.0)
}

/// Nothing matches: say so, and what to try.
fn empty(query: &str, p: &Palette) -> El {
    col()
        .key("empty")
        .grow(1.0)
        .min_h(0.0)
        .center()
        .gap(4.0)
        .px(24.0)
        .child(text(format!("No apps match \u{201c}{}\u{201d}", query.trim())).font(TextStyle::new(14.0, 600.0)).color(p.text).truncate())
        .child(text("Check the spelling, or try part of the name.").font(TextStyle::new(12.0, 450.0)).color(p.text_secondary))
}

/// The scan has not answered yet.
fn loading(p: &Palette) -> El {
    let dot = |i: usize| {
        el().size(6.0, 6.0)
            .radius(3.0)
            .fill(p.text_secondary)
            .looping(LoopSpec::opacity(0.9, i as f64 * 0.15, 0.25, 1.0))
    };
    col()
        .key("loading")
        .grow(1.0)
        .min_h(0.0)
        .center()
        .gap(10.0)
        .child(row().gap(5.0).child(dot(0)).child(dot(1)).child(dot(2)))
        .child(text("Finding your apps").font(TextStyle::new(12.5, 500.0)).color(p.text_secondary))
}

/// The keys, quietly.
fn footer(p: &Palette) -> El {
    let hint = |key: &str, what: &str| {
        row().gap(5.0).center_y()
            .child(el().h(17.0).px(5.0).radius(4.0).center().fill(p.hover).child(text(key).font(TextStyle::new(10.5, 600.0)).color(p.text_secondary)))
            .child(text(what).font(TextStyle::new(11.0, 450.0)).color(p.text_secondary))
    };
    row()
        .key("hints")
        .h(32.0)
        .px(14.0)
        .gap(14.0)
        .center_y()
        .child(hint("\u{21a9}", "Add"))
        .child(hint("\u{2191}\u{2193}", "Choose"))
        .child(spacer().grow(1.0))
        .child(hint("esc", "Close"))
}
