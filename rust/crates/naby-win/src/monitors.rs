//! Monitor enumeration, selection and dock placement (C# `Native.ChooseScreen`,
//! `MonitorFollowPolicy`, `DisplayFollow`).
//!
//! Everything but the `os` module is pure so the selection and placement rules are tested on
//! Linux. A monitor's stable id is its GDI device name (`\\.\DISPLAY1`), the same string C#
//! stores in `Settings.Monitor` (`Screen.DeviceName`).

use naby_core::settings::{DisplayMode, DockEdge, Settings};

/// Integer rectangle in virtual-screen pixels (right / bottom exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectI {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl RectI {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    pub fn center(&self) -> (i32, i32) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }
    /// Squared distance from a point to the rectangle (0 inside).
    pub fn distance_sq(&self, x: i32, y: i32) -> i64 {
        let dx = (self.left - x).max(0).max(x - (self.right - 1)) as i64;
        let dy = (self.top - y).max(0).max(y - (self.bottom - 1)) as i64;
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// GDI device name, the persisted `Settings.Monitor` value.
    pub id: String,
    pub bounds: RectI,
    /// Bounds without the taskbar and docked tool bars.
    pub work: RectI,
    /// Effective DPI (96 = 100%).
    pub dpi: u32,
    pub primary: bool,
    /// `HMONITOR` value, for matching windows to monitors.
    pub handle: isize,
}

impl MonitorInfo {
    pub fn scale(&self) -> f32 {
        self.dpi.max(1) as f32 / 96.0
    }

    /// Short name for pickers, e.g. `DISPLAY2 · 2560x1440 (primary)`.
    pub fn label(&self) -> String {
        let name = self.id.trim_start_matches('\\').trim_start_matches('.');
        let name = name.trim_start_matches('\\');
        format!(
            "{name} \u{b7} {}x{}{}",
            self.bounds.width(),
            self.bounds.height(),
            if self.primary { " (primary)" } else { "" }
        )
    }
}

fn primary_index(monitors: &[MonitorInfo]) -> usize {
    monitors.iter().position(|m| m.primary).unwrap_or(0)
}

/// Which monitor hosts the dock (C# `MonitorFollowPolicy.Target` + `Native.ChooseScreen`).
/// `followed` is the monitor the pointer last moved the dock to in `Follow` mode. Unknown ids
/// fall back to the primary monitor. Returns `None` only for an empty list.
pub fn choose_monitor(
    monitors: &[MonitorInfo],
    mode: DisplayMode,
    stored: &str,
    followed: Option<&str>,
) -> Option<usize> {
    if monitors.is_empty() {
        return None;
    }
    let primary = primary_index(monitors);
    let preferred = match mode {
        DisplayMode::Follow => followed.unwrap_or(stored),
        DisplayMode::Primary => monitors[primary].id.as_str(),
        DisplayMode::Selected => stored,
    };
    Some(
        monitors
            .iter()
            .position(|m| m.id == preferred)
            .unwrap_or(primary),
    )
}

/// The monitor containing a point, or the nearest one when the point is outside all of them
/// (cursor on a just-unplugged display).
pub fn monitor_at(monitors: &[MonitorInfo], x: i32, y: i32) -> Option<usize> {
    monitors
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| m.bounds.distance_sq(x, y))
        .map(|(i, _)| i)
}

/// Debounce for `Follow` mode: the pointer must stay on another monitor for 250 ms before the
/// dock moves, unless `immediate` (C# `MonitorFollowPolicy.Observe`).
#[derive(Debug, Default)]
pub struct FollowPolicy {
    candidate: String,
    since_ms: i64,
}

impl FollowPolicy {
    pub const DWELL_MS: i64 = 250;

    pub fn reset(&mut self) {
        self.candidate.clear();
        self.since_ms = 0;
    }

    /// Returns the monitor id to move to, if any.
    pub fn observe(
        &mut self,
        current: &str,
        pointer: &str,
        now_ms: i64,
        blocked: bool,
        hidden: bool,
        immediate: bool,
    ) -> Option<String> {
        if hidden || blocked || pointer == current {
            self.reset();
            return None;
        }
        if immediate {
            self.reset();
            return Some(pointer.to_string());
        }
        if self.candidate != pointer {
            self.candidate = pointer.to_string();
            self.since_ms = now_ms;
            return None;
        }
        if now_ms - self.since_ms < Self::DWELL_MS {
            return None;
        }
        self.reset();
        Some(pointer.to_string())
    }
}

