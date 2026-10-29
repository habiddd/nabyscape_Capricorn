//! Motion, dock geometry and interaction policies (C# `Motion/*` and `UI/Island/*Policy`).
//!
//! Everything here is pure and time-sampled. Animations are values computed from `now`
//! (seconds on any monotonic clock the host chooses); each animated object reports when it next
//! needs a frame through `next_frame(now) -> Option<f64>` so the host can sleep when nothing
//! moves. No timers or threads live in this module.
//!
//! Units: sizes in device-independent pixels (DIP) unless a function says "px"; placement
//! functions take monitor rectangles in physical pixels plus a `dpi_scale` (1.0 = 96 dpi) and
//! return physical pixels.

pub mod ambient;
pub mod dock_editing;
pub mod easing;
pub mod feedback;
pub mod follow;
pub mod frame;
pub mod geometry;
pub mod hover;
pub mod navigation;
pub mod peek;
pub mod spring;
pub mod tuck;

pub use ambient::{
    ambient_audio_gain, bottom_fade, color_band, AmbientColors, AmbientLayer, AmbientLoop,
    AmbientMotion, BottomFade, ColorBand, FadeShape,
};
pub use dock_editing::{DockLayout, DockPositionModel, OverlaySettingsState};
pub use easing::{Ease, EaseKind, EasingMode, Interaction, Lerp, MotionConfig, Tween};
pub use feedback::{button_feedback, notice_entry, ButtonFeedback, NoticeEntry};
pub use follow::{
    hidden_tuck_edge_hit, monitor_at, DisplayFollow, FollowMove, FollowSlide, Monitor,
    MonitorFollowPolicy,
};
pub use frame::{frame_limit_for, FrameBudget, FramePacer, FrameTelemetry, Performance};
pub use geometry::{
    place_host, placement_area, Contour, DockState, HoverStage, IslandGeometry, PathCmd, RectF,
    RectI, SizeF,
};
pub use hover::{HoverAction, HoverInputs, HoverPolicy};
pub use navigation::{cycle_page, PanelNavigation, PanelSwipe, WheelNode};
pub use peek::{PeekLayout, PeekPolicy, PeekWheelAccumulator, VisibilityPolicy};
pub use spring::{Channel, Step, SurfaceMotion};
pub use tuck::{AutoTuckPolicy, TuckController, TuckOutcome};
