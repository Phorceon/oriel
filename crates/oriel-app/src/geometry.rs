//! Pure math for the UI: coordinate conversion between Oriel rects and AppKit's bottom-left
//! points, the bar's tab layout, tab reordering, and the picker's ranking. Unit tested in
//! [`crate::geometry::tests`].

use oriel_core::{AppInfo, Rect, Tab};
use std::collections::BTreeSet;

/// How long the app waits between two writes of the saved state, in seconds.
pub(crate) const SAVE_INTERVAL: f64 = 0.5;

/// Space before the first tab: room for its left "wing" (`TAB_WING`).
pub(crate) const STRIP_PAD: f64 = 10.0;
/// A tab is never narrower than this (points): its icon with a little air.
pub(crate) const TAB_MIN: f64 = 36.0;
/// A tab is never wider than this (points), so a few tabs leave drag space.
pub(crate) const TAB_MAX: f64 = 220.0;
/// A tab this wide or wider shows its label; below it shows only the icon.
pub(crate) const TAB_LABEL: f64 = 76.0;
/// A tab this wide or wider has room for the icon and the close button side by side.
pub(crate) const TAB_ICON_AND_CLOSE: f64 = 58.0;
/// The concave curve that joins the active tab to the window under the bar.
pub(crate) const TAB_WING: f64 = 8.0;
/// The "+" button's slot (button plus its margins).
pub(crate) const NEW_TAB: f64 = 36.0;
/// The bar always keeps this much empty space for dragging at its right end (points).
pub(crate) const DRAG_MIN: f64 = 28.0;
/// A pointer press that travels less than this is a click, not a drag (points).
pub(crate) const DRAG_SLOP: f64 = 4.0;

/// AppKit's bottom-left, primary-screen-height-relative frame → an Oriel rect. AppKit
/// positions are points with the origin at the primary display's bottom-left; Oriel's are
/// points with the origin at its top-left, `y` growing downward.
pub(crate) fn appkit_to_rect(x: f64, y_bottom: f64, w: f64, h: f64, primary_height: f64) -> Rect {
    Rect::new(x, primary_height - y_bottom - h, w, h)
}

/// The room the tabs share in a bar `bar_w` points wide.
pub(crate) fn tab_room(bar_w: f64) -> f64 {
    (bar_w - STRIP_PAD - NEW_TAB - DRAG_MIN).max(0.0)
}

/// One tab's width for `count` tabs in a bar `bar_w` points wide: even widths, clamped to
/// [`TAB_MIN`], [`TAB_MAX`]], leaving [`DRAG_MIN`] for dragging. Below the minimum the tabs
/// overflow (the strip scrolls sideways) rather than shrinking further, and the "+" stays.
pub(crate) fn tab_width(bar_w: f64, count: usize) -> f64 {
    if count == 0 {
        return 0.0;
    }
    (tab_room(bar_w) / count as f64).clamp(TAB_MIN, TAB_MAX)
}

/// Whether `count` tabs overflow the room they have (the strip then scrolls).
pub(crate) fn tabs_overflow(bar_w: f64, count: usize) -> bool {
    tab_width(bar_w, count) * count as f64 > tab_room(bar_w) + 0.5
}

/// What a tab of width `w` shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TabParts {
    pub label: bool,
    /// The icon shows (a narrow active tab gives its place to the close button).
    pub icon: bool,
    pub close: bool,
}

/// The parts of a tab `w` points wide. The close button shows on the active tab and the tab
/// under the pointer; on a narrow tab it shows only on the active one, in place of the icon.
pub(crate) fn tab_parts(w: f64, active: bool, hovered: bool) -> TabParts {
    let label = w >= TAB_LABEL;
    if w >= TAB_ICON_AND_CLOSE {
        TabParts { label, icon: true, close: active || hovered }
    } else {
        TabParts { label, icon: !active, close: active }
    }
}

/// The index a tab dragged from `from` by `dx` points would drop at, among `count` tabs
/// `slot` points apart.
pub(crate) fn drop_index(from: usize, dx: f64, slot: f64, count: usize) -> usize {
    if count == 0 || slot <= 0.0 {
        return from;
    }
    let at = (from as f64 + dx / slot).round();
    at.clamp(0.0, (count - 1) as f64) as usize
}

/// `order` with the item at `from` moved to `to`.
pub(crate) fn moved<T: Clone>(order: &[T], from: usize, to: usize) -> Vec<T> {
    let mut out = order.to_vec();
    if from < out.len() && to < out.len() {
        let item = out.remove(from);
        out.insert(to, item);
    }
    out
}

/// Each app's tabs form one run (the model's invariant 1).
pub(crate) fn apps_contiguous(tabs: &[&str]) -> bool {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut current: Option<&str> = None;
    for app in tabs {
        if current != Some(*app) {
            if !seen.insert(app) {
                return false;
            }
            current = Some(app);
        }
    }
    true
}

/// The nearest index to `wanted` that the tab at `from` can move to without splitting an
/// app's tabs (checked from `wanted` back toward `from`, which is always allowed).
pub(crate) fn valid_drop(tabs: &[Tab], from: usize, wanted: usize) -> usize {
    let apps: Vec<&str> = tabs.iter().map(|t| t.app.bundle_id.as_str()).collect();
    let mut to = wanted;
    while to != from {
        if apps_contiguous(&moved(&apps, from, to)) {
            return to;
        }
        to = if to > from { to - 1 } else { to + 1 };
    }
    from
}

