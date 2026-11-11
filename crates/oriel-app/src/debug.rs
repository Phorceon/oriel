//! Debug hooks for input-free testing. All are off unless their env var is set, and only
//! the bar window reads them (from `Ui::poll`).
//!
//! - `ORIEL_DEBUG_RAISE_AFTER=<seconds>`: that long after the first poll, dispatch
//!   `Event::WindowFocused` for the active tab's window, as the platform does when the
//!   glued app becomes frontmost. The model then emits `Effect::RaiseBar`.
//! - `ORIEL_DEBUG_OPEN_PICKER=<seconds>`: that long after the first poll, run the same
//!   `Ui::open_picker` path as a press on the "+" button.
//! - `ORIEL_DEBUG_GLUED_WINDOW=<CGWindowID>`: `Effect::RaiseBar` orders the bar above this
//!   window instead of the active tab's (the fake's window ids are not real windows).
//! - `ORIEL_DEBUG_SCRIPT="<t> <step> [args]; ..."`: timed steps, `t` in seconds after the
//!   first poll. Every step prints `oriel-mark <epoch ms> <step>` on stderr, so a sampler
//!   can line its samples up with it. Steps:
//!   - `drag <speed pt/s> <span pt> <seconds> [<end dx> <end dy>]`: drag the bar back and
//!     forth along x, then (with the end offsets) to that offset from where it began, as a
//!     drag that ends elsewhere does.
//!   - `activate <tab index>`: the same `Action::Activate` as a click on that tab.
//!   - `split <tab index>`: the same `Action::Split` as "Split with Current Tab" on that tab.
//!   - `unsplit`: `Action::Unsplit`.
//!   - `ratio <0..1>`: `Action::SplitRatio`, as a divider drag step.
//!   - `focus <tab index>`: `Event::WindowFocused` for that tab's window, as a click into a pane.
//!   - `quit`: the same quit path as ⌘Q.
//!   - `mark <name>`: only the mark.
//!
//! - `ORIEL_DEBUG_COMMANDS=1` (with marks on): also print every platform command the model sends
//!   (`oriel-mark <ms> cmd Present { .. }`), and `divider <rect>` for every divider effect.
//!
//! The first two fire once.

use crate::ui::Ui;
use oriel_core::WindowId;
use oriel_core::model::{Action, Input};
use oriel_core::platform::Event;
use rune::Tx;

#[derive(Default)]
pub(crate) struct Hooks {
    start: Option<f64>,
    raise_at: Option<f64>,
    picker_at: Option<f64>,
    script: Vec<(f64, String)>,
    /// Headless scenes: the demo was seeded and its scene state is still to apply.
    scene: Option<bool>,
}

/// The `ORIEL_DEBUG_GLUED_WINDOW` override, if set.
pub(crate) fn glued_override() -> Option<u32> {
    std::env::var("ORIEL_DEBUG_GLUED_WINDOW").ok()?.trim().parse().ok()
}

fn seconds(name: &str) -> Option<f64> {
    std::env::var(name).ok()?.trim().parse::<f64>().ok().filter(|s| *s >= 0.0)
}

/// Wall-clock milliseconds since the epoch, the sampler's clock.
pub(crate) fn epoch_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Whether harness marks are on (`ORIEL_DEBUG_SCRIPT` or `ORIEL_DEBUG_MARKS` is set).
pub(crate) fn marks_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var_os("ORIEL_DEBUG_SCRIPT").is_some() || std::env::var_os("ORIEL_DEBUG_MARKS").is_some()
    })
}

/// Print one mark for the harness.
pub(crate) fn mark(what: &str) {
    if marks_on() {
        eprintln!("oriel-mark {:.3} {what}", epoch_ms());
    }
}

fn parse_script(src: &str) -> Vec<(f64, String)> {
    let mut steps: Vec<(f64, String)> = src
        .split(';')
        .filter_map(|step| {
            let step = step.trim();
            let (t, rest) = step.split_once(' ')?;
            Some((t.parse().ok()?, rest.trim().to_string()))
        })
        .collect();
    steps.sort_by(|a, b| a.0.total_cmp(&b.0));
    steps
}

