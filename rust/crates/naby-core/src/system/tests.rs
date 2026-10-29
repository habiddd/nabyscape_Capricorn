use super::*;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ---- SystemMonitorTests.cs ----

fn probe(threads: Option<usize>) -> ProcessProbe {
    ProcessProbe {
        name: Some("p".into()),
        id: 1,
        working_set: Some(1),
        threads,
    }
}

#[test]
fn failed_process_enumeration_leaves_both_counts_unavailable() {
    let c = capture_processes(false, || Err::<Vec<ProcessProbe>, _>("Enumeration failed"));
    assert!(c.count.is_none() && c.threads.is_none() && c.top.is_empty());
    assert_eq!(process_counts(c.count, c.threads, c.partial), "Unavailable");
}

#[test]
fn successful_empty_enumeration_is_a_measured_zero() {
    let c = capture_processes(false, || Ok::<_, ()>(Vec::new()));
    assert_eq!((c.count, c.threads, c.partial), (Some(0), Some(0), false));
    assert_eq!(process_counts(c.count, c.threads, c.partial), "0 / 0");
}

#[test]
fn per_process_thread_failure_keeps_count_and_lower_bound() {
    let c = capture_processes(false, || Ok::<_, ()>(vec![probe(None)]));
    assert_eq!((c.count, c.threads, c.partial), (Some(1), Some(0), true));
    assert_eq!(process_counts(c.count, c.threads, c.partial), "1 / ≥ 0");
    let c = capture_processes(false, || Ok::<_, ()>(vec![probe(Some(4)), probe(Some(6))]));
    assert_eq!(process_counts(c.count, c.threads, c.partial), "2 / 10");
}

#[test]
fn top_processes_are_the_five_largest_working_sets() {
    let list: Vec<ProcessProbe> = (0..8)
        .map(|i| ProcessProbe {
            name: Some(format!("p{i}")),
            id: i,
            working_set: Some((i as i64 * 7) % 8 * 100),
            threads: Some(1),
        })
        .chain([ProcessProbe {
            name: None,
            id: 99,
            working_set: Some(10_000),
            threads: Some(1),
        }])
        .collect();
    let c = capture_processes(true, || Ok::<_, ()>(list.clone()));
    assert_eq!(c.count, Some(9));
    assert_eq!(c.top.len(), 5);
    assert!(c
        .top
        .windows(2)
        .all(|w| w[0].working_set >= w[1].working_set));
    assert_eq!(c.top[0].working_set, 700);
    // Top is not collected unless asked.
    assert!(capture_processes(false, || Ok::<_, ()>(list.clone()))
        .top
        .is_empty());
}

#[test]
fn network_rate_rules() {
    assert_eq!(rate(100, None, 2.0), None, "baseline is sampling");
    assert_eq!(rate(1100, Some(100), 2.0), Some(500.0));
    assert_eq!(
        rate(100, Some(100), 2.0),
        Some(0.0),
        "unchanged is a real zero"
    );
    assert_eq!(
        rate(10, Some(100), 2.0),
        None,
        "counter reset starts a new baseline"
    );
    assert_eq!(rate(100, Some(10), 0.0), None);
    assert_eq!(rate(100, Some(10), f64::NAN), None);
    assert_eq!(rate(100, Some(10), f64::INFINITY), None);
}

#[test]
fn bytes_formatting() {
    assert_eq!(bytes(0.0), "0 B");
    assert_eq!(bytes(1.0), "1 B");
    assert_eq!(bytes(1023.0), "1023 B");
    assert_eq!(bytes(1024.0), "1 KiB");
    assert_eq!(bytes(1536.0), "1.5 KiB");
    assert_eq!(
        bytes(1280.0),
        "1.3 KiB",
        "halves round away from zero like .NET"
    );
    assert_eq!(bytes(1_073_741_824.0), "1 GiB");
    assert_eq!(bytes(1024.0 * 1024.0 * 1024.0 * 1024.0 * 3.25), "3.3 TiB");
    assert_eq!(bytes(1024f64.powi(5) * 2048.0), "2048 PiB");
    assert_eq!(bytes(-1.0), "Unavailable");
    assert_eq!(bytes(f64::NAN), "Unavailable");
    assert_eq!(bytes(f64::INFINITY), "Unavailable");
    assert_eq!(bytes(0.4), "0 B");
    assert_eq!(bytes(0.5), "1 B");
    // Same as C#: unit selection happens before rounding.
    assert_eq!(bytes(1023.96 * 1024.0), "1024 KiB");
}

