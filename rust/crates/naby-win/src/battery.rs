//! Battery and power state from `GetSystemPowerStatus`.
//!
//! C# reads `SystemInformation.PowerStatus` (a `GetSystemPowerStatus` wrapper) whenever the
//! battery page or the hover text refreshes, and `SettingsBackdrop`/`FramePerformance` read the
//! AC line and battery-saver flag. There are no power-setting notifications in the C# build, so
//! none are registered here either: [`BatteryService::snapshot`] is a cheap direct read, and
//! nothing runs between calls.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::wake::Waker;

/// Raw `SYSTEM_POWER_STATUS` fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawPowerStatus {
    pub ac_line_status: u8,
    pub battery_flag: u8,
    pub battery_life_percent: u8,
    pub system_status_flag: u8,
    pub battery_life_time: u32,
}

/// What the views show. `available == false` is the "unavailable" form (the API failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatterySnapshot {
    /// `GetSystemPowerStatus` succeeded.
    pub available: bool,
    /// A battery exists (the `NoSystemBattery` flag, 128, is clear and the state is known).
    pub has_battery: bool,
    /// `Some(true)` on AC power, `Some(false)` on battery, `None` when unknown.
    pub on_ac: Option<bool>,
    /// 0..=100, `None` when unknown (255).
    pub percent: Option<u8>,
    /// The battery is charging (flag 8).
    pub charging: bool,
    /// Battery saver is on (`SystemStatusFlag`).
    pub saver: bool,
    /// Estimated seconds left on battery.
    pub seconds_remaining: Option<u32>,
}

impl BatterySnapshot {
    /// The form shown when the OS cannot say.
    pub const UNAVAILABLE: BatterySnapshot = BatterySnapshot {
        available: false,
        has_battery: false,
        on_ac: None,
        percent: None,
        charging: false,
        saver: false,
        seconds_remaining: None,
    };

    /// Interprets a raw status the way the C# views do.
    pub fn from_raw(raw: RawPowerStatus) -> Self {
        const NO_BATTERY: u8 = 128;
        const UNKNOWN: u8 = 255;
        const CHARGING: u8 = 8;
        let has_battery = raw.battery_flag != UNKNOWN && raw.battery_flag & NO_BATTERY == 0;
        Self {
            available: true,
            has_battery,
            on_ac: match raw.ac_line_status {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            percent: (has_battery && raw.battery_life_percent <= 100)
                .then_some(raw.battery_life_percent),
            charging: has_battery
                && raw.battery_flag != UNKNOWN
                && raw.battery_flag & CHARGING != 0,
            saver: raw.system_status_flag != 0,
            seconds_remaining: (raw.battery_life_time != u32::MAX).then_some(raw.battery_life_time),
        }
    }

    /// C# `PowerLineStatus.Offline`: running on battery (frame budget drops to Hemat).
    pub fn on_battery(&self) -> bool {
        self.available && self.on_ac == Some(false)
    }

    /// Hover text (`ExtraHoverViews`): "Connected to power" without a battery, else "63% battery".
    pub fn hover_text(&self) -> String {
        match (self.has_battery, self.percent) {
            (true, Some(p)) => format!("{p}% battery"),
            _ => "Connected to power".to_owned(),
        }
    }

    /// Page text (`OverlayWindow`): "63% · Charging" while on AC, else "63% · Battery".
    pub fn status_text(&self) -> String {
        let percent = self
            .percent
            .map_or_else(|| "—".to_owned(), |p| format!("{p}%"));
        format!(
            "{percent} · {}",
            if self.on_ac == Some(true) {
                "Charging"
            } else {
                "Battery"
            }
        )
    }

    /// The battery card is shown only when a battery exists.
    pub fn visible(&self) -> bool {
        self.available && self.has_battery
    }
}

/// Reads the power status on demand. Holds no thread or handle, so dropping it releases nothing.
pub struct BatteryService {
    _wake: Waker,
}

impl BatteryService {
    pub fn start(wake: Waker) -> Self {
        Self { _wake: wake }
    }

    /// A fresh reading (a single cheap system call).
    pub fn snapshot(&self) -> BatterySnapshot {
        read()
    }
}

#[cfg(windows)]
fn read() -> BatterySnapshot {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS for the duration of the call.
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return BatterySnapshot::UNAVAILABLE;
    }
    BatterySnapshot::from_raw(RawPowerStatus {
        ac_line_status: status.ACLineStatus,
        battery_flag: status.BatteryFlag,
        battery_life_percent: status.BatteryLifePercent,
        system_status_flag: status.SystemStatusFlag,
        battery_life_time: status.BatteryLifeTime,
    })
}

#[cfg(not(windows))]
fn read() -> BatterySnapshot {
    BatterySnapshot::UNAVAILABLE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(ac: u8, flag: u8, percent: u8) -> RawPowerStatus {
        RawPowerStatus {
            ac_line_status: ac,
            battery_flag: flag,
            battery_life_percent: percent,
            system_status_flag: 0,
            battery_life_time: u32::MAX,
        }
    }

    #[test]
    fn desktop_without_battery_is_connected_to_power_and_hidden() {
        let s = BatterySnapshot::from_raw(raw(1, 128, 255));
        assert!(s.available && !s.has_battery && !s.visible());
        assert_eq!(s.percent, None);
        assert_eq!(s.hover_text(), "Connected to power");
        assert!(!s.on_battery());
    }

    #[test]
    fn laptop_on_battery_and_charging() {
        let discharging = BatterySnapshot::from_raw(raw(0, 1, 63));
        assert!(discharging.visible() && discharging.on_battery() && !discharging.charging);
        assert_eq!(discharging.status_text(), "63% · Battery");
        assert_eq!(discharging.hover_text(), "63% battery");
        let charging = BatterySnapshot::from_raw(raw(1, 9, 80));
        assert!(charging.charging && !charging.on_battery());
        assert_eq!(charging.status_text(), "80% · Charging");
    }

    #[test]
    fn unknown_values_never_become_fabricated_numbers() {
        let s = BatterySnapshot::from_raw(raw(255, 255, 255));
        assert!(!s.has_battery && s.on_ac.is_none() && s.percent.is_none() && !s.on_battery());
        assert_eq!(s.status_text(), "— · Battery");
        let gone = BatterySnapshot::UNAVAILABLE;
        assert!(!gone.available && !gone.visible() && !gone.on_battery());
    }

    #[test]
    fn saver_flag_and_remaining_time() {
        let mut r = raw(0, 1, 50);
        r.system_status_flag = 1;
        r.battery_life_time = 5400;
        let s = BatterySnapshot::from_raw(r);
        assert!(s.saver);
        assert_eq!(s.seconds_remaining, Some(5400));
    }

    #[test]
    fn service_snapshot_is_cheap_and_total() {
        let service = BatteryService::start(crate::wake::noop());
        let _ = service.snapshot();
    }
}