impl Hooks {
    /// The demo scene state still waits to be applied (true from the seeding on, until
    /// `scene_applied`).
    pub(crate) fn scene_pending(&mut self, seeded: bool) -> bool {
        if seeded {
            self.scene = Some(true);
        }
        self.scene == Some(true)
    }

    pub(crate) fn scene_applied(&mut self) {
        self.scene = Some(false);
    }

    pub(crate) fn from_env() -> Hooks {
        Hooks {
            scene: None,
            start: None,
            raise_at: seconds("ORIEL_DEBUG_RAISE_AFTER"),
            picker_at: seconds("ORIEL_DEBUG_OPEN_PICKER"),
            script: std::env::var("ORIEL_DEBUG_SCRIPT").map(|s| parse_script(&s)).unwrap_or_default(),
        }
    }
}

/// Run what is due; schedule a wake for what is not. Returns "state changed".
pub(crate) fn tick(s: &mut Ui, now: f64, tx: &mut Tx) -> bool {
    if s.debug.raise_at.is_none() && s.debug.picker_at.is_none() && s.debug.script.is_empty() {
        return false;
    }
    let start = *s.debug.start.get_or_insert(now);
    let mut changed = false;
    if let Some(after) = s.debug.raise_at {
        if now - start >= after {
            s.debug.raise_at = None;
            let target = s.core.with_model(|m| {
                m.active_tab().and_then(|tab| tab.window.as_ref().map(|w| (tab.app.bundle_id.clone(), w.id.0)))
            });
            if let Some((bundle_id, id)) = target {
                eprintln!("oriel debug: WindowFocused {bundle_id} {id}");
                s.core.dispatch_event(Event::WindowFocused { bundle_id, window: WindowId(id) }, now);
                changed = true;
            }
        } else {
            tx.wake_at(start + after);
        }
    }
    if let Some(after) = s.debug.picker_at {
        if now - start >= after {
            s.debug.picker_at = None;
            eprintln!("oriel debug: open_picker");
            s.open_picker(tx);
            changed = true;
        } else {
            tx.wake_at(start + after);
        }
    }
    while let Some((at, step)) = s.debug.script.first().cloned() {
        if now - start < at {
            tx.wake_at(start + at);
            break;
        }
        s.debug.script.remove(0);
        mark(&step);
        changed |= run_step(s, &step, now, tx);
    }
    changed
}

fn run_step(s: &mut Ui, step: &str, now: f64, tx: &mut Tx) -> bool {
    let parts: Vec<&str> = step.split_whitespace().collect();
    let num = |i: usize, default: f64| parts.get(i).and_then(|v| v.parse().ok()).unwrap_or(default);
    match parts.first().copied() {
        Some("drag") => {
            s.core.debug_drag(num(1, 1000.0), num(2, 300.0), num(3, 3.0), (num(4, 0.0), num(5, 0.0)));
            false
        }
        Some("activate") => {
            let index = num(1, 0.0) as usize;
            let id = s.core.with_model(|m| m.tabs.get(index).map(|t| t.id));
            if let Some(id) = id {
                s.core.dispatch(Input::Action(Action::Activate(id)), now);
            }
            true
        }
        Some("split") => {
            let id = s.core.with_model(|m| m.tabs.get(num(1, 0.0) as usize).map(|t| t.id));
            if let Some(id) = id {
                s.core.dispatch(Input::Action(Action::Split(id)), now);
            }
            true
        }
        Some("unsplit") => {
            s.core.dispatch(Input::Action(Action::Unsplit), now);
            true
        }
        Some("ratio") => {
            s.core.dispatch(Input::Action(Action::SplitRatio(num(1, 0.5))), now);
            true
        }
        Some("focus") => {
            let target = s.core.with_model(|m| {
                m.tabs
                    .get(num(1, 0.0) as usize)
                    .and_then(|t| t.window.as_ref().map(|w| (t.app.bundle_id.clone(), w.id)))
            });
            if let Some((bundle_id, window)) = target {
                s.core.dispatch_event(Event::WindowFocused { bundle_id, window }, now);
            }
            true
        }
        Some("quit") => {
            s.quit(tx);
            false
        }
        _ => false,
    }
}
