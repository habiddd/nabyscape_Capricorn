//! System monitor sampling (C# `SystemMonitor.Capture` and the WMI half of `HardwareMonitor`).
//!
//! [`SystemService`] samples every [`SAMPLE_INTERVAL_SECONDS`] **only while a [`SystemLease`] is
//! held** (the system page is visible); with no lease its thread sleeps on a condition variable
//! and no timer runs. All interpretation (CPU percentage, byte/rate text, process capture
//! arithmetic, GPU engine maths, hardware retry policy) is `naby_core::system`; this module
//! supplies the OS readings and the WMI query closure plugged into `HardwareReader`.
#![cfg_attr(not(windows), allow(dead_code))]

use naby_core::system::{self, CpuHistory, MonitorSnapshot};
use std::sync::Arc;

/// Which parts of the machine to read (C# `Capture(network, storage, processes, ...)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemOptions {
    pub network: bool,
    pub storage: bool,
    pub processes: bool,
    /// Process and thread counts (the C# call passes the `processes` flag for both).
    pub process_counts: bool,
    pub hardware: bool,
    /// Skip CPU/model/sensor WMI discovery and read only the GPU counters.
    pub gpu_only: bool,
}

impl SystemOptions {
    /// What the monitor tab `tab` reads, given which optional tabs the settings enable
    /// (`MonitorNetwork`, `MonitorStorage`, `MonitorProcesses`).
    pub fn for_tab(tab: &str, network_on: bool, storage_on: bool, processes_on: bool) -> Self {
        let processes = tab == "processes" && processes_on;
        Self {
            network: tab == "network" && network_on,
            storage: tab == "storage" && storage_on,
            processes,
            process_counts: processes,
            hardware: matches!(tab, "overview" | "hardware"),
            gpu_only: tab == "overview",
        }
    }
}

/// Receive/send rate of one adapter, `None` while there is no baseline (renders "Sampling…").
#[derive(Debug, Clone, PartialEq)]
pub struct NetRate {
    pub id: String,
    pub name: String,
    pub received: Option<f64>,
    pub sent: Option<f64>,
}

/// Per-adapter rates between two snapshots (C# `SystemMonitor.Rate` over matching adapter ids).
pub fn net_rates(
    latest: &MonitorSnapshot,
    previous: Option<&MonitorSnapshot>,
    ticks_per_second: f64,
) -> Vec<NetRate> {
    let seconds = previous.map_or(0.0, |p| {
        (latest.timestamp - p.timestamp) as f64 / ticks_per_second
    });
    latest
        .networks
        .iter()
        .map(|net| {
            let before = previous.and_then(|p| p.networks.iter().find(|n| n.id == net.id));
            NetRate {
                id: net.id.clone(),
                name: net.name.clone(),
                received: system::rate(net.received, before.map(|b| b.received), seconds),
                sent: system::rate(net.sent, before.map(|b| b.sent), seconds),
            }
        })
        .collect()
}

/// Microsecond timestamps: `MonitorSnapshot::timestamp` units.
pub const TICKS_PER_SECOND: f64 = 1_000_000.0;

/// What the system page paints.
#[derive(Debug, Clone, Default)]
pub struct SystemState {
    pub latest: Option<Arc<MonitorSnapshot>>,
    /// CPU busy percentage between the last two samples.
    pub cpu: Option<f64>,
    pub cpu_history: CpuHistory,
    pub networks: Vec<NetRate>,
    /// Samples delivered since the page became visible.
    pub reads: u64,
}

impl SystemState {
    /// Folds a new sample into the state. `previous` is the sample before it (None right after
    /// a tab change or when the page was just shown).
    pub fn push(&self, sample: MonitorSnapshot, previous: Option<&MonitorSnapshot>) -> Self {
        let cpu = system::cpu(&sample, previous);
        let mut history = self.cpu_history.clone();
        history.push(cpu);
        Self {
            networks: net_rates(&sample, previous, TICKS_PER_SECOND),
            latest: Some(Arc::new(sample)),
            cpu,
            cpu_history: history,
            reads: self.reads + 1,
        }
    }

