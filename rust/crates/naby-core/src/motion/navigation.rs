//! Panel page navigation: wheel, swipe and cycling (C# `UI/Island/PanelNavigation.cs` and
//! `CyclePage` in `EnhancedInteraction.cs`).

use super::peek::PeekWheelAccumulator;
use crate::settings::Settings;

/// An ancestor of the wheel event source, innermost first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelNode {
    /// Inside the countdown editor (its text boxes still page).
    CountdownView,
    /// A text box or password box.
    TextInput,
    /// The panel navigation strip (`Tag == "panel-navigation"`): stops the ancestor walk for
    /// vertical wheels so the outer viewport cannot steal them.
    NavigationRegion,
    /// A scroll viewer. `content_scroll` is `Tag == "panel-content-scroll"`. `exempt` is the
    /// outer panel viewport on the wide Focus page with under 48 DIP of overflow.
    ScrollViewer {
        content_scroll: bool,
        scrollable_height: f64,
        exempt: bool,
    },
    Other,
}

/// Result of routing a wheel event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelOutcome {
    /// The event was consumed (do not scroll natively).
    pub handled: bool,
    /// Page direction to cycle (`-1`, `0`, `1`).
    pub direction: i32,
}

/// What the wheel does for the current surface (C# `PreviewMouseWheel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelTarget {
    PeekCarousel,
    PanelPages,
    Volume,
    Pages,
    None,
}

/// Wheel / swipe page navigation of the expanded panel.
#[derive(Debug, Clone, Default)]
pub struct PanelNavigation {
    wheel: PeekWheelAccumulator,
}

impl PanelNavigation {
    pub fn new() -> Self {
        Self::default()
    }

    /// C# `CanPanelSwipe`.
    pub fn can_swipe(
        s: &Settings,
        expanded: bool,
        command_open: bool,
        assistant_open: bool,
    ) -> bool {
        expanded && s.compact_layout && s.panel_swipe && !command_open && !assistant_open
    }

    /// Which handler a wheel event on the closed / peeking / expanded dock goes to.
    pub fn wheel_target(s: &Settings, peeking: bool, expanded: bool) -> WheelTarget {
        if peeking {
            WheelTarget::PeekCarousel
        } else if expanded {
            WheelTarget::PanelPages
        } else {
            match s.wheel_action.as_str() {
                "volume" => WheelTarget::Volume,
                "pages" => WheelTarget::Pages,
                _ => WheelTarget::None,
            }
        }
    }

    /// C# `RoutePanelWheel`. `source` is the ancestor chain of the wheel target, innermost
    /// first; `media_controls_own_it` is set when the media popover contains the source.
    #[allow(clippy::too_many_arguments)]
    pub fn route_wheel(
        &mut self,
        can_swipe: bool,
        delta: i32,
        horizontal: bool,
        source: &[WheelNode],
        media_controls_own_it: bool,
        now_ms: i64,
    ) -> WheelOutcome {
        let ignored = WheelOutcome {
            handled: false,
            direction: 0,
        };
        if !can_swipe {
            return ignored;
        }
        let countdown_editor = source.contains(&WheelNode::CountdownView);
        if media_controls_own_it {
            return ignored;
        }
        for node in source {
            match *node {
                WheelNode::TextInput if !countdown_editor => return ignored,
                WheelNode::NavigationRegion if !horizontal => break,
                WheelNode::ScrollViewer {
                    content_scroll,
                    scrollable_height,
                    exempt,
                } if !horizontal && (content_scroll || scrollable_height > 1.0 && !exempt) => {
                    return ignored;
                }
                _ => {}
            }
        }
        let direction = self
            .wheel
            .push(if horizontal { delta } else { -delta }, now_ms);
        WheelOutcome {
            handled: true,
            direction,
        }
    }

    pub fn reset(&mut self) {
        self.wheel.reset();
    }
}

/// Click-drag swipe between pages (C# `InitializePanelSwipe`).
#[derive(Debug, Clone, Default)]
pub struct PanelSwipe {
    down: Option<(f64, f64)>,
    swiped: bool,
}

impl PanelSwipe {
    pub fn new() -> Self {
        Self::default()
    }

    /// Left button pressed at `pos`. `blocked` when the press landed on a button, text box,
    /// slider, scroll bar, combo box or a file-drag / media-popover region.
    pub fn press(&mut self, pos: (f64, f64), can_swipe: bool, blocked: bool) {
        self.down = None;
        self.swiped = false;
        if can_swipe && !blocked {
            self.down = Some(pos);
        }
    }