/// The rectangle the dock aligns to: the full monitor for the top edge (the dock may overlap
/// the taskbar strip), the work area for every other edge.
pub fn dock_area(monitor: &MonitorInfo, edge: DockEdge) -> RectI {
    if edge == DockEdge::Top {
        monitor.bounds
    } else {
        monitor.work
    }
}

/// Top-left of a dock window of `width` x `height` pixels whose painted pill is
/// `painted_width` wide (C# `MonitorFollowPolicy.Place`). `scale` is the monitor DPI scale.
pub fn place_dock(
    s: &Settings,
    area: RectI,
    scale: f64,
    width: i32,
    height: i32,
    tucked: bool,
    painted_width: i32,
) -> (i32, i32) {
    let compact = s.width * scale;
    let painted = if painted_width > 0 {
        painted_width
    } else {
        width
    };
    let inset = ((width - painted) / 2).max(0);
    let gap = (s.dock_inset * scale).round_ties_even() as i32;
    let edge = s.dock_edge();
    let mut x = area.left
        + ((area.width() as f64 - compact) * s.horizontal / 100.0 + compact / 2.0
            - width as f64 / 2.0) as i32;
    let top_offset = if tucked || (s.preset == "Mac" && edge == DockEdge::Top) {
        0.0
    } else {
        s.top
    };
    let mut y = area.top + (top_offset * scale) as i32;
    match edge {
        DockEdge::Bottom => y = area.bottom - height - if tucked { 0 } else { gap },
        DockEdge::Left | DockEdge::Right => {
            x = if edge == DockEdge::Left {
                area.left - inset + if tucked { 0 } else { gap }
            } else {
                area.right - painted - inset - if tucked { 0 } else { gap }
            };
            y = area.top + ((area.height() - height) as f64 * s.dock_vertical / 100.0) as i32;
        }
        _ => {}
    }
    let min_x = area.left - inset;
    let max_x = min_x.max(area.right - width + inset);
    let max_y = area.top.max(area.bottom - height);
    (x.clamp(min_x, max_x), y.clamp(area.top, max_y))
}