    /// Forgets the baseline but keeps the sparkline (a tab change).
    pub fn without_baseline(&self) -> Self {
        Self {
            cpu_history: self.cpu_history.clone(),
            ..Self::default()
        }
    }
}

/// `Process.ProcessName`: the executable name without `.exe`; pid 0 is "Idle".
pub fn process_name(pid: u32, exe: &str) -> String {
    if pid == 0 {
        return "Idle".to_owned();
    }
    let lower = exe.to_ascii_lowercase();
    if lower.ends_with(".exe") {
        exe[..exe.len() - 4].to_owned()
    } else {
        exe.to_owned()
    }
}

/// Drive letters set in a `GetLogicalDrives` mask, as `"C:\"` roots.
pub fn drive_roots(mask: u32) -> Vec<String> {
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i) as char))
        .collect()
}

/// Splits the column list of a simple `SELECT a,b FROM c` WQL query.
pub fn select_columns(wql: &str) -> Vec<String> {
    let lower = wql.to_ascii_lowercase();
    let (Some(start), Some(end)) = (lower.find("select"), lower.find(" from ")) else {
        return Vec::new();
    };
    if start + 6 > end {
        return Vec::new();
    }
    wql[start + 6..end]
        .split(',')
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty() && c != "*")
        .collect()
}