/// The apps whose name matches `query`, best first: names that start with it, then names
/// with a word that starts with it, then names that contain it, each group in the order
/// given. An empty query keeps the order given.
pub(crate) fn matches_query<'a>(apps: &'a [AppInfo], query: &str) -> Vec<&'a AppInfo> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return apps.iter().collect();
    }
    let mut ranked: Vec<(u8, &AppInfo)> = apps
        .iter()
        .filter_map(|a| {
            let name = a.name.to_lowercase();
            let rank = if name.starts_with(&q) {
                0
            } else if name.split([' ', '-', '.']).any(|word| word.starts_with(&q)) {
                1
            } else if name.contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, a))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, a)| a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oriel_core::{TabId, WindowId, WindowInfo};

    #[test]
    fn appkit_flip_uses_the_primary_height() {
        // A window 100 pt above the primary display's bottom edge.
        let rect = appkit_to_rect(30.0, 100.0, 200.0, 40.0, 900.0);
        assert_eq!(rect, Rect::new(30.0, 760.0, 200.0, 40.0));
        // The top-left corner of the screen is y 0 in Oriel coordinates.
        let top = appkit_to_rect(0.0, 860.0, 100.0, 40.0, 900.0);
        assert_eq!(top.y, 0.0);
    }

    #[test]
    fn tabs_share_the_strip_evenly_and_stay_inside_their_bounds() {
        assert_eq!(tab_width(1200.0, 0), 0.0);
        // Few tabs: the maximum width wins, and the leftover is drag space.
        assert_eq!(tab_width(1470.0, 3), TAB_MAX);
        // Many tabs shrink evenly and leave the drag space.
        let w = tab_width(1200.0, 10);
        assert!((w - tab_room(1200.0) / 10.0).abs() < 0.01, "{w}");
        assert!((TAB_MIN..=TAB_MAX).contains(&w));
        assert!(!tabs_overflow(1200.0, 10));
        // The narrowest bar keeps its drag area with 3 tabs.
        let narrow = tab_width(230.0, 3);
        assert!(230.0 - STRIP_PAD - NEW_TAB - 3.0 * narrow >= DRAG_MIN - 0.01, "{narrow}");
        // 20 tabs in 230 pt stop at the minimum and overflow (the strip scrolls).
        assert_eq!(tab_width(230.0, 20), TAB_MIN);
        assert!(tabs_overflow(230.0, 20));
    }

    #[test]
    fn tab_parts_follow_the_width() {
        let wide = tab_parts(200.0, false, false);
        assert_eq!(wide, TabParts { label: true, icon: true, close: false });
        assert!(tab_parts(200.0, true, false).close);
        assert!(tab_parts(200.0, false, true).close);
        // Icon and close side by side, no label.
        assert_eq!(tab_parts(60.0, true, false), TabParts { label: false, icon: true, close: true });
        // Narrow: the active tab shows only its close button, others only their icon.
        assert_eq!(tab_parts(40.0, true, false), TabParts { label: false, icon: false, close: true });
        assert_eq!(tab_parts(40.0, false, true), TabParts { label: false, icon: true, close: false });
    }

    fn tab(id: u64, app: &str, window: Option<u32>) -> Tab {
        Tab {
            id: TabId(id),
            app: AppInfo { bundle_id: app.into(), name: app.into(), path: "/x.app".into() },
            window: window.map(|w| WindowInfo { id: WindowId(w), title: String::new() }),
        }
    }

    #[test]
    fn a_dragged_tab_drops_at_the_nearest_slot_inside_the_strip() {
        assert_eq!(drop_index(0, 0.0, 100.0, 4), 0);
        assert_eq!(drop_index(0, 49.0, 100.0, 4), 0);
        assert_eq!(drop_index(0, 51.0, 100.0, 4), 1);
        assert_eq!(drop_index(1, -500.0, 100.0, 4), 0);
        assert_eq!(drop_index(1, 900.0, 100.0, 4), 3);
    }

    #[test]
    fn a_reorder_never_splits_an_app() {
        // a a b c: the second a can move inside its run only; b can jump past c or before a.
        let tabs = vec![tab(1, "a", Some(1)), tab(2, "a", Some(2)), tab(3, "b", None), tab(4, "c", None)];
        assert_eq!(valid_drop(&tabs, 1, 0), 0);
        assert_eq!(valid_drop(&tabs, 1, 3), 1, "a's second tab cannot leave its run");
        assert_eq!(valid_drop(&tabs, 3, 0), 0, "c can go first");
        assert_eq!(valid_drop(&tabs, 3, 1), 2, "c cannot split a: it stops after a's run");
        assert_eq!(valid_drop(&tabs, 2, 3), 3);
        assert!(apps_contiguous(&["a", "a", "b"]));
        assert!(!apps_contiguous(&["a", "b", "a"]));
        assert_eq!(moved(&[1, 2, 3, 4], 0, 2), vec![2, 3, 1, 4]);
    }

    #[test]
    fn picker_ranks_prefix_then_word_then_substring() {
        let apps: Vec<AppInfo> = ["Calendar", "Calculator", "iCal Helper", "Notes", "Music"]
            .iter()
            .map(|n| AppInfo { bundle_id: n.to_lowercase(), name: (*n).into(), path: "/x.app".into() })
            .collect();
        let names = |q: &str| matches_query(&apps, q).iter().map(|a| a.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("").len(), apps.len(), "empty query lists everything");
        assert_eq!(names("cal"), ["Calendar", "Calculator", "iCal Helper"]);
        assert_eq!(names("  HELP "), ["iCal Helper"], "trimmed, case-insensitive, word start");
        assert_eq!(names("usi"), ["Music"]);
        assert!(names("zzz").is_empty());
    }
}
