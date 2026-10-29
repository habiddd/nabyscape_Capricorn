//! Battery and power state from `GetSystemPowerStatus`.
//!
//! C# reads `SystemInformation.PowerStatus` (a `GetSystemPowerStatus` wrapper) whenever the
//! battery page or the hover text refreshes, and `SettingsBackdrop`/`FramePerformance` read the
//! AC line and battery-saver flag. There are no power-setting notifications in the C# build, so
//! none are registered here either: [`BatteryService::snapshot`] is a cheap direct read, and
//! nothing runs between calls.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::wake::Waker;

pub use naby_core::system::battery::{BatterySnapshot, RawPowerStatus};

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

    #[test]
    fn service_snapshot_is_cheap_and_total() {
        let service = BatteryService::start(crate::wake::noop());
        let _ = service.snapshot();
    }
}
