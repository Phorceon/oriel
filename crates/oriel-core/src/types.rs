//! Shared data types. This file is the frozen contract between the crates.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Height of Oriel's tab bar, in points.
pub const BAR_HEIGHT: f64 = 40.0;

/// Two rects are the same when every field differs by less than this many points.
pub const RECT_EPSILON: f64 = 1.0;

/// Width of the divider between the two panes of a split view, in points.
pub const SPLIT_GAP: f64 = 8.0;

/// The narrowest pane of a split view, in points. When the content is too narrow for two such
/// panes, the ratio is clamped to [`FALLBACK_RATIO`] instead.
pub const MIN_PANE: f64 = 320.0;

/// The ratio limits that apply when the content cannot hold two [`MIN_PANE`] panes.
pub const FALLBACK_RATIO: (f64, f64) = (0.2, 0.8);

/// A rectangle in global screen points. The origin is the top-left corner of the primary
/// display and `y` grows downward. This is the coordinate space of the macOS Accessibility
/// API and of Core Graphics (not of AppKit, whose origin is bottom-left).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// True when `self` and `other` differ by less than [`RECT_EPSILON`] in every field.
    pub fn close_to(&self, other: &Rect) -> bool {
        (self.x - other.x).abs() < RECT_EPSILON
            && (self.y - other.y).abs() < RECT_EPSILON
            && (self.w - other.w).abs() < RECT_EPSILON
            && (self.h - other.h).abs() < RECT_EPSILON
    }

    /// The tab bar's rect for an Oriel frame: the top [`BAR_HEIGHT`] points.
    pub fn bar(&self) -> Rect {
        Rect::new(self.x, self.y, self.w, BAR_HEIGHT)
    }

    /// The glued app window's rect for an Oriel frame: everything below the bar.
    pub fn content(&self) -> Rect {
        Rect::new(self.x, self.y + BAR_HEIGHT, self.w, (self.h - BAR_HEIGHT).max(0.0))
    }

    /// The Oriel frame whose bar is `bar`, keeping the content height `content_h`.
    pub fn from_bar(bar: &Rect, content_h: f64) -> Rect {
        Rect::new(bar.x, bar.y, bar.w, BAR_HEIGHT + content_h)
    }

    /// The *fit* of a window of size `w x h` that cannot take this content rect: that size,
    /// top-aligned (`y = self.y`) and centered across (`x = self.x + (self.w - w) / 2`), or
    /// left-aligned (`x = self.x`) when it is wider. The model and the platform both use it.
    pub fn fit(&self, w: f64, h: f64) -> Rect {
        let x = if w > self.w { self.x } else { self.x + (self.w - w) / 2.0 };
        Rect::new(x, self.y, w, h)
    }

    /// The Oriel frame whose content rect is `content`.
    pub fn from_content(content: &Rect) -> Rect {
        Rect::new(content.x, content.y - BAR_HEIGHT, content.w, content.h + BAR_HEIGHT)
    }
}

/// Two tabs shown side by side below the one bar (split view). See "Split view" in `model.rs`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Split {
    /// The tab of the left pane.
    pub left: TabId,
    /// The tab of the right pane.
    pub right: TabId,
    /// The left pane's share of the usable content width (the width without [`SPLIT_GAP`]).
    pub ratio: f64,
}

impl Split {
    /// Is this tab one of the two?
    pub fn has(&self, id: TabId) -> bool {
        self.left == id || self.right == id
    }

    /// The other tab of the pair, or `None` when `id` is not in it.
    pub fn partner(&self, id: TabId) -> Option<TabId> {
        if id == self.left {
            Some(self.right)
        } else if id == self.right {
            Some(self.left)
        } else {
            None
        }
    }
}

/// The ratio limits for a content `width`: both panes at least [`MIN_PANE`] wide, or
/// [`FALLBACK_RATIO`] when the content is too narrow for that.
pub fn ratio_limits(width: f64) -> (f64, f64) {
    let usable = width - SPLIT_GAP;
    if usable >= 2.0 * MIN_PANE {
        (MIN_PANE / usable, 1.0 - MIN_PANE / usable)
    } else {
        FALLBACK_RATIO
    }
}

/// `ratio` held inside [`ratio_limits`] of a content `width`. Not a number gives one half.
pub fn clamp_ratio(width: f64, ratio: f64) -> f64 {
    if ratio.is_nan() {
        return 0.5;
    }
    let (lo, hi) = ratio_limits(width);
    ratio.clamp(lo, hi)
}

/// The three rects of a split view: the left pane, the divider and the right pane. They tile
/// `content` exactly: the left width is rounded to whole points, the divider is
/// [`SPLIT_GAP`] wide, the right pane takes the rest. `ratio` is clamped first.
pub fn split_rects(content: &Rect, ratio: f64) -> (Rect, Rect, Rect) {
    let ratio = clamp_ratio(content.w, ratio);
    let usable = (content.w - SPLIT_GAP).max(0.0);
    let left_w = (usable * ratio).round();
    let left = Rect::new(content.x, content.y, left_w, content.h);
    let divider = Rect::new(content.x + left_w, content.y, SPLIT_GAP, content.h);
    let right_x = content.x + left_w + SPLIT_GAP;
    let right = Rect::new(right_x, content.y, content.x + content.w - right_x, content.h);
    (left, divider, right)
}

/// The ratio that puts a left pane `left_w` wide in a content `width` (not clamped).
pub fn ratio_for_left(width: f64, left_w: f64) -> f64 {
    let usable = width - SPLIT_GAP;
    if usable <= 0.0 { 0.5 } else { left_w / usable }
}

/// An installed application.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppInfo {
    /// The bundle identifier, for example `com.todesktop.230313mzl4w4u92` (Cursor). This is
    /// the app's identity everywhere in Oriel.
    pub bundle_id: String,
    /// The display name, for example `Cursor`.
    pub name: String,
    /// The absolute path of the `.app` bundle. The UI reads the icon from it.
    pub path: PathBuf,
}

/// The system-wide id of one window (`CGWindowID` on macOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(pub u32);

/// One standard (document-like) window of a watched app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
}

/// The id of one tab. Unique for the life of the process. Never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

/// One tab in the bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    pub app: AppInfo,
    /// The glued window. `None` is a placeholder tab: the app is not running, or it is
    /// running with no standard window. An app has at most one placeholder tab, and only
    /// when it has no window tab.
    pub window: Option<WindowInfo>,
}

impl Tab {
    /// The text the bar shows: the window title, or the app name when the title is empty
    /// or the tab is a placeholder.
    pub fn label(&self) -> &str {
        match &self.window {
            Some(w) if !w.title.trim().is_empty() => &w.title,
            _ => &self.app.name,
        }
    }
}

/// What Oriel remembers between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    /// The apps in the bar, in tab order, without duplicates.
    #[serde(default)]
    pub apps: Vec<AppInfo>,
    /// The Oriel frame (bar plus content).
    #[serde(default)]
    pub frame: Option<Rect>,
    /// The bundle id of the app whose tab was active.
    #[serde(default)]
    pub active_app: Option<String>,
}
