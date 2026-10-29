//! Ported from `Features/System/{SystemMonitor,HardwareMonitor,SystemMonitorOverlay}.cs`:
//! snapshot records, rate/percent/byte formatting and the sampling policy. The OS reads
//! (`GetSystemTimes`, `GlobalMemoryStatusEx`, network/drive/process enumeration, WMI) are the
//! Windows host's job; it fills a [`MonitorSnapshot`] and this module interprets it.

pub mod hardware;
#[cfg(test)]
mod tests;

use std::time::Duration;

pub use hardware::{
    adapter_id, percent as hardware_percent, sensor_value, utilization, CaptureOutcome, Cell,
    GpuEngine, HardwareCaptureWorker, HardwareCpu, HardwareGpu, HardwareReader, HardwareSensor,
    MonitorHardware, Row, Slot,
};

/// Seconds between background samples while the system page is visible.
pub const SAMPLE_INTERVAL_SECONDS: f64 = 2.0;
/// CPU history points kept for the sparkline.
pub const CPU_HISTORY_LEN: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorNetwork {
    pub id: String,
    pub name: String,
    pub received: i64,
    pub sent: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorDrive {
    pub name: String,
    pub free: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorProcess {
    pub name: String,
    pub id: u32,
    pub working_set: i64,
}

/// One reading of the machine. Every field the OS could not provide is `None` / empty and the
/// views render it as "Unavailable" or "Sampling…", never as a fabricated zero.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorSnapshot {
    /// Monotonic timestamp in `ticks_per_second` units chosen by the host.
    pub timestamp: i64,
    /// Idle and total (kernel + user) CPU time from `GetSystemTimes`.
    pub idle: Option<i64>,
    pub cpu_total: Option<i64>,
    pub available: Option<u64>,
    pub total: Option<u64>,
    pub uptime: Duration,
    pub process_count: Option<usize>,
    pub thread_count: Option<usize>,
    pub partial_threads: bool,
    pub networks: Vec<MonitorNetwork>,
    pub drives: Vec<MonitorDrive>,
    pub processes: Vec<MonitorProcess>,
    pub errors: Vec<String>,
    pub hardware: Option<MonitorHardware>,
}

impl MonitorSnapshot {
    /// The "nothing could be read" form: every reading absent, one explanatory error.
    pub fn unavailable(reason: &str) -> Self {
        Self {
            timestamp: 0,
            idle: None,
            cpu_total: None,
            available: None,
            total: None,
            uptime: Duration::ZERO,
            process_count: None,
            thread_count: None,
            partial_threads: false,
            networks: Vec::new(),
            drives: Vec::new(),
            processes: Vec::new(),
            errors: vec![reason.to_owned()],
            hardware: None,
        }
    }

    /// Bytes in use (`total - available`), when both are known and consistent.
    pub fn memory_used(&self) -> Option<u64> {
        match (self.total, self.available) {
            (Some(total), Some(available)) if available <= total => Some(total - available),
            _ => None,
        }
    }

    /// "used / total" memory text, or "Unavailable".
    pub fn memory_text(&self) -> String {
        match (self.memory_used(), self.total) {
            (Some(used), Some(total)) => {
                format!("{} / {}", bytes(used as f64), bytes(total as f64))
            }
            _ => "Unavailable".to_owned(),
        }
    }
}

/// One process as seen by the host; `None` fields are reads that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessProbe {
    pub name: Option<String>,
    pub id: u32,
    pub working_set: Option<i64>,
    pub threads: Option<usize>,
}

/// Result of [`capture_processes`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessCapture {
    pub count: Option<usize>,
    pub threads: Option<usize>,
    pub partial: bool,
    pub top: Vec<MonitorProcess>,
}

/// C# `CaptureProcesses`: counts processes and threads, and the five largest working sets when
/// `include_top`. A failed enumeration leaves everything unavailable; a failed per-process
/// thread read keeps the count and marks the thread total as a lower bound.
pub fn capture_processes<E>(
    include_top: bool,
    enumerate: impl FnOnce() -> Result<Vec<ProcessProbe>, E>,
) -> ProcessCapture {
    let Ok(list) = enumerate() else {
        return ProcessCapture::default();
    };
    let (mut threads, mut partial) = (0usize, false);
    let mut top = Vec::new();
    for p in &list {
        match p.threads {
            Some(n) => threads += n,
            None => partial = true,
        }
        if include_top {
            if let (Some(name), Some(ws)) = (&p.name, p.working_set) {
                top.push(MonitorProcess {
                    name: name.clone(),
                    id: p.id,
                    working_set: ws,
                });
            }
        }
    }
    top.sort_by_key(|p| std::cmp::Reverse(p.working_set));
    top.truncate(5);
    ProcessCapture {
        count: Some(list.len()),
        threads: Some(threads),
        partial,
        top,
    }
}

