//! Ported from `Features/System/HardwareMonitor.cs`: hardware records, GPU engine arithmetic,
//! metadata retry policy and the bounded-wait capture worker. The WMI queries themselves are
//! injected (`HardwareReader::new(query, now)`), so the Windows host supplies the real ones.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use super::format_decimal;

#[derive(Debug, Clone, PartialEq)]
pub struct HardwareCpu {
    pub name: String,
    pub cores: Option<i32>,
    pub threads: Option<i32>,
    /// MHz.
    pub max_clock: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HardwareSensor {
    pub name: String,
    /// `Temperature` or `Fan`.
    pub kind: String,
    pub value: f64,
    pub provider: String,
}

/// One `GPUEngine` performance counter instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuEngine {
    pub name: String,
    pub counter: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HardwareGpu {
    pub id: String,
    pub utilization: Option<f64>,
    pub dedicated: Option<u64>,
    pub shared: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorHardware {
    pub cpus: Vec<HardwareCpu>,
    pub gpu_models: Vec<String>,
    pub gpus: Vec<HardwareGpu>,
    pub sensors: Vec<HardwareSensor>,
    pub status: Option<String>,
}

/// A WMI property value.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
}

/// One WMI result row, property name to value.
pub type Row = HashMap<String, Cell>;

fn text(row: &Row, key: &str) -> String {
    match row.get(key) {
        None | Some(Cell::Null) => String::new(),
        Some(Cell::Int(n)) => n.to_string(),
        Some(Cell::UInt(n)) => n.to_string(),
        Some(Cell::Float(n)) => n.to_string(),
        Some(Cell::Text(s)) => s.trim().to_owned(),
    }
}

fn unsigned(row: &Row, key: &str) -> Option<u64> {
    text(row, key).parse().ok()
}

fn integer(row: &Row, key: &str) -> Option<i32> {
    text(row, key).parse().ok().filter(|&n| n > 0)
}

fn number(row: &Row, key: &str) -> Option<f64> {
    text(row, key).parse().ok().filter(|n: &f64| n.is_finite())
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

/// Length of the longest run of bytes at `from` satisfying `pred`.
fn run(bytes: &[u8], from: usize, pred: impl Fn(u8) -> bool) -> usize {
    bytes[from.min(bytes.len())..]
        .iter()
        .take_while(|&&b| pred(b))
        .count()
}

/// Matches `luid_0x[0-9a-f]+_0x[0-9a-f]+_phys_\d+` (ignoring case) starting exactly at `at`.
fn adapter_at(lower: &[u8], at: usize) -> Option<usize> {
    let mut i = at;
    for part in [&b"luid_0x"[..], b"", b"_0x", b"", b"_phys_"] {
        if part.is_empty() {
            let n = run(lower, i, is_hex);
            if n == 0 {
                return None;
            }
            i += n;
        } else {
            if !lower[i.min(lower.len())..].starts_with(part) {
                return None;
            }
            i += part.len();
        }
    }
    let n = run(lower, i, |b| b.is_ascii_digit());
    (n > 0).then_some(i + n)
}

/// Windows engine instances include the process id, the adapter LUID and the physical engine:
/// `pid_1_luid_0x..._0x..._phys_0_eng_0_engtype_3D`. Returns the lowercase adapter part.
pub fn adapter_id(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    (0..bytes.len()).find_map(|i| adapter_at(bytes, i).map(|end| lower[i..end].to_owned()))
}

/// The lowercase `_eng_<n>` part of an engine name.
fn engine_id(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    (0..bytes.len()).find_map(|i| {
        if !bytes[i..].starts_with(b"_eng_") {
            return None;
        }
        let n = run(bytes, i + 5, |b| b.is_ascii_digit());
        (n > 0).then(|| lower[i..i + 5 + n].to_owned())
    })
}

/// Sums processes sharing an engine, then takes the busiest engine per adapter (Task Manager
/// semantics). Engines without a prior sample, with a reset counter or a non-advancing
/// timestamp contribute nothing, so an unprimed adapter is absent rather than zero.
pub fn utilization(current: &[GpuEngine], before: &[GpuEngine]) -> HashMap<String, f64> {
    let mut old: HashMap<&str, &GpuEngine> = HashMap::new();
    for e in before {
        old.entry(e.name.as_str()).or_insert(e);
    }
    let mut totals: HashMap<(String, String), f64> = HashMap::new();
    for e in current {
        let (Some(id), Some(engine)) = (adapter_id(&e.name), engine_id(&e.name)) else {
            continue;
        };
        let Some(prior) = old.get(e.name.as_str()) else {
            continue;
        };
        if e.timestamp <= prior.timestamp || e.counter < prior.counter {
            continue;
        }
        let pct =
            100.0 * (e.counter - prior.counter) as f64 / (e.timestamp - prior.timestamp) as f64;
        *totals.entry((id, engine)).or_insert(0.0) += pct;
    }
    let mut per_adapter: HashMap<String, f64> = HashMap::new();
    for ((adapter, _), value) in totals {
        let slot = per_adapter.entry(adapter).or_insert(f64::MIN);
        *slot = slot.max(value);
    }
    per_adapter
        .into_iter()
        .map(|(k, v)| (k, v.clamp(0.0, 100.0)))
        .collect()
}

/// C# `Percent`: `0.#%`, or "Unavailable / sampling" for a missing or out-of-range value.
pub fn percent(value: Option<f64>) -> String {
    match value {
        Some(v) if v.is_finite() && (0.0..=100.0).contains(&v) => {
            format!("{}%", format_decimal(v, 1))
        }
        _ => "Unavailable / sampling".to_owned(),
    }
}

/// C# `SensorValue`: `42.5 °C` or `1200 RPM`.
pub fn sensor_value(sensor: &HardwareSensor) -> String {
    format!(
        "{}{}",
        format_decimal(sensor.value, 1),
        if sensor.kind == "Fan" { " RPM" } else { " °C" }
    )
}

/// Empty discoveries (including swallowed WMI failures) are retried with a doubling delay
/// (5 s up to 30 s), never cached as success.
#[derive(Debug)]
struct MetadataRetry {
    next: i64,
    delay: i64,
}

impl MetadataRetry {
    fn new() -> Self {
        Self {
            next: 0,
            delay: 5000,
        }
    }
    fn due(&self, now: i64) -> bool {
        now >= self.next
    }
    fn reset(&mut self) {
        *self = Self::new();
    }
    fn failed(&mut self, now: i64) {
        self.next = now + self.delay;
        self.delay = (self.delay * 2).min(30_000);
    }
}

type Query = Box<dyn Fn(&str, &str) -> Vec<Row> + Send>;
type Clock = Box<dyn Fn() -> i64 + Send>;

/// C# `HardwareMonitor.Reader`: one hardware read from injected WMI-style queries.
pub struct HardwareReader {
    query: Query,
    now: Clock,
    cpus: Option<Vec<HardwareCpu>>,
    models: Option<Vec<String>>,
    cpu_retry: MetadataRetry,
    model_retry: MetadataRetry,
    sensor_retries: Vec<(&'static str, MetadataRetry)>,
    retry_requested: AtomicBool,
    previous: Vec<GpuEngine>,
    previous_generation: Option<u32>,
}

impl HardwareReader {
    /// `query(scope, wql)` returns the rows, or an empty list on any failure; `now` is a
    /// millisecond clock.
    pub fn new(
        query: impl Fn(&str, &str) -> Vec<Row> + Send + 'static,
        now: impl Fn() -> i64 + Send + 'static,
    ) -> Self {
        Self {
            query: Box::new(query),
            now: Box::new(now),
            cpus: None,
            models: None,
            cpu_retry: MetadataRetry::new(),
            model_retry: MetadataRetry::new(),
            sensor_retries: vec![
                ("LibreHardwareMonitor", MetadataRetry::new()),
                ("OpenHardwareMonitor", MetadataRetry::new()),
            ],
            retry_requested: AtomicBool::new(false),
            previous: Vec::new(),
            previous_generation: None,
        }
    }

    /// Makes the next read retry metadata discovery immediately.
    pub fn request_retry(&self) {
        self.retry_requested.store(true, Ordering::SeqCst);
    }

    /// A reading with no GPU or sensor data and `reason` as the status.
    pub fn unavailable(&self, reason: &str) -> MonitorHardware {
        MonitorHardware {
            cpus: self.cpus.clone().unwrap_or_default(),
            gpu_models: self.models.clone().unwrap_or_default(),
            gpus: Vec::new(),
            sensors: Vec::new(),
            status: Some(reason.to_owned()),
        }
    }

    /// One read. `generation` changes when sampling is reset (tab switch), which discards the
    /// GPU counter baseline; `only_gpu` skips CPU, model and sensor discovery.
    pub fn read(&mut self, generation: u32, only_gpu: bool) -> MonitorHardware {
        let now = (self.now)();
        if self.retry_requested.swap(false, Ordering::SeqCst) {
            self.cpu_retry.reset();
            self.model_retry.reset();
            self.sensor_retries.iter_mut().for_each(|(_, r)| r.reset());
        }
        if !only_gpu && self.cpus.is_none() && self.cpu_retry.due(now) {
            let found: Vec<HardwareCpu> = (self.query)(
                "root\\cimv2",
                "SELECT Name,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed FROM Win32_Processor",
            )
            .iter()
            .map(|r| HardwareCpu {
                name: text(r, "Name"),
                cores: integer(r, "NumberOfCores"),
                threads: integer(r, "NumberOfLogicalProcessors"),
                max_clock: number(r, "MaxClockSpeed"),
            })
            .filter(|c| !c.name.is_empty())
            .collect();
            if found.is_empty() {
                self.cpu_retry.failed(now);
            } else {
                self.cpus = Some(found);
            }
        }
        if !only_gpu && self.models.is_none() && self.model_retry.due(now) {
            let found: Vec<String> =
                (self.query)("root\\cimv2", "SELECT Name FROM Win32_VideoController")
                    .iter()
                    .map(|r| text(r, "Name"))
                    .filter(|n| !n.is_empty())
                    .collect();
            if found.is_empty() {
                self.model_retry.failed(now);
            } else {
                self.models = Some(found);
            }
        }
        let engines: Vec<GpuEngine> = (self.query)(
            "root\\cimv2",
            "SELECT Name,UtilizationPercentage,Timestamp_Sys100NS FROM Win32_PerfRawData_GPUPerformanceCounters_GPUEngine",
        )
        .iter()
        .filter_map(|r| {
            Some(GpuEngine {
                name: text(r, "Name"),
                counter: unsigned(r, "UtilizationPercentage")?,
                timestamp: unsigned(r, "Timestamp_Sys100NS")?,
            })
        })
        .collect();
        let baseline: &[GpuEngine] = if self.previous_generation == Some(generation) {
            &self.previous
        } else {
            &[]
        };
        let usage = utilization(&engines, baseline);
        self.previous = engines;
        self.previous_generation = Some(generation);
        let memory = (self.query)(
            "root\\cimv2",
            "SELECT Name,DedicatedUsage,SharedUsage FROM Win32_PerfRawData_GPUPerformanceCounters_GPUAdapterMemory",
        );
        let mut ids: Vec<String> = Vec::new();
        let engine_ids = self.previous.iter().map(|e| adapter_id(&e.name));
        let memory_ids = memory.iter().map(|r| adapter_id(&text(r, "Name")));
        for id in engine_ids.chain(memory_ids).flatten() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        let gpus: Vec<HardwareGpu> = ids
            .into_iter()
            .map(|id| {
                let row = memory
                    .iter()
                    .find(|r| adapter_id(&text(r, "Name")).as_deref() == Some(id.as_str()));
                HardwareGpu {
                    utilization: usage.get(&id).copied(),
                    dedicated: row.and_then(|r| unsigned(r, "DedicatedUsage")),
                    shared: row.and_then(|r| unsigned(r, "SharedUsage")),
                    id,
                }
            })
            .collect();
        let mut sensors = Vec::new();
        if !only_gpu {
            for (provider, retry) in &mut self.sensor_retries {
                if !retry.due(now) {
                    continue;
                }
                let rows = (self.query)(
                    &format!("root\\{provider}"),
                    "SELECT Name,SensorType,Value FROM Sensor",
                );
                if rows.is_empty() {
                    retry.failed(now);
                    continue;
                }
                for row in &rows {
                    let kind = text(row, "SensorType");
                    if kind == "Temperature" || kind == "Fan" {
                        if let Some(value) = number(row, "Value") {
                            sensors.push(HardwareSensor {
                                name: text(row, "Name"),
                                kind,
                                value,
                                provider: (*provider).to_owned(),
                            });
                        }
                    }
                }
            }
        }
        MonitorHardware {
            cpus: self.cpus.clone().unwrap_or_default(),
            gpu_models: self.models.clone().unwrap_or_default(),
            status: gpus
                .is_empty()
                .then(|| "GPU performance counters unavailable".to_owned()),
            gpus,
            sensors,
        }
    }
}

/// The result of one hardware read, or the reason it failed.
pub type CaptureOutcome = Result<MonitorHardware, String>;

/// A single in-flight read: filled once by whichever thread performs it.
#[derive(Debug, Default)]
pub struct Slot {
    value: Mutex<Option<CaptureOutcome>>,
    ready: Condvar,
}

impl Slot {
    fn lock(&self) -> MutexGuard<'_, Option<CaptureOutcome>> {
        self.value.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Delivers the read's result (first call wins).
    pub fn complete(&self, outcome: CaptureOutcome) {
        let mut guard = self.lock();
        if guard.is_none() {
            *guard = Some(outcome);
        }
        self.ready.notify_all();
    }

    pub fn is_completed(&self) -> bool {
        self.lock().is_some()
    }

    /// Blocks up to `timeout` for completion.
    pub fn wait(&self, timeout: Duration) -> bool {
        let guard = self.lock();
        let (guard, _) = self
            .ready
            .wait_timeout_while(guard, timeout, |v| v.is_none())
            .unwrap_or_else(|e| e.into_inner());
        guard.is_some()
    }

    fn take(&self) -> Option<CaptureOutcome> {
        self.lock().take()
    }
}

type StartRead = Box<dyn Fn(u32, Arc<Slot>) + Send + Sync>;
type Unavailable = Box<dyn Fn(&str) -> MonitorHardware + Send + Sync>;
type Wait = Box<dyn Fn(&Slot) -> bool + Send + Sync>;

#[derive(Default)]
struct Inner {
    pending: Option<Arc<Slot>>,
    generation: u32,
    pending_generation: u32,
    started_at: i64,
}

/// C# `HardwareCaptureWorker`: at most one hardware read in flight, a bounded caller wait, and
/// delivery of a late result at most once while it is still fresh and from the current sampling
/// generation. A stuck provider can occupy one read, never an unbounded queue.
pub struct HardwareCaptureWorker {
    start: StartRead,
    unavailable: Unavailable,
    now: Clock2,
    wait: Option<Wait>,
    inner: Mutex<Inner>,
}

type Clock2 = Box<dyn Fn() -> i64 + Send + Sync>;

/// A late sample may be delivered once, for five seconds from the read's start. Measuring age
/// from the start also rejects a formerly stuck provider's old measurements.
const FRESHNESS_MS: i64 = 5000;
/// How long a capture waits for its own read.
pub const WAIT_MS: u64 = 1200;

impl HardwareCaptureWorker {
    /// `start(generation, slot)` begins a read (usually on a worker thread) that must call
    /// `slot.complete`; `unavailable(reason)` builds the placeholder reading; `now` is a
    /// millisecond clock; `wait` overrides the default 1.2 s bounded wait (tests).
    pub fn new(
        start: impl Fn(u32, Arc<Slot>) + Send + Sync + 'static,
        unavailable: impl Fn(&str) -> MonitorHardware + Send + Sync + 'static,
        now: impl Fn() -> i64 + Send + Sync + 'static,
        wait: Option<Wait>,
    ) -> Self {
        Self {
            start: Box::new(start),
            unavailable: Box::new(unavailable),
            now: Box::new(now),
            wait,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Invalidates any read already in flight (tab switch, restart).
    pub fn reset_sampling(&self) {
        let mut inner = self.inner();
        inner.generation = inner.generation.wrapping_add(1);
    }

    /// Consumes a completed read. Faulted, reset or stale results are dropped.
    fn take_completed(&self, inner: &mut Inner) -> Option<MonitorHardware> {
        let slot = inner.pending.take()?;
        let outcome = slot.take()?;
        if inner.pending_generation != inner.generation
            || (self.now)() - inner.started_at > FRESHNESS_MS
        {
            return None;
        }
        outcome.ok()
    }

    /// One capture: returns a fresh reading, a late reading from the previous call, or an
    /// "unavailable" placeholder naming why.
    pub fn capture(&self) -> MonitorHardware {
        let slot = {
            let mut inner = self.inner();
            if inner.pending.as_ref().is_some_and(|p| !p.is_completed()) {
                return (self.unavailable)("Hardware provider busy · retrying");
            }
            if inner.pending.is_some() {
                if let Some(done) = self.take_completed(&mut inner) {
                    return done;
                }
            }
            inner.pending_generation = inner.generation;
            inner.started_at = (self.now)();
            let slot = Arc::new(Slot::default());
            inner.pending = Some(slot.clone());
            (self.start)(inner.generation, slot.clone());
            slot
        };
        let finished = match &self.wait {
            Some(wait) => wait(&slot),
            None => slot.wait(Duration::from_millis(WAIT_MS)),
        };
        if !finished {
            return (self.unavailable)("Hardware query timed out · retrying");
        }
        let mut inner = self.inner();
        if inner
            .pending
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(p, &slot))
            && slot.is_completed()
        {
            return self
                .take_completed(&mut inner)
                .unwrap_or_else(|| (self.unavailable)("Hardware sample expired · retrying"));
        }
        (self.unavailable)("Hardware sample reset · retrying")
    }
}