#[test]
fn rate_text_never_shows_unprimed_as_zero() {
    assert_eq!(rate_text(None), "Sampling…");
    assert_eq!(rate_text(Some(0.0)), "0 B/s");
    assert_eq!(rate_text(Some(2048.0)), "2 KiB/s");
}

#[test]
fn uptime_retains_days() {
    assert_eq!(uptime(Duration::from_secs(1501 * 60)), "1d 1h 1m");
    assert_eq!(uptime(Duration::ZERO), "0d 0h 0m");
    assert_eq!(uptime(Duration::from_secs(86_399)), "0d 23h 59m");
}

fn sample(idle: i64, total: i64) -> MonitorSnapshot {
    MonitorSnapshot {
        timestamp: 0,
        idle: Some(idle),
        cpu_total: Some(total),
        available: Some(1),
        total: Some(2),
        ..MonitorSnapshot::unavailable("")
    }
}

#[test]
fn cpu_rules() {
    assert_eq!(cpu(&sample(10, 20), None), None, "requires a baseline");
    assert_eq!(cpu(&sample(40, 200), Some(&sample(10, 100))), Some(70.0));
    assert_eq!(cpu(&sample(1, 2), Some(&sample(10, 100))), None, "reset");
    assert_eq!(
        cpu(&sample(10, 100), Some(&sample(10, 100))),
        None,
        "zero interval"
    );
    let missing = MonitorSnapshot::unavailable("x");
    assert_eq!(cpu(&sample(40, 200), Some(&missing)), None);
    assert_eq!(cpu(&missing, Some(&sample(10, 100))), None);
    // Idle larger than elapsed is nonsense, not a negative load.
    assert_eq!(cpu(&sample(500, 200), Some(&sample(10, 100))), None);
}

#[test]
fn snapshot_unavailable_form_and_memory_text() {
    let s = MonitorSnapshot::unavailable("no OS");
    assert_eq!(s.errors, ["no OS"]);
    assert_eq!(s.memory_text(), "Unavailable");
    assert!(s.memory_used().is_none() && s.hardware.is_none());
    let m = MonitorSnapshot {
        total: Some(8 * 1024 * 1024 * 1024),
        available: Some(2 * 1024 * 1024 * 1024),
        ..s.clone()
    };
    assert_eq!(m.memory_text(), "6 GiB / 8 GiB");
    let inconsistent = MonitorSnapshot {
        total: Some(1),
        available: Some(2),
        ..s
    };
    assert_eq!(inconsistent.memory_text(), "Unavailable");
}

#[test]
fn percent_and_decimals() {
    assert_eq!(percent(Some(70.0)), "70%");
    assert_eq!(percent(Some(12.34)), "12.3%");
    assert_eq!(percent(Some(12.35)), "12.4%");
    assert_eq!(percent(Some(99.96)), "100%");
    assert_eq!(percent(None), "Sampling…");
    assert_eq!(percent(Some(f64::NAN)), "Sampling…");
    assert_eq!(format_decimal(0.0, 1), "0");
    assert_eq!(format_decimal(-0.04, 1), "0");
    assert_eq!(format_decimal(-5.25, 1), "-5.3");
    assert_eq!(format_decimal(0.05, 1), "0.1");
    assert_eq!(format_decimal(9.95, 1), "10");
    assert_eq!(format_decimal(1e-7, 1), "0");
    assert_eq!(format_decimal(123456789.0, 1), "123456789");
    assert_eq!(format_decimal(2.5, 0), "3");
    assert_eq!(format_decimal(1e20, 0), "100000000000000000000");
}

