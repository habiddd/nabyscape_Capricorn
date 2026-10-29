//! Battery state as the dock shows it (`GetSystemPowerStatus` on Windows; the C# build read
//! `SystemInformation.PowerStatus`). The host fills it; views only format it.

/// One reading of the system power status. `Default` is the "no battery information" form.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BatterySnapshot {
    /// A battery exists (false on desktops: the dock shows "Connected to power").
    pub present: bool,
    /// Charge 0..=100, `None` when Windows does not know.
    pub percent: Option<u8>,
    /// Plugged in (AC line online).
    pub ac_online: bool,
    /// Actively charging.
    pub charging: bool,
    /// Windows' estimate of the remaining runtime in minutes while on battery.
    pub minutes_remaining: Option<u32>,
    /// Battery saver is on.
    pub saver: bool,
}

impl BatterySnapshot {
    /// The status page line, e.g. `83% · Charging` / `41% · Battery`, or `Connected to power`
    /// without a battery.
    pub fn text(&self) -> String {
        if !self.present {
            return "Connected to power".to_owned();
        }
        let level = self
            .percent
            .map_or_else(|| "—".to_owned(), |p| format!("{p}%"));
        format!(
            "{level} · {}",
            if self.ac_online { "Charging" } else { "Battery" }
        )
    }

    /// The hover-peek line (`battery` page).
    pub fn peek_text(&self) -> String {
        match (self.present, self.percent) {
            (true, Some(p)) => format!("{p}% battery"),
            (true, None) => "Battery status unavailable".to_owned(),
            (false, _) => "Connected to power".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_status_page() {
        let desktop = BatterySnapshot::default();
        assert_eq!(desktop.text(), "Connected to power");
        assert_eq!(desktop.peek_text(), "Connected to power");
        let laptop = BatterySnapshot {
            present: true,
            percent: Some(83),
            ac_online: true,
            charging: true,
            ..Default::default()
        };
        assert_eq!(laptop.text(), "83% · Charging");
        assert_eq!(laptop.peek_text(), "83% battery");
        let unplugged = BatterySnapshot {
            ac_online: false,
            percent: Some(41),
            ..laptop
        };
        assert_eq!(unplugged.text(), "41% · Battery");
        let unknown = BatterySnapshot {
            percent: None,
            ..unplugged
        };
        assert_eq!(unknown.text(), "— · Battery");
    }
}
