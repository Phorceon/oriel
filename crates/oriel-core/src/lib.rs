//! Oriel's platform-free core. See `model.rs` for the rules and `platform.rs` for the OS
//! contract.

pub mod model;
pub mod platform;
pub mod store;
pub mod types;

pub use model::Action;
pub use model::Effect;
pub use model::Input;
pub use model::Model;
pub use platform::Command;
pub use platform::Event;
pub use platform::EventSink;
pub use platform::FakePlatform;
pub use platform::Platform;
pub use types::AppInfo;
pub use types::BAR_HEIGHT;
pub use types::RECT_EPSILON;
pub use types::Rect;
pub use types::SPLIT_GAP;
pub use types::MIN_PANE;
pub use types::Split;
pub use types::{clamp_ratio, ratio_for_left, ratio_limits, split_rects};
pub use types::Saved;
pub use types::Tab;
pub use types::TabId;
pub use types::WindowId;
pub use types::WindowInfo;