#[test]
fn sampling_policy() {
    assert!(should_sample(false, None));
    assert!(!should_sample(true, None));
    assert!(!should_sample(false, Some(1.9)));
    assert!(should_sample(false, Some(2.0)));
    let mut h = CpuHistory::default();
    assert!(h.is_empty());
    for i in 0..40 {
        h.push(if i % 2 == 0 { Some(i as f64) } else { None });
    }
    assert_eq!(h.len(), 30);
    assert_eq!(h.iter().next(), Some(Some(10.0)));
    h.clear();
    assert!(h.is_empty());
}

// ---- HardwareMonitorTests.cs ----

const ADAPTER: &str = "luid_0x00000000_0x00000001_phys_0";
const SECOND: &str = "luid_0x00000000_0x00000002_phys_0";

fn name(pid: u32, engine: u32) -> String {
    format!("pid_{pid}_{ADAPTER}_eng_{engine}_engtype_3D")
}

fn engine(pid: u32, eng: u32, counter: u64, timestamp: u64) -> GpuEngine {
    GpuEngine {
        name: name(pid, eng),
        counter,
        timestamp,
    }
}

#[test]
fn gpu_sums_processes_per_engine_and_uses_busiest_engine() {
    let prior = [
        engine(1, 0, 100, 1000),
        engine(2, 0, 0, 1000),
        engine(1, 1, 0, 1000),
    ];
    let current = [
        engine(1, 0, 140, 1100),
        engine(2, 0, 30, 1100),
        engine(1, 1, 50, 1100),
    ];
    let values = utilization(&current, &prior);
    assert_eq!(values.len(), 1);
    assert_eq!(values[ADAPTER], 70.0);
}

#[test]
fn gpu_initial_read_is_unavailable_never_a_fabricated_zero() {
    let current = [engine(1, 0, 140, 1100)];
    assert!(utilization(&current, &[]).is_empty());
}

#[test]
fn gpu_reset_counters_and_timestamps_invalidate_the_baseline() {
    let prior = [engine(1, 0, 100, 1000)];
    assert!(utilization(&[engine(1, 0, 1, 1100)], &prior).is_empty());
    assert!(utilization(&[engine(1, 0, 140, 1000)], &prior).is_empty());
    assert!(utilization(&[engine(1, 0, 140, 900)], &prior).is_empty());
}

#[test]
fn gpu_unchanged_counters_are_measured_zero_and_aggregate_is_bounded() {
    let prior = [engine(1, 0, 100, 1000)];
    assert_eq!(
        utilization(&[engine(1, 0, 100, 1100)], &prior)[ADAPTER],
        0.0
    );
    assert_eq!(
        utilization(&[engine(1, 0, 300, 1100)], &prior)[ADAPTER],
        100.0
    );
}

#[test]
fn malformed_adapter_names_cannot_become_measurements() {
    assert_eq!(adapter_id("_Total"), None);
    assert_eq!(adapter_id(&name(1, 0)).as_deref(), Some(ADAPTER));
    assert_eq!(
        adapter_id("LUID_0xABCDEF_0x1F_PHYS_12_x").as_deref(),
        Some("luid_0xabcdef_0x1f_phys_12")
    );
    assert_eq!(adapter_id("luid_0x_0x1_phys_0"), None);
    assert_eq!(adapter_id("luid_0x1_0x2_phys_"), None);
    assert_eq!(adapter_id("luid_0x1_0x2_phys_x"), None);
    assert_eq!(adapter_id(""), None);
    // A second, valid adapter later in the string still matches.
    assert_eq!(
        adapter_id("luid_0x1_0x2_phys_ luid_0x3_0x4_phys_5").as_deref(),
        Some("luid_0x3_0x4_phys_5")
    );
    // Engines without an `_eng_<n>` part are not measurable.
    let bad = |c| GpuEngine {
        name: format!("pid_1_{ADAPTER}_engtype_3D"),
        counter: c,
        timestamp: c * 10,
    };
    assert!(utilization(&[bad(2)], &[bad(1)]).is_empty());
}

