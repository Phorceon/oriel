//! Oriel's colors and sizes, light and dark. The bar reads as the title area of the window
//! glued under it: the active tab takes the system window background (read from AppKit when
//! the app starts, `macwin::window_backgrounds`), so it runs into that window's title bar,
//! and the strip around it is a step darker, as a browser's tab strip is.

use rune::{Color, oklch, oklcha};

/// The bar's top corners (its bottom edge meets the glued window, square).
pub(crate) const BAR_RADIUS: f32 = 14.0;
/// The active tab: top inset in the bar, and its top corners.
pub(crate) const TAB_TOP: f32 = 6.0;
pub(crate) const TAB_RADIUS: f32 = 9.0;
/// The picker window's corners.
pub(crate) const PICKER_RADIUS: f32 = 16.0;
/// The picker's size in points.
pub(crate) const PICKER_SIZE: (f32, f32) = (380.0, 430.0);

#[derive(Clone, Copy, Debug)]
pub(crate) struct Palette {
    /// The bar behind the tabs.
    pub strip: Color,
    /// The active tab: the window background, as the glued window's title bar.
    pub tab: Color,
    /// The tab of a shown split that does not have the focus: between the strip and the tab.
    pub tab_pair: Color,
    /// An inactive tab under the pointer, and a held press.
    pub hover: Color,
    pub press: Color,
    pub text: Color,
    pub text_secondary: Color,
    /// A tab whose app is not running.
    pub text_dormant: Color,
    /// The line between two inactive tabs.
    pub separator: Color,
    /// The bar's outline (top and sides).
    pub hairline: Color,
    /// The picker's panel tint over its material.
    pub panel: Color,
    pub accent: Color,
}

/// sRGB components (0..1) to the linear colour Rune works in.
pub(crate) fn srgb(r: f64, g: f64, b: f64, a: f64) -> Color {
    let lin = |c: f64| {
        let c = c.clamp(0.0, 1.0);
        (if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }) as f32
    };
    Color { r: lin(r), g: lin(g), b: lin(b), a: a as f32 }
}

/// The linear mix of two colors, `t` of the way from `a` to `b`.
fn mix(a: Color, b: Color, t: f32) -> Color {
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    Color { r: lerp(a.r, b.r), g: lerp(a.g, b.g), b: lerp(a.b, b.b), a: lerp(a.a, b.a) }
}

/// A Rune color (linear) as sRGB components (0..1), for AppKit.
pub(crate) fn to_srgb(c: Color) -> [f64; 4] {
    let enc = |v: f32| {
        let v = f64::from(v).clamp(0.0, 1.0);
        if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    };
    [enc(c.r), enc(c.g), enc(c.b), f64::from(c.a)]
}

/// The palette for one appearance. `window_bg` is the system window background for it, when
/// known (a headless render has none and uses the macOS 26 default).
pub(crate) fn palette(dark: bool, window_bg: Option<Color>) -> Palette {
    if dark {
        let tab = window_bg.unwrap_or(oklch(0.262, 0.003, 265.0));
        let strip = oklch(0.178, 0.004, 265.0);
        Palette {
            strip,
            tab,
            tab_pair: mix(strip, tab, 0.62),
            hover: oklcha(1.0, 0.0, 0.0, 0.07),
            press: oklcha(1.0, 0.0, 0.0, 0.11),
            text: oklch(0.965, 0.002, 265.0),
            text_secondary: oklch(0.74, 0.006, 265.0),
            text_dormant: oklch(0.56, 0.006, 265.0),
            separator: oklcha(1.0, 0.0, 0.0, 0.13),
            hairline: oklcha(1.0, 0.0, 0.0, 0.11),
            panel: oklcha(0.2, 0.004, 265.0, 0.86),
            accent: oklch(0.6, 0.17, 255.0),
        }
    } else {
        let tab = window_bg.unwrap_or(oklch(0.985, 0.001, 265.0));
        let strip = oklch(0.912, 0.004, 265.0);
        Palette {
            strip,
            tab,
            tab_pair: mix(strip, tab, 0.62),
            hover: oklcha(0.0, 0.0, 0.0, 0.055),
            press: oklcha(0.0, 0.0, 0.0, 0.09),
            text: oklch(0.21, 0.004, 265.0),
            text_secondary: oklch(0.45, 0.006, 265.0),
            text_dormant: oklch(0.6, 0.006, 265.0),
            separator: oklcha(0.0, 0.0, 0.0, 0.14),
            hairline: oklcha(0.0, 0.0, 0.0, 0.12),
            panel: oklcha(0.985, 0.002, 265.0, 0.86),
            accent: oklch(0.58, 0.18, 255.0),
        }
    }
}