#[cfg(windows)]
pub use os::{SystemLease, SystemService};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::svc_util::{from_wide, pcwstr, wide, ComGuard, OwnedHandle};
    use crate::wake::Waker;
    use naby_core::system::{
        capture_processes, Cell, HardwareCaptureWorker, HardwareReader, MonitorDrive,
        MonitorHardware, MonitorNetwork, MonitorProcess, ProcessProbe, Row,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex, MutexGuard};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};
    use windows::core::{BSTR, PCWSTR};
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
    };
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoSetProxyBlanket, CLSCTX_INPROC_SERVER, EOAC_NONE,
        RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::SystemInformation::{
        GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };
    use windows::Win32::System::Threading::{
        GetSystemTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::System::Variant::{
        VariantClear, VARIANT, VT_BOOL, VT_BSTR, VT_I1, VT_I2, VT_I4, VT_I8, VT_R4, VT_R8, VT_UI1,
        VT_UI2, VT_UI4, VT_UI8,
    };
    use windows::Win32::System::Wmi::{
        IEnumWbemClassObject, IWbemClassObject, IWbemLocator, IWbemServices, WbemLocator,
        WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    };

    const DRIVE_FIXED: u32 = 3;
    const IF_OPER_STATUS_UP: i32 = 1;
    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;
    /// `FilterInterface` bit of `MIB_IF_ROW2::InterfaceAndOperStatusFlags`.
    const FILTER_INTERFACE: u8 = 0b10;
    const WMI_TIMEOUT_MS: i32 = 350;
    const WMI_ROW_LIMIT: usize = 4096;
    const WBEM_S_TIMEDOUT: i32 = 0x0004_0004;

    fn filetime(ft: FILETIME) -> i64 {
        ((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)) as i64
    }

    fn read_cpu(errors: &mut Vec<String>) -> (Option<i64>, Option<i64>) {
        let (mut idle, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: the three out pointers are valid FILETIMEs for the duration of the call.
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_ok() {
            (
                Some(filetime(idle)),
                Some(filetime(kernel) + filetime(user)),
            )
        } else {
            errors.push("CPU unavailable".to_owned());
            (None, None)
        }
    }

    fn read_memory(errors: &mut Vec<String>) -> (Option<u64>, Option<u64>) {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is writable with dwLength set as the API requires.
        if unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() {
            (Some(status.ullAvailPhys), Some(status.ullTotalPhys))
        } else {
            errors.push("Memory unavailable".to_owned());
            (None, None)
        }
    }

    fn working_set(pid: u32) -> Option<i64> {
        if pid == 0 {
            return None;
        }
        // SAFETY: the handle is closed by OwnedHandle; the counters struct is writable and sized.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let handle = OwnedHandle(handle);
            let mut counters = PROCESS_MEMORY_COUNTERS {
                cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                ..Default::default()
            };
            GetProcessMemoryInfo(handle.0, &mut counters, counters.cb).ok()?;
            Some(counters.WorkingSetSize as i64)
        }
    }

    fn enumerate_processes(include_top: bool) -> Result<Vec<ProcessProbe>, String> {
        // SAFETY: standard ToolHelp enumeration; the snapshot handle is closed by OwnedHandle
        // and the entry struct is writable with dwSize set.
        unsafe {
            let snapshot = OwnedHandle(
                CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| e.message())?,
            );
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut out = Vec::new();
            let mut more = Process32FirstW(snapshot.0, &mut entry).is_ok();
            while more {
                let id = entry.th32ProcessID;
                out.push(ProcessProbe {
                    name: Some(process_name(id, &from_wide(&entry.szExeFile))),
                    id,
                    working_set: if include_top { working_set(id) } else { None },
                    threads: Some(entry.cntThreads as usize),
                });
                more = Process32NextW(snapshot.0, &mut entry).is_ok();
            }
            Ok(out)
        }
    }

    fn read_networks(errors: &mut Vec<String>) -> Vec<MonitorNetwork> {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        // SAFETY: GetIfTable2 allocates a table that is read within NumEntries and freed once.
        unsafe {
            if GetIfTable2(&mut table).0 != 0 || table.is_null() {
                errors.push("Network unavailable".to_owned());
                return Vec::new();
            }
            let count = (*table).NumEntries as usize;
            let rows: *const MIB_IF_ROW2 = (*table).Table.as_ptr();
            let mut out = Vec::new();
            for i in 0..count {
                let row = &*rows.add(i);
                if row.OperStatus.0 != IF_OPER_STATUS_UP
                    || row.Type == IF_TYPE_SOFTWARE_LOOPBACK
                    || row.InterfaceAndOperStatusFlags._bitfield & FILTER_INTERFACE != 0
                {
                    continue;
                }
                let g = row.InterfaceGuid;
                out.push(MonitorNetwork {
                    id: format!(
                        "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
                        g.data1, g.data2, g.data3, g.data4[0], g.data4[1], g.data4[2],
                        g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7]
                    ),
                    name: from_wide(&row.Alias),
                    received: row.InOctets as i64,
                    sent: row.OutOctets as i64,
                });
            }
            FreeMibTable(table.cast_const().cast());
            out
        }
    }

    fn read_drives(errors: &mut Vec<String>) -> Vec<MonitorDrive> {
        // SAFETY: argument-free query.
        let mask = unsafe { GetLogicalDrives() };
        if mask == 0 {
            errors.push("Storage unavailable".to_owned());
            return Vec::new();
        }
        let mut out = Vec::new();
        for root in drive_roots(mask) {
            let w = wide(&root);
            // SAFETY: `w` is NUL-terminated; the out pointers are valid u64s.
            unsafe {
                if GetDriveTypeW(pcwstr(&w)) != DRIVE_FIXED {
                    continue;
                }
                let (mut free, mut total) = (0u64, 0u64);
                if GetDiskFreeSpaceExW(pcwstr(&w), None, Some(&mut total), Some(&mut free)).is_ok()
                {
                    out.push(MonitorDrive {
                        name: root,
                        free: free as i64,
                        total: total as i64,
                    });
                } else {
                    errors.push("A volume is unavailable".to_owned());
                }
            }
        }
        out
    }

    // ---- WMI -----------------------------------------------------------------------------

    fn cell(variant: &VARIANT) -> Cell {
        // SAFETY: the union member read is selected by the variant's own type tag.
        unsafe {
            let v = &variant.Anonymous.Anonymous;
            let a = &v.Anonymous;
            match v.vt {
                VT_BSTR => Cell::Text(a.bstrVal.to_string()),
                VT_I1 => Cell::Int(i64::from(a.cVal)),
                VT_UI1 => Cell::Int(i64::from(a.bVal)),
                VT_I2 => Cell::Int(i64::from(a.iVal)),
                VT_UI2 => Cell::Int(i64::from(a.uiVal)),
                VT_I4 => Cell::Int(i64::from(a.lVal)),
                VT_UI4 => Cell::UInt(u64::from(a.ulVal)),
                VT_I8 => Cell::Int(a.llVal),
                VT_UI8 => Cell::UInt(a.ullVal),
                VT_R4 => Cell::Float(f64::from(a.fltVal)),
                VT_R8 => Cell::Float(a.dblVal),
                VT_BOOL => Cell::Int(i64::from(a.boolVal.0 != 0)),
                _ => Cell::Null,
            }
        }
    }

    fn read_object(object: &IWbemClassObject, columns: &[String]) -> Row {
        let mut row = Row::new();
        for column in columns {
            let name = wide(column);
            let mut value = VARIANT::default();
            // SAFETY: `name` is NUL-terminated; `value` is a valid VARIANT cleared right after.
            unsafe {
                if object.Get(pcwstr(&name), 0, &mut value, None, None).is_ok() {
                    row.insert(column.clone(), cell(&value));
                }
                let _ = VariantClear(&mut value);
            }
        }
        row
    }

    /// Sets the proxy security on the interface itself (not on a queried IUnknown).
    fn blanket<P: windows::core::Param<windows::core::IUnknown>>(proxy: P) {
        // SAFETY: standard proxy security setup on a live interface pointer.
        unsafe {
            let _ = CoSetProxyBlanket(
                proxy,
                10, // RPC_C_AUTHN_WINNT
                0,  // RPC_C_AUTHZ_NONE
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            );
        }
    }

    fn connect(scope: &str) -> Option<IWbemServices> {
        // SAFETY: COM calls on a thread that initialised COM; every BSTR outlives its call.
        unsafe {
            let locator: IWbemLocator =
                CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
            let services = locator
                .ConnectServer(
                    &BSTR::from(scope),
                    &BSTR::new(),
                    &BSTR::new(),
                    &BSTR::new(),
                    0,
                    &BSTR::new(),
                    None,
                )
                .ok()?;
            blanket(&services);
            Some(services)
        }
    }

    /// One WQL query as property rows. Any failure (missing class, absent provider, timeout,
    /// more than 4096 rows) is an empty list, like the C# `catch { return new(); }`.
    pub fn wmi_query(scope: &str, wql: &str) -> Vec<Row> {
        let columns = select_columns(wql);
        let Some(services) = connect(scope) else {
            return Vec::new();
        };
        // SAFETY: COM calls with valid BSTRs and a bounded object array.
        unsafe {
            let Ok(enumerator): Result<IEnumWbemClassObject, _> = services.ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from(wql),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            ) else {
                return Vec::new();
            };
            blanket(&enumerator);
            let mut rows = Vec::new();
            loop {
                let mut objects: [Option<IWbemClassObject>; 16] = Default::default();
                let mut returned = 0u32;
                let hr = enumerator.Next(WMI_TIMEOUT_MS, &mut objects, &mut returned);
                if hr.0 == WBEM_S_TIMEDOUT || hr.is_err() {
                    return Vec::new();
                }
                for object in objects.iter().take(returned as usize).flatten() {
                    rows.push(read_object(object, &columns));
                    if rows.len() >= WMI_ROW_LIMIT {
                        // Never present a truncated process-engine total as complete.
                        return Vec::new();
                    }
                }
                if returned == 0 {
                    break;
                }
            }
            rows
        }
    }

    // ---- service -------------------------------------------------------------------------

    struct Control {
        leases: usize,
        options: SystemOptions,
        generation: u64,
        shutdown: bool,
    }

    struct Shared {
        control: Mutex<Control>,
        wake_thread: Condvar,
        state: Mutex<Arc<SystemState>>,
        hardware: HardwareCaptureWorker,
        gpu_only: Arc<AtomicBool>,
        retry: Arc<AtomicBool>,
        wake: Waker,
    }

    impl Shared {
        fn control(&self) -> MutexGuard<'_, Control> {
            self.control.lock().unwrap_or_else(|e| e.into_inner())
        }

        fn set_state(&self, state: SystemState) {
            *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(state);
        }

        fn reset_hardware(&self) {
            self.retry.store(true, Ordering::SeqCst);
            self.hardware.reset_sampling();
        }
    }

    /// Held while the system page is visible; sampling stops when the last lease drops.
    pub struct SystemLease {
        shared: Arc<Shared>,
    }

    impl Drop for SystemLease {
        fn drop(&mut self) {
            let idle = {
                let mut control = self.shared.control();
                control.leases = control.leases.saturating_sub(1);
                control.generation += 1;
                control.leases == 0
            };
            if idle {
                // C# InvalidateSystemMonitor: baseline, history and hardware sampling reset.
                self.shared.set_state(SystemState::default());
                self.shared.reset_hardware();
            }
            self.shared.wake_thread.notify_all();
        }
    }

    pub struct SystemService {
        shared: Arc<Shared>,
        thread: Option<JoinHandle<()>>,
    }

    impl SystemService {
        pub fn start(wake: Waker) -> Self {
            let gpu_only = Arc::new(AtomicBool::new(false));
            let retry = Arc::new(AtomicBool::new(false));
            let known: Arc<Mutex<(Vec<naby_core::system::HardwareCpu>, Vec<String>)>> =
                Arc::default();
            let reader = Arc::new(Mutex::new(HardwareReader::new(wmi_query, tick_ms)));
            let start_reader = reader.clone();
            let start_gpu = gpu_only.clone();
            let start_retry = retry.clone();
            let start_known = known.clone();
            let hardware = HardwareCaptureWorker::new(
                move |generation, slot| {
                    let reader = start_reader.clone();
                    let gpu = start_gpu.load(Ordering::SeqCst);
                    let retry = start_retry.clone();
                    let known = start_known.clone();
                    let spawned = std::thread::Builder::new()
                        .name("NabyScape WMI".into())
                        .spawn({
                            let slot = slot.clone();
                            move || {
                                let _com = ComGuard::mta();
                                let mut reader = reader.lock().unwrap_or_else(|e| e.into_inner());
                                if retry.swap(false, Ordering::SeqCst) {
                                    reader.request_retry();
                                }
                                let result = reader.read(generation, gpu);
                                *known.lock().unwrap_or_else(|e| e.into_inner()) =
                                    (result.cpus.clone(), result.gpu_models.clone());
                                slot.complete(Ok(result));
                            }
                        });
                    if let Err(e) = spawned {
                        slot.complete(Err(e.to_string()));
                    }
                },
                move |reason| {
                    let (cpus, models) = known.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    MonitorHardware {
                        cpus,
                        gpu_models: models,
                        gpus: Vec::new(),
                        sensors: Vec::new(),
                        status: Some(reason.to_owned()),
                    }
                },
                tick_ms,
                None,
            );
            let shared = Arc::new(Shared {
                control: Mutex::new(Control {
                    leases: 0,
                    options: SystemOptions::default(),
                    generation: 0,
                    shutdown: false,
                }),
                wake_thread: Condvar::new(),
                state: Mutex::new(Arc::new(SystemState::default())),
                hardware,
                gpu_only,
                retry,
                wake,
            });
            let worker = shared.clone();
            let thread = std::thread::Builder::new()
                .name("NabyScape system monitor".into())
                .spawn(move || sampler(worker))
                .ok();
            Self { shared, thread }
        }

        /// Starts sampling until the returned lease is dropped.
        pub fn lease(&self) -> SystemLease {
            let first = {
                let mut control = self.shared.control();
                control.leases += 1;
                control.generation += 1;
                control.leases == 1
            };
            if first {
                self.shared.reset_hardware();
            }
            self.shared.wake_thread.notify_all();
            SystemLease {
                shared: self.shared.clone(),
            }
        }

        /// Changes what is read (tab change). Drops the baseline so rates restart at "Sampling…".
        pub fn set_options(&self, options: SystemOptions) {
            {
                let mut control = self.shared.control();
                if control.options == options {
                    return;
                }
                control.options = options;
                control.generation += 1;
            }
            self.shared
                .gpu_only
                .store(options.gpu_only, Ordering::SeqCst);
            if options.hardware {
                self.shared.reset_hardware();
            }
            let current = self.snapshot();
            self.shared.set_state(current.without_baseline());
            self.shared.wake_thread.notify_all();
        }

        /// The latest state; cheap.
        pub fn snapshot(&self) -> Arc<SystemState> {
            self.shared
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }

        pub fn sampling(&self) -> bool {
            self.shared.control().leases > 0
        }
    }

    impl Drop for SystemService {
        fn drop(&mut self) {
            self.shared.control().shutdown = true;
            self.shared.wake_thread.notify_all();
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    fn tick_ms() -> i64 {
        // SAFETY: argument-free query.
        unsafe { GetTickCount64() as i64 }
    }

    fn capture(options: SystemOptions, start: Instant, shared: &Shared) -> MonitorSnapshot {
        let mut errors = Vec::new();
        let (idle, cpu_total) = read_cpu(&mut errors);
        let (available, total) = read_memory(&mut errors);
        let processes = if options.process_counts || options.processes {
            let capture =
                capture_processes(options.processes, || enumerate_processes(options.processes));
            if options.process_counts && capture.count.is_none() {
                errors.push("Process enumeration unavailable".to_owned());
            }
            capture
        } else {
            Default::default()
        };
        let networks = if options.network {
            read_networks(&mut errors)
        } else {
            Vec::new()
        };
        let drives = if options.storage {
            read_drives(&mut errors)
        } else {
            Vec::new()
        };
        errors.dedup();
        MonitorSnapshot {
            timestamp: start.elapsed().as_micros() as i64,
            idle,
            cpu_total,
            available,
            total,
            uptime: Duration::from_millis(tick_ms().max(0) as u64),
            process_count: processes.count,
            thread_count: processes.threads,
            partial_threads: processes.partial,
            networks,
            drives,
            processes: processes
                .top
                .into_iter()
                .map(|p| MonitorProcess {
                    name: p.name,
                    id: p.id,
                    working_set: p.working_set,
                })
                .collect(),
            errors,
            hardware: options.hardware.then(|| shared.hardware.capture()),
        }
    }

    fn sampler(shared: Arc<Shared>) {
        let start = Instant::now();
        loop {
            let (options, generation) = {
                let mut control = shared.control();
                while !control.shutdown && control.leases == 0 {
                    control = shared
                        .wake_thread
                        .wait(control)
                        .unwrap_or_else(|e| e.into_inner());
                }
                if control.shutdown {
                    return;
                }
                (control.options, control.generation)
            };
            let sample = capture(options, start, &shared);
            {
                let control = shared.control();
                if control.shutdown {
                    return;
                }
                // A tab change or hidden page while reading: reject the stale sample.
                if control.generation == generation && control.leases > 0 {
                    let current = shared
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone();
                    let next = current.push(sample, current.latest.as_deref());
                    drop(control);
                    shared.set_state(next);
                    (shared.wake)();
                }
            }
            let mut control = shared.control();
            let deadline =
                Instant::now() + Duration::from_secs_f64(system::SAMPLE_INTERVAL_SECONDS);
            while !control.shutdown && control.leases > 0 && control.generation == generation {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                control = shared
                    .wake_thread
                    .wait_timeout(control, left)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use naby_core::system::{MonitorNetwork, MonitorSnapshot};

    fn snap(timestamp: i64, nets: &[(&str, i64, i64)]) -> MonitorSnapshot {
        let mut s = MonitorSnapshot::unavailable("x");
        s.timestamp = timestamp;
        s.idle = Some(0);
        s.cpu_total = Some(0);
        s.errors.clear();
        s.networks = nets
            .iter()
            .map(|(id, rx, tx)| MonitorNetwork {
                id: (*id).into(),
                name: format!("Adapter {id}"),
                received: *rx,
                sent: *tx,
            })
            .collect();
        s
    }

    #[test]
    fn options_follow_the_selected_tab() {
        let overview = SystemOptions::for_tab("overview", true, true, true);
        assert!(overview.hardware && overview.gpu_only && !overview.network && !overview.processes);
        let hardware = SystemOptions::for_tab("hardware", true, true, true);
        assert!(hardware.hardware && !hardware.gpu_only);
        let network = SystemOptions::for_tab("network", true, true, true);
        assert!(network.network && !network.hardware);
        assert!(!SystemOptions::for_tab("network", false, true, true).network);
        let processes = SystemOptions::for_tab("processes", true, true, true);
        assert!(processes.processes && processes.process_counts);
        assert!(!SystemOptions::for_tab("processes", true, true, false).processes);
        assert!(SystemOptions::for_tab("storage", true, true, true).storage);
    }

    #[test]
    fn first_sample_has_no_rates_then_deltas_use_elapsed_time() {
        let first = snap(0, &[("a", 100, 10)]);
        let rates = net_rates(&first, None, TICKS_PER_SECOND);
        assert_eq!(rates[0].received, None, "sampling, never a fabricated zero");
        let second = snap(2_000_000, &[("a", 1100, 10), ("b", 5, 5)]);
        let rates = net_rates(&second, Some(&first), TICKS_PER_SECOND);
        assert_eq!(rates[0].received, Some(500.0));
        assert_eq!(rates[0].sent, Some(0.0), "unchanged counter is a real zero");
        assert_eq!(rates[1].received, None, "a new adapter has no baseline");
        let reset = snap(4_000_000, &[("a", 10, 10)]);
        assert_eq!(
            net_rates(&reset, Some(&second), TICKS_PER_SECOND)[0].received,
            None
        );
    }

    #[test]
    fn state_tracks_cpu_history_and_resets_baseline_but_keeps_the_sparkline() {
        let mut first = snap(0, &[]);
        first.idle = Some(10);
        first.cpu_total = Some(100);
        let mut second = snap(2_000_000, &[]);
        second.idle = Some(40);
        second.cpu_total = Some(200);
        let state = SystemState::default().push(first.clone(), None);
        assert_eq!(state.cpu, None);
        let state = state.push(second, Some(&first));
        assert_eq!(state.cpu, Some(70.0));
        assert_eq!(state.cpu_history.len(), 2);
        assert_eq!(state.reads, 2);
        let reset = state.without_baseline();
        assert!(reset.latest.is_none() && reset.cpu.is_none());
        assert_eq!(reset.cpu_history.len(), 2);
    }

    #[test]
    fn process_names_match_dotnet() {
        assert_eq!(process_name(0, "[System Process]"), "Idle");
        assert_eq!(process_name(4, "System"), "System");
        assert_eq!(process_name(88, "Chrome.EXE"), "Chrome");
        assert_eq!(process_name(88, "app.exe.bak"), "app.exe.bak");
    }

    #[test]
    fn drive_mask_lists_letters() {
        assert_eq!(drive_roots(0b101), vec!["A:\\", "C:\\"]);
        assert!(drive_roots(0).is_empty());
        assert_eq!(drive_roots(1 << 25), vec!["Z:\\"]);
    }

    #[test]
    fn wql_select_lists_become_columns() {
        assert_eq!(
            select_columns("SELECT Name,NumberOfCores,NumberOfLogicalProcessors,MaxClockSpeed FROM Win32_Processor"),
            vec!["Name", "NumberOfCores", "NumberOfLogicalProcessors", "MaxClockSpeed"]
        );
        assert_eq!(
            select_columns("SELECT Name, SensorType, Value FROM Sensor"),
            vec!["Name", "SensorType", "Value"]
        );
        assert!(select_columns("garbage").is_empty());
    }
}