#[test]
fn gpu_and_sensor_formatting_preserve_unavailable_states() {
    assert_eq!(hardware_percent(None), "Unavailable / sampling");
    assert_eq!(hardware_percent(Some(f64::NAN)), "Unavailable / sampling");
    assert_eq!(hardware_percent(Some(101.0)), "Unavailable / sampling");
    assert_eq!(hardware_percent(Some(-1.0)), "Unavailable / sampling");
    assert_eq!(hardware_percent(Some(0.0)), "0%");
    assert_eq!(hardware_percent(Some(40.0)), "40%");
    let s = |name: &str, kind: &str, value: f64| HardwareSensor {
        name: name.into(),
        kind: kind.into(),
        value,
        provider: "test".into(),
    };
    assert_eq!(sensor_value(&s("CPU", "Temperature", 42.5)), "42.5 °C");
    assert_eq!(sensor_value(&s("CPU fan", "Fan", 1200.0)), "1200 RPM");
}

#[test]
fn independent_gpu_adapters_keep_independent_utilization() {
    let copy = |c, t| GpuEngine {
        name: format!("pid_3_{SECOND}_eng_0_engtype_Copy"),
        counter: c,
        timestamp: t,
    };
    let separate = utilization(
        &[engine(1, 0, 140, 1100), copy(20, 1100)],
        &[engine(1, 0, 100, 1000), copy(0, 1000)],
    );
    assert_eq!(separate.len(), 2);
    assert_eq!((separate[ADAPTER], separate[SECOND]), (40.0, 20.0));
}

fn missing(reason: &str) -> MonitorHardware {
    MonitorHardware {
        cpus: vec![],
        gpu_models: vec![],
        gpus: vec![],
        sensors: vec![],
        status: Some(reason.to_owned()),
    }
}

fn reading(label: &str) -> MonitorHardware {
    MonitorHardware {
        cpus: vec![],
        gpu_models: vec![],
        gpus: vec![],
        sensors: vec![HardwareSensor {
            name: label.into(),
            kind: "Temperature".into(),
            value: 42.0,
            provider: "test".into(),
        }],
        status: None,
    }
}

#[test]
fn unavailable_hardware_includes_no_invented_readings() {
    let reader = HardwareReader::new(|_, _| Vec::new(), || 0);
    let u = reader.unavailable("Test unavailable");
    assert!(u.gpus.is_empty() && u.sensors.is_empty());
    assert_eq!(u.status.as_deref(), Some("Test unavailable"));
}

type Slots = Arc<Mutex<Vec<Arc<Slot>>>>;

struct Fixture {
    now: Arc<AtomicI64>,
    reads: Arc<AtomicUsize>,
    waits: Arc<AtomicUsize>,
    slots: Slots,
}

/// Worker whose first read stays pending (its slot is kept for the test) and later reads
/// complete immediately with `fresh`.
fn worker(fix: &Fixture, fresh: MonitorHardware, advance_on_wait: bool) -> HardwareCaptureWorker {
    let (reads, slots) = (fix.reads.clone(), fix.slots.clone());
    let (now, waits) = (fix.now.clone(), fix.waits.clone());
    let clock = fix.now.clone();
    HardwareCaptureWorker::new(
        move |_generation, slot| {
            let n = reads.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                slots.lock().unwrap().push(slot);
            } else {
                slot.complete(Ok(fresh.clone()));
            }
        },
        missing,
        move || clock.load(Ordering::SeqCst),
        Some(Box::new(move |slot: &Slot| {
            waits.fetch_add(1, Ordering::SeqCst);
            if advance_on_wait {
                now.fetch_add(1200, Ordering::SeqCst);
            }
            slot.is_completed()
        })),
    )
}

fn fixture() -> Fixture {
    Fixture {
        now: Arc::new(AtomicI64::new(0)),
        reads: Arc::new(AtomicUsize::new(0)),
        waits: Arc::new(AtomicUsize::new(0)),
        slots: Arc::new(Mutex::new(Vec::new())),
    }
}

fn status(h: &MonitorHardware) -> &str {
    h.status.as_deref().unwrap_or("")
}