#[cfg(windows)]
pub use os::*;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::util::{from_wide, wide};
    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, MonitorFromWindow, DEVMODEW,
        ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
        MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    fn rect(r: RECT) -> RectI {
        RectI::new(r.left, r.top, r.right, r.bottom)
    }

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<MonitorInfo>` passed by `enumerate`, alive for the
        // whole synchronous EnumDisplayMonitors call.
        let list = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
        if let Some(info) = monitor_info(monitor) {
            list.push(info);
        }
        BOOL(1)
    }

    fn monitor_info(monitor: HMONITOR) -> Option<MonitorInfo> {
        let mut raw = MONITORINFOEXW::default();
        raw.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `raw` is a properly sized MONITORINFOEXW whose cbSize is set.
        let ok = unsafe { GetMonitorInfoW(monitor, &mut raw as *mut _ as *mut MONITORINFO) };
        if !ok.as_bool() {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        // SAFETY: out-pointers reference live locals. Failure keeps the 96 default.
        let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
        Some(MonitorInfo {
            id: from_wide(&raw.szDevice),
            bounds: rect(raw.monitorInfo.rcMonitor),
            work: rect(raw.monitorInfo.rcWork),
            dpi: dpi_x.max(48),
            primary: raw.monitorInfo.dwFlags & 1 != 0,
            handle: monitor.0 as isize,
        })
    }

    /// All attached monitors in the order Windows enumerates them.
    pub fn enumerate() -> Vec<MonitorInfo> {
        let mut list: Vec<MonitorInfo> = Vec::new();
        // SAFETY: the callback only touches the Vec behind `data`, which outlives the call.
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(collect),
                LPARAM(&mut list as *mut Vec<MonitorInfo> as isize),
            );
        }
        list
    }

    /// Current pointer position in virtual-screen pixels.
    pub fn cursor_pos() -> Option<(i32, i32)> {
        let mut p = POINT::default();
        // SAFETY: out-pointer references a live local.
        unsafe { GetCursorPos(&mut p) }.ok()?;
        Some((p.x, p.y))
    }

    /// Index of the monitor under the pointer.
    pub fn monitor_under_cursor(monitors: &[MonitorInfo]) -> Option<usize> {
        let (x, y) = cursor_pos()?;
        monitor_at(monitors, x, y)
    }

    /// `HMONITOR` (as `isize`) the window is mostly on.
    pub fn monitor_of_window(hwnd: HWND) -> isize {
        // SAFETY: plain query on a window handle; stale handles yield the nearest monitor.
        unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }.0 as isize
    }

    /// Refresh rate in Hz of a display device (0 when unknown), for pacing animation.
    pub fn refresh_rate(device_id: &str) -> u32 {
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let name = wide(device_id);
        // SAFETY: `name` is NUL-terminated and `mode` has dmSize set.
        let ok = unsafe {
            EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode)
        };
        if ok.as_bool() {
            mode.dmDisplayFrequency
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: &str, x: i32, w: i32, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: id.into(),
            bounds: RectI::new(x, 0, x + w, 1080),
            work: RectI::new(x, 0, x + w, 1040),
            dpi: 96,
            primary,
            handle: 0,
        }
    }

    fn two() -> Vec<MonitorInfo> {
        vec![
            mon(r"\\.\DISPLAY2", -1920, 1920, false),
            mon(r"\\.\DISPLAY1", 0, 1920, true),
        ]
    }

    #[test]
    fn primary_mode_ignores_the_stored_monitor() {
        let m = two();
        assert_eq!(
            choose_monitor(&m, DisplayMode::Primary, r"\\.\DISPLAY2", None),
            Some(1)
        );
    }

    #[test]
    fn selected_mode_uses_stored_and_falls_back_to_primary_when_unplugged() {
        let m = two();
        assert_eq!(
            choose_monitor(&m, DisplayMode::Selected, r"\\.\DISPLAY2", None),
            Some(0)
        );
        assert_eq!(
            choose_monitor(&m, DisplayMode::Selected, r"\\.\DISPLAY9", None),
            Some(1)
        );
        assert_eq!(choose_monitor(&m, DisplayMode::Selected, "", None), Some(1));
    }

    #[test]
    fn follow_mode_prefers_followed_then_stored_then_primary() {
        let m = two();
        assert_eq!(
            choose_monitor(
                &m,
                DisplayMode::Follow,
                r"\\.\DISPLAY1",
                Some(r"\\.\DISPLAY2")
            ),
            Some(0)
        );
        assert_eq!(
            choose_monitor(&m, DisplayMode::Follow, r"\\.\DISPLAY2", None),
            Some(0)
        );
        assert_eq!(
            choose_monitor(&m, DisplayMode::Follow, "", Some("gone")),
            Some(1)
        );
        assert_eq!(choose_monitor(&[], DisplayMode::Primary, "", None), None);
    }

    #[test]
    fn no_flagged_primary_still_picks_the_first() {
        let mut m = two();
        m[1].primary = false;
        assert_eq!(choose_monitor(&m, DisplayMode::Primary, "", None), Some(0));
    }

    #[test]
    fn monitor_at_handles_edges_and_gaps() {
        let m = two();
        assert_eq!(monitor_at(&m, 10, 10), Some(1));
        assert_eq!(monitor_at(&m, -1, 10), Some(0));
        assert_eq!(monitor_at(&m, 5000, 500), Some(1));
        assert_eq!(monitor_at(&m, -5000, 500), Some(0));
        assert_eq!(monitor_at(&[], 0, 0), None);
    }

    #[test]
    fn follow_needs_a_250ms_dwell_and_resets_on_return() {
        let mut p = FollowPolicy::default();
        assert_eq!(p.observe("A", "B", 1000, false, false, false), None);
        assert_eq!(p.observe("A", "B", 1200, false, false, false), None);
        assert_eq!(
            p.observe("A", "B", 1250, false, false, false),
            Some("B".into())
        );
        // Moving away before the dwell restarts the clock.
        assert_eq!(p.observe("A", "B", 2000, false, false, false), None);
        assert_eq!(p.observe("A", "A", 2100, false, false, false), None);
        assert_eq!(p.observe("A", "B", 2200, false, false, false), None);
        assert_eq!(p.observe("A", "B", 2300, false, false, false), None);
        // Blocked (user interacting) or hidden never moves.
        assert_eq!(p.observe("A", "B", 9000, true, false, false), None);
        assert_eq!(p.observe("A", "B", 9000, false, true, true), None);
        assert_eq!(
            p.observe("A", "B", 9000, false, false, true),
            Some("B".into())
        );
    }

    fn settings() -> Settings {
        Settings {
            width: 228.0,
            height: 32.0,
            horizontal: 50.0,
            top: 8.0,
            dock_inset: 12.0,
            dock_vertical: 50.0,
            preset: "Classic".into(),
            dock_edge: "Top".into(),
            ..Settings::default()
        }
    }

    #[test]
    fn top_dock_is_centered_with_the_top_offset() {
        let s = settings();
        let area = RectI::new(0, 0, 1920, 1080);
        assert_eq!(
            place_dock(&s, area, 1.0, 228, 32, false, 0),
            ((1920 - 228) / 2, 8)
        );
        // 150% scale scales width and offset.
        assert_eq!(
            place_dock(&s, RectI::new(0, 0, 2880, 1620), 1.5, 342, 48, false, 0),
            ((2880 - 342) / 2, 12)
        );
        // Tucked ignores the top offset.
        assert_eq!(place_dock(&s, area, 1.0, 228, 32, true, 0).1, 0);
    }

    #[test]
    fn mac_notch_preset_sits_flush_with_the_top() {
        let mut s = settings();
        s.preset = "Mac".into();
        assert_eq!(
            place_dock(&s, RectI::new(0, 0, 1920, 1080), 1.0, 228, 32, false, 0).1,
            0
        );
    }

    #[test]
    fn horizontal_extremes_clamp_to_the_monitor() {
        let mut s = settings();
        let area = RectI::new(100, 0, 2020, 1080);
        s.horizontal = 0.0;
        assert_eq!(place_dock(&s, area, 1.0, 228, 32, false, 0).0, 100);
        s.horizontal = 100.0;
        assert_eq!(place_dock(&s, area, 1.0, 228, 32, false, 0).0, 2020 - 228);
        // A window wider than its pill may overhang by the transparent inset.
        s.horizontal = 0.0;
        assert_eq!(place_dock(&s, area, 1.0, 300, 32, false, 228).0, 100 - 36);
    }

    #[test]
    fn bottom_and_side_edges_use_the_gap() {
        let mut s = settings();
        let area = RectI::new(0, 0, 1920, 1040);
        s.dock_edge = "Bottom".into();
        assert_eq!(
            place_dock(&s, area, 1.0, 228, 32, false, 0).1,
            1040 - 32 - 12
        );
        assert_eq!(place_dock(&s, area, 1.0, 228, 32, true, 0).1, 1040 - 32);
        s.dock_edge = "Left".into();
        let (x, y) = place_dock(&s, area, 1.0, 228, 32, false, 0);
        assert_eq!((x, y), (12, (1040 - 32) / 2));
        s.dock_edge = "Right".into();
        assert_eq!(
            place_dock(&s, area, 1.0, 228, 32, false, 0).0,
            1920 - 228 - 12
        );
    }

    #[test]
    fn dock_area_is_full_bounds_only_for_the_top_edge() {
        let m = mon("a", 0, 1920, true);
        assert_eq!(dock_area(&m, DockEdge::Top), m.bounds);
        assert_eq!(dock_area(&m, DockEdge::Bottom), m.work);
    }

    #[test]
    fn labels_are_readable() {
        let m = mon(r"\\.\DISPLAY1", 0, 1920, true);
        assert_eq!(m.label(), "DISPLAY1 \u{b7} 1920x1080 (primary)");
        assert_eq!(mon("x", 0, 800, false).scale(), 1.0);
    }
}