/// C# `ProcessCounts`: `"{count} / {≥ }{threads}"`, or "Unavailable".
pub fn process_counts(count: Option<usize>, threads: Option<usize>, partial: bool) -> String {
    match (count, threads) {
        (Some(c), Some(t)) => format!("{c} / {}{t}", if partial { "≥ " } else { "" }),
        _ => "Unavailable".to_owned(),
    }
}

/// C# `Rate`: bytes per second between two counter readings. `None` on the first sample,
/// after a counter reset, or for a non-positive or non-finite interval.
pub fn rate(current: i64, previous: Option<i64>, seconds: f64) -> Option<f64> {
    let previous = previous?;
    (current >= previous && seconds > 0.0 && seconds.is_finite())
        .then(|| (current - previous) as f64 / seconds)
}

/// C# `Cpu`: busy percentage between two snapshots; `None` without a baseline or after a reset.
pub fn cpu(current: &MonitorSnapshot, previous: Option<&MonitorSnapshot>) -> Option<f64> {
    let previous = previous?;
    let (before, before_idle) = (previous.cpu_total?, previous.idle?);
    let (after, after_idle) = (current.cpu_total?, current.idle?);
    let (elapsed, idle) = (after - before, after_idle - before_idle);
    (elapsed > 0 && idle >= 0 && idle <= elapsed)
        .then(|| 100.0 * (elapsed - idle) as f64 / elapsed as f64)
}

/// `.NET` custom format `"0"` / `"0.#"` of a double: 15 significant digits, then rounded half
/// away from zero to `decimals` places with trailing zeros removed.
pub fn format_decimal(value: f64, decimals: usize) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let sci = format!("{:.14e}", value.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    // Split into integer and fractional digit vectors.
    let (mut int, mut frac): (Vec<u8>, Vec<u8>) = if exp >= 0 {
        let n = exp as usize + 1;
        let mut int: Vec<u8> = digits.iter().take(n).copied().collect();
        int.resize(n, 0);
        (int, digits.iter().skip(n).copied().collect())
    } else {
        let mut frac = vec![0u8; (-exp - 1) as usize];
        frac.extend(&digits);
        (vec![0], frac)
    };
    let round_up = frac.get(decimals).is_some_and(|&d| d >= 5);
    frac.truncate(decimals);
    if round_up {
        let mut carry = true;
        for d in frac.iter_mut().rev().chain(int.iter_mut().rev()) {
            if !carry {
                break;
            }
            if *d == 9 {
                *d = 0;
            } else {
                *d += 1;
                carry = false;
            }
        }
        if carry {
            int.insert(0, 1);
        }
    }
    while frac.last() == Some(&0) {
        frac.pop();
    }
    let zero = int.iter().all(|&d| d == 0) && frac.is_empty();
    let mut out = String::new();
    if value < 0.0 && !zero {
        out.push('-');
    }
    out.extend(int.iter().map(|d| (b'0' + d) as char));
    if !frac.is_empty() {
        out.push('.');
        out.extend(frac.iter().map(|d| (b'0' + d) as char));
    }
    out
}

/// C# `Bytes`: binary units (`B`, `KiB`, ... `PiB`); negative or non-finite is "Unavailable".
pub fn bytes(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return "Unavailable".to_owned();
    }
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let (mut v, mut unit) = (value, 0);
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    format!(
        "{} {}",
        format_decimal(v, usize::from(unit != 0)),
        UNITS[unit]
    )
}

/// C# `RateText`: `"{bytes}/s"`, or "Sampling…" while there is no baseline.
pub fn rate_text(rate: Option<f64>) -> String {
    match rate {
        Some(r) => format!("{}/s", bytes(r)),
        None => "Sampling…".to_owned(),
    }
}

/// C# `Uptime`: `"{days}d {hours}h {minutes}m"`.
pub fn uptime(value: Duration) -> String {
    let secs = value.as_secs();
    format!(
        "{}d {}h {}m",
        secs / 86_400,
        secs % 86_400 / 3_600,
        secs % 3_600 / 60
    )
}

/// Percentage with at most one decimal (`{0:0.#}%`), or "Sampling…" without a reading.
pub fn percent(value: Option<f64>) -> String {
    match value {
        Some(v) if v.is_finite() => format!("{}%", format_decimal(v, 1)),
        _ => "Sampling…".to_owned(),
    }
}

/// Whether the background worker should read now: not while a read is running, and at most
/// once per [`SAMPLE_INTERVAL_SECONDS`].
pub fn should_sample(busy: bool, seconds_since_last_read: Option<f64>) -> bool {
    !busy && seconds_since_last_read.is_none_or(|s| s >= SAMPLE_INTERVAL_SECONDS)
}

/// The last [`CPU_HISTORY_LEN`] CPU readings for the sparkline (`None` = no reading).
#[derive(Debug, Clone, Default)]
pub struct CpuHistory {
    values: std::collections::VecDeque<Option<f64>>,
}

impl CpuHistory {
    pub fn push(&mut self, value: Option<f64>) {
        self.values.push_back(value);
        while self.values.len() > CPU_HISTORY_LEN {
            self.values.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = Option<f64>> + '_ {
        self.values.iter().copied()
    }
}