#[test]
fn delayed_read_respects_bounded_wait_and_is_delivered_once() {
    let fix = fixture();
    let (late, fresh) = (reading("late"), reading("fresh"));
    let w = worker(&fix, fresh.clone(), true);
    let first = w.capture();
    assert_eq!(status(&first), "Hardware query timed out · retrying");
    assert_eq!(fix.reads.load(Ordering::SeqCst), 1);
    assert_eq!(fix.waits.load(Ordering::SeqCst), 1);
    // A busy provider does not queue a second read.
    let busy = w.capture();
    assert_eq!(status(&busy), "Hardware provider busy · retrying");
    assert_eq!(
        (
            fix.reads.load(Ordering::SeqCst),
            fix.waits.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
    // The late result completes within the freshness window: delivered on the next capture...
    fix.now.store(1500, Ordering::SeqCst);
    fix.slots.lock().unwrap()[0].complete(Ok(late.clone()));
    fix.now.store(2000, Ordering::SeqCst);
    assert_eq!(w.capture(), late);
    assert_eq!(fix.reads.load(Ordering::SeqCst), 1);
    // ...and only once.
    assert_eq!(w.capture(), fresh);
    assert_eq!(fix.reads.load(Ordering::SeqCst), 2);
}

#[test]
fn reset_keeps_a_single_old_generation_read_and_rejects_its_result() {
    let fix = fixture();
    let (late, fresh) = (reading("late"), reading("fresh"));
    let w = worker(&fix, fresh.clone(), false);
    w.capture();
    w.reset_sampling();
    assert_eq!(status(&w.capture()), "Hardware provider busy · retrying");
    assert_eq!(fix.reads.load(Ordering::SeqCst), 1);
    fix.now.store(1500, Ordering::SeqCst);
    fix.slots.lock().unwrap()[0].complete(Ok(late));
    fix.now.store(2000, Ordering::SeqCst);
    assert_eq!(w.capture(), fresh, "old-generation completion is discarded");
    assert_eq!(fix.reads.load(Ordering::SeqCst), 2);
}

#[test]
fn completion_older_than_freshness_window_is_discarded() {
    let fix = fixture();
    let fresh = reading("fresh");
    let w = worker(&fix, fresh.clone(), false);
    w.capture();
    fix.now.store(6000, Ordering::SeqCst);
    fix.slots.lock().unwrap()[0].complete(Ok(reading("late")));
    assert_eq!(w.capture(), fresh);
    assert_eq!(fix.reads.load(Ordering::SeqCst), 2);
}

#[test]
fn faulted_read_is_cleared_and_retried() {
    let fix = fixture();
    let fresh = reading("fresh");
    let w = worker(&fix, fresh.clone(), false);
    w.capture();
    fix.slots.lock().unwrap()[0].complete(Err("WMI exploded".into()));
    assert_eq!(w.capture(), fresh);
}

#[test]
fn reset_during_the_caller_wait_rejects_a_completed_sample() {
    let cell: Arc<Mutex<Option<Arc<HardwareCaptureWorker>>>> = Arc::new(Mutex::new(None));
    let late = reading("late");
    let handle = cell.clone();
    let w = Arc::new(HardwareCaptureWorker::new(
        move |_g, slot| slot.complete(Ok(late.clone())),
        missing,
        || 0,
        Some(Box::new(move |_slot: &Slot| {
            if let Some(w) = handle.lock().unwrap().as_ref() {
                w.reset_sampling();
            }
            true
        })),
    ));
    *cell.lock().unwrap() = Some(w.clone());
    assert!(w.capture().sensors.is_empty());
    // The default bounded wait (real clock, synchronous read) delivers normally.
    let late = reading("late");
    let plain = HardwareCaptureWorker::new(
        move |_g, slot| slot.complete(Ok(late.clone())),
        missing,
        || 0,
        None,
    );
    assert_eq!(plain.capture().sensors.len(), 1);
}

#[test]
fn default_wait_times_out_without_blocking_forever() {
    let w = HardwareCaptureWorker::new(|_g, _slot| {}, missing, || 0, None);
    let started = std::time::Instant::now();
    assert_eq!(status(&w.capture()), "Hardware query timed out · retrying");
    assert!(started.elapsed() < Duration::from_secs(5));
}

fn row(pairs: &[(&str, Cell)]) -> Row {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn text_cell(s: &str) -> Cell {
    Cell::Text(s.to_owned())
}

#[test]
fn metadata_recovers_with_backoff_and_is_cached() {
    let now = Arc::new(AtomicI64::new(0));
    let cpu_reads = Arc::new(AtomicUsize::new(0));
    let model_reads = Arc::new(AtomicUsize::new(0));
    let (c, m) = (cpu_reads.clone(), model_reads.clone());
    let query = move |_scope: &str, q: &str| -> Vec<Row> {
        if q.contains("Win32_Processor") && c.fetch_add(1, Ordering::SeqCst) + 1 >= 3 {
            return vec![row(&[
                ("Name", text_cell("Recovered CPU")),
                ("NumberOfCores", Cell::Int(8)),
                ("NumberOfLogicalProcessors", Cell::Int(16)),
            ])];
        }
        if q.contains("Win32_VideoController") && m.fetch_add(1, Ordering::SeqCst) + 1 >= 2 {
            return vec![row(&[("Name", text_cell("Recovered GPU"))])];
        }
        Vec::new()
    };
    let clock = now.clone();
    let mut reader = HardwareReader::new(query, move || clock.load(Ordering::SeqCst));
    let first = reader.read(0, false);
    assert!(first.cpus.is_empty() && first.gpu_models.is_empty());
    now.store(4999, Ordering::SeqCst);
    reader.read(0, false);
    assert_eq!(
        (
            cpu_reads.load(Ordering::SeqCst),
            model_reads.load(Ordering::SeqCst)
        ),
        (1, 1)
    );
    now.store(5000, Ordering::SeqCst);
    let second = reader.read(0, false);
    assert_eq!(second.gpu_models, ["Recovered GPU"]);
    assert_eq!(model_reads.load(Ordering::SeqCst), 2);
    now.store(14_999, Ordering::SeqCst);
    reader.read(0, false);
    assert_eq!(
        (
            cpu_reads.load(Ordering::SeqCst),
            model_reads.load(Ordering::SeqCst)
        ),
        (2, 2)
    );
    now.store(15_000, Ordering::SeqCst);
    let recovered = reader.read(1, false);
    assert_eq!(recovered.cpus.len(), 1);
    let cpu = &recovered.cpus[0];
    assert_eq!(
        (cpu.name.as_str(), cpu.cores, cpu.threads),
        ("Recovered CPU", Some(8), Some(16))
    );
    now.store(100_000, Ordering::SeqCst);
    reader.read(2, false);
    assert_eq!(
        (
            cpu_reads.load(Ordering::SeqCst),
            model_reads.load(Ordering::SeqCst)
        ),
        (3, 2)
    );
}

#[test]
fn unavailable_metadata_caps_backoff_at_thirty_seconds() {
    let now = Arc::new(AtomicI64::new(0));
    let attempts = Arc::new(AtomicUsize::new(0));
    let a = attempts.clone();
    let clock = now.clone();
    let mut absent = HardwareReader::new(
        move |_, q| {
            if q.contains("Win32_Processor") {
                a.fetch_add(1, Ordering::SeqCst);
            }
            Vec::new()
        },
        move || clock.load(Ordering::SeqCst),
    );
    let times = [0i64, 5000, 15_000, 35_000, 65_000, 95_000, 125_000];
    for (i, t) in times.iter().enumerate() {
        if i > 0 {
            now.store(t - 1, Ordering::SeqCst);
            absent.read(0, false);
            assert_eq!(attempts.load(Ordering::SeqCst), i, "early retry at {t}");
        }
        now.store(*t, Ordering::SeqCst);
        absent.read(0, false);
        assert_eq!(attempts.load(Ordering::SeqCst), i + 1, "no retry at {t}");
    }
    // An explicit retry request ignores the backoff.
    absent.request_retry();
    absent.read(0, false);
    assert_eq!(attempts.load(Ordering::SeqCst), times.len() + 1);
}

#[test]
fn reader_generation_reset_preserves_unavailable_gpu_baseline() {
    let counter = Arc::new(AtomicI64::new(100));
    let stamp = Arc::new(AtomicI64::new(1000));
    let (c, t) = (counter.clone(), stamp.clone());
    let engine_name = format!("pid_1_{ADAPTER}_eng_0_engtype_3D");
    let mut reader = HardwareReader::new(
        move |_, q| {
            if q.contains("GPUEngine") {
                vec![row(&[
                    ("Name", text_cell(&engine_name)),
                    (
                        "UtilizationPercentage",
                        Cell::UInt(c.load(Ordering::SeqCst) as u64),
                    ),
                    (
                        "Timestamp_Sys100NS",
                        Cell::UInt(t.load(Ordering::SeqCst) as u64),
                    ),
                ])]
            } else {
                Vec::new()
            }
        },
        || 0,
    );
    let baseline = reader.read(0, false);
    counter.store(140, Ordering::SeqCst);
    stamp.store(1100, Ordering::SeqCst);
    let measured = reader.read(0, false);
    counter.store(180, Ordering::SeqCst);
    stamp.store(1200, Ordering::SeqCst);
    let reset = reader.read(1, false);
    assert_eq!(baseline.gpus[0].utilization, None);
    assert_eq!(measured.gpus[0].utilization, Some(40.0));
    assert_eq!(reset.gpus[0].utilization, None);
    assert_eq!(baseline.status, None);
}

#[test]
fn reader_without_counters_reports_status_and_filters_sensors() {
    let query = |scope: &str, q: &str| -> Vec<Row> {
        if q.contains("FROM Sensor") && scope.ends_with("LibreHardwareMonitor") {
            return vec![
                row(&[
                    ("Name", text_cell("CPU")),
                    ("SensorType", text_cell("Temperature")),
                    ("Value", Cell::Float(55.5)),
                ]),
                row(&[
                    ("Name", text_cell("Fan")),
                    ("SensorType", text_cell("Fan")),
                    ("Value", text_cell("1200")),
                ]),
                row(&[
                    ("Name", text_cell("Load")),
                    ("SensorType", text_cell("Load")),
                    ("Value", Cell::Float(5.0)),
                ]),
                row(&[
                    ("Name", text_cell("Bad")),
                    ("SensorType", text_cell("Temperature")),
                    ("Value", text_cell("NaN")),
                ]),
                row(&[
                    ("Name", text_cell("Gone")),
                    ("SensorType", text_cell("Temperature")),
                    ("Value", Cell::Null),
                ]),
            ];
        }
        if q.contains("GPUAdapterMemory") {
            return vec![row(&[
                ("Name", text_cell(&format!("{ADAPTER}_part_0"))),
                ("DedicatedUsage", Cell::UInt(1024)),
                ("SharedUsage", text_cell("2048")),
            ])];
        }
        Vec::new()
    };
    let mut reader = HardwareReader::new(query, || 0);
    let h = reader.read(0, false);
    assert_eq!(h.sensors.len(), 2);
    assert_eq!(h.sensors[0].provider, "LibreHardwareMonitor");
    assert_eq!(sensor_value(&h.sensors[1]), "1200 RPM");
    // Memory-only adapters are listed with no utilization.
    assert_eq!(h.gpus.len(), 1);
    assert_eq!(
        (h.gpus[0].utilization, h.gpus[0].dedicated, h.gpus[0].shared),
        (None, Some(1024), Some(2048))
    );
    assert_eq!(h.status, None);
    // GPU-only reads skip sensors.
    assert!(reader.read(0, true).sensors.is_empty());
    let mut empty = HardwareReader::new(|_, _| Vec::new(), || 0);
    let h = empty.read(0, false);
    assert_eq!(
        h.status.as_deref(),
        Some("GPU performance counters unavailable")
    );
    assert!(h.gpus.is_empty());
}