    /// Pointer moved with the left button held. Returns the page direction once the horizontal
    /// distance passes `SwipeDistance`; a mostly vertical drag (over 10 DIP) cancels.
    pub fn moved(
        &mut self,
        pos: (f64, f64),
        left_pressed: bool,
        can_swipe: bool,
        swipe_distance: f64,
    ) -> Option<i32> {
        let down = self.down?;
        if !can_swipe || !left_pressed {
            return None;
        }
        let (dx, dy) = (pos.0 - down.0, pos.1 - down.1);
        if dy.abs() > dx.abs() && dy.abs() > 10.0 {
            self.down = None;
            return None;
        }
        if dx.abs() < swipe_distance {
            return None;
        }
        self.down = None;
        self.swiped = true;
        Some(if dx < 0.0 { 1 } else { -1 })
    }

    /// Button released. `true` when the release ends a swipe and must not click.
    pub fn release(&mut self) -> bool {
        self.down = None;
        std::mem::take(&mut self.swiped)
    }

    pub fn cancel(&mut self) {
        self.down = None;
    }

    pub fn active(&self) -> bool {
        self.down.is_some()
    }
}

/// C# `CyclePage`: next page key with wrap-around (`None` without pages).
pub fn cycle_page(keys: &[String], active: &str, direction: i32) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let index = keys
        .iter()
        .position(|k| k == active)
        .map_or(-1, |i| i as i64);
    let next = (index + i64::from(direction) + keys.len() as i64).rem_euclid(keys.len() as i64);
    Some(keys[next as usize].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        ["media", "timer", "notes"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn cycle_wraps_both_ways() {
        let k = keys();
        assert_eq!(cycle_page(&k, "media", 1).as_deref(), Some("timer"));
        assert_eq!(cycle_page(&k, "notes", 1).as_deref(), Some("media"));
        assert_eq!(cycle_page(&k, "media", -1).as_deref(), Some("notes"));
        assert_eq!(cycle_page(&k, "gone", 1).as_deref(), Some("media"));
        assert_eq!(
            cycle_page(&["only".to_string()], "only", -1).as_deref(),
            Some("only")
        );
        assert_eq!(cycle_page(&[], "x", 1), None);
    }

    #[test]
    fn swipe_availability() {
        let s = Settings {
            compact_layout: true,
            panel_swipe: true,
            ..Settings::default()
        };
        assert!(PanelNavigation::can_swipe(&s, true, false, false));
        assert!(!PanelNavigation::can_swipe(&s, false, false, false));
        assert!(!PanelNavigation::can_swipe(&s, true, true, false));
        assert!(!PanelNavigation::can_swipe(&s, true, false, true));
        let off = Settings {
            panel_swipe: false,
            ..s.clone()
        };
        assert!(!PanelNavigation::can_swipe(&off, true, false, false));
        let classic = Settings {
            compact_layout: false,
            ..s
        };
        assert!(!PanelNavigation::can_swipe(&classic, true, false, false));
    }

    #[test]
    fn wheel_routes_to_the_right_handler() {
        let s = Settings {
            wheel_action: "volume".into(),
            ..Settings::default()
        };
        assert_eq!(
            PanelNavigation::wheel_target(&s, true, true),
            WheelTarget::PeekCarousel
        );
        assert_eq!(
            PanelNavigation::wheel_target(&s, false, true),
            WheelTarget::PanelPages
        );
        assert_eq!(
            PanelNavigation::wheel_target(&s, false, false),
            WheelTarget::Volume
        );
        let pages = Settings {
            wheel_action: "pages".into(),
            ..Settings::default()
        };
        assert_eq!(
            PanelNavigation::wheel_target(&pages, false, false),
            WheelTarget::Pages
        );
        let none = Settings {
            wheel_action: "none".into(),
            ..Settings::default()
        };
        assert_eq!(
            PanelNavigation::wheel_target(&none, false, false),
            WheelTarget::None
        );
    }

    #[test]
    fn horizontal_wheel_pages_once_per_gesture() {
        let mut n = PanelNavigation::new();
        let r = n.route_wheel(true, 120, true, &[WheelNode::Other], false, 1000);
        assert_eq!(
            r,
            WheelOutcome {
                handled: true,
                direction: 1
            }
        );
        let r = n.route_wheel(true, 120, true, &[WheelNode::Other], false, 1050);
        assert_eq!(
            r,
            WheelOutcome {
                handled: true,
                direction: 0
            },
            "same gesture"
        );
        let r = n.route_wheel(true, -120, true, &[WheelNode::Other], false, 1500);
        assert_eq!(r.direction, -1);
    }

    #[test]
    fn vertical_wheel_pages_inverted_when_nothing_scrolls() {
        let mut n = PanelNavigation::new();
        // WPF wheel up is positive; scrolling up pages backwards.
        let r = n.route_wheel(true, -120, false, &[WheelNode::Other], false, 0);
        assert_eq!(
            r,
            WheelOutcome {
                handled: true,
                direction: 1
            }
        );
    }

    #[test]
    fn nested_content_keeps_its_wheel() {
        let mut n = PanelNavigation::new();
        let scrolling = WheelNode::ScrollViewer {
            content_scroll: false,
            scrollable_height: 40.0,
            exempt: false,
        };
        assert!(
            !n.route_wheel(true, 120, false, &[scrolling], false, 0)
                .handled
        );
        let marked = WheelNode::ScrollViewer {
            content_scroll: true,
            scrollable_height: 0.0,
            exempt: false,
        };
        assert!(!n.route_wheel(true, 120, false, &[marked], false, 0).handled);
        // The outer viewport on the wide Focus page is exempt, and a barely scrollable one too.
        let exempt = WheelNode::ScrollViewer {
            content_scroll: false,
            scrollable_height: 30.0,
            exempt: true,
        };
        assert!(
            n.route_wheel(true, 120, false, &[exempt], false, 10_000)
                .handled
        );
        let flat = WheelNode::ScrollViewer {
            content_scroll: false,
            scrollable_height: 1.0,
            exempt: false,
        };
        assert!(
            n.route_wheel(true, 120, false, &[flat], false, 20_000)
                .handled
        );
        // Horizontal wheels ignore scroll viewers entirely.
        assert!(
            n.route_wheel(true, 120, true, &[scrolling], false, 30_000)
                .handled
        );
    }

    #[test]
    fn navigation_region_stops_the_walk_for_vertical_wheels() {
        let mut n = PanelNavigation::new();
        let scrolling = WheelNode::ScrollViewer {
            content_scroll: false,
            scrollable_height: 400.0,
            exempt: false,
        };
        let chain = [WheelNode::NavigationRegion, scrolling];
        assert!(n.route_wheel(true, 120, false, &chain, false, 0).handled);
        let chain = [scrolling, WheelNode::NavigationRegion];
        assert!(!n.route_wheel(true, 120, false, &chain, false, 5000).handled);
    }

    #[test]
    fn text_inputs_keep_the_wheel_except_the_countdown_editor() {
        let mut n = PanelNavigation::new();
        assert!(
            !n.route_wheel(true, 120, true, &[WheelNode::TextInput], false, 0)
                .handled
        );
        let countdown = [WheelNode::TextInput, WheelNode::CountdownView];
        assert!(n.route_wheel(true, 120, true, &countdown, false, 0).handled);
    }

    #[test]
    fn disabled_or_media_popover_ignores_the_wheel() {
        let mut n = PanelNavigation::new();
        assert!(!n.route_wheel(false, 120, true, &[], false, 0).handled);
        assert!(!n.route_wheel(true, 120, true, &[], true, 0).handled);
        n.reset();
    }

    #[test]
    fn swipe_distance_and_direction() {
        let mut sw = PanelSwipe::new();
        sw.press((100.0, 100.0), true, false);
        assert_eq!(sw.moved((120.0, 102.0), true, true, 48.0), None);
        assert_eq!(
            sw.moved((50.0, 102.0), true, true, 48.0),
            Some(1),
            "swipe left = next"
        );
        assert!(sw.release(), "the release after a swipe is swallowed");
        assert!(!sw.release());
        sw.press((100.0, 100.0), true, false);
        assert_eq!(
            sw.moved((160.0, 100.0), true, true, 48.0),
            Some(-1),
            "swipe right = previous"
        );
    }

    #[test]
    fn swipe_cancels_on_vertical_drag_blocked_press_or_released_button() {
        let mut sw = PanelSwipe::new();
        sw.press((0.0, 0.0), true, false);
        assert_eq!(sw.moved((5.0, 30.0), true, true, 48.0), None);
        assert!(!sw.active(), "vertical drag cancels");
        assert_eq!(sw.moved((200.0, 30.0), true, true, 48.0), None);
        sw.press((0.0, 0.0), true, true);
        assert!(!sw.active(), "buttons and sliders never start a swipe");
        sw.press((0.0, 0.0), false, false);
        assert!(!sw.active());
        sw.press((0.0, 0.0), true, false);
        assert_eq!(
            sw.moved((200.0, 0.0), false, true, 48.0),
            None,
            "button released"
        );
        assert_eq!(
            sw.moved((200.0, 0.0), true, false, 48.0),
            None,
            "swipe disabled"
        );
        sw.cancel();
        assert!(!sw.active());
        // Small vertical wobble under 10 DIP does not cancel.
        sw.press((0.0, 0.0), true, false);
        assert_eq!(sw.moved((20.0, 9.0), true, true, 48.0), None);
        assert!(sw.active());
    }
}
