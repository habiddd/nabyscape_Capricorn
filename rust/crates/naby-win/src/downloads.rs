//! Download arrival watcher (C# `DownloadArrivals` OS half). A `ReadDirectoryChangesW` thread
//! feeds file events into `naby_core::downloads::DownloadInference`; [`DiskFs`] is the real
//! `DownloadFs` (directory listing, signatures, and a read probe that fails while another
//! process still holds the file open for writing, like `FileShare.Read`).
#![cfg_attr(not(windows), allow(dead_code))]

use naby_core::downloads::{DownloadError, DownloadFs, FileSig, Probe, MAX_KNOWN};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// What the watcher reports to the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEvent {
    /// Files that finished arriving, sorted case-insensitively.
    Arrived(Vec<String>),
    /// Monitoring stopped; the message is shown to the user (C# `failed`).
    Failed(String),
}

/// Length and last-write time of a file in the units `FileSig` uses.
pub fn signature(meta: &fs::Metadata) -> FileSig {
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    FileSig {
        size: meta.len(),
        modified_ms,
    }
}

/// The real file system behind a download folder.
pub struct DiskFs {
    folder: PathBuf,
}

impl DiskFs {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
        }
    }

    /// The folder's current files with their signatures, for `DownloadInference::configure`.
    /// More than [`MAX_KNOWN`] files fails closed.
    pub fn baseline(&self) -> Result<Vec<(String, FileSig)>, DownloadError> {
        let entries = fs::read_dir(&self.folder).map_err(|e| DownloadError::Io(e.to_string()))?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            if out.len() == MAX_KNOWN {
                return Err(DownloadError::TooManyFiles);
            }
            out.push((
                entry.path().to_string_lossy().into_owned(),
                signature(&meta),
            ));
        }
        Ok(out)
    }
}

/// Opens `path` for reading while denying writers (`FileShare.Read`).
fn open_shared_read(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        options.share_mode(FILE_SHARE_READ);
    }
    options.open(path)
}

impl DownloadFs for DiskFs {
    fn list(&mut self) -> Result<Vec<String>, String> {
        let entries = fs::read_dir(&self.folder).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            if out.len() > MAX_KNOWN {
                break;
            }
            if entry.metadata().is_ok_and(|m| m.is_file()) {
                out.push(entry.path().to_string_lossy().into_owned());
            }
        }
        Ok(out)
    }

    fn probe(&mut self, path: &str) -> Probe {
        let meta = match fs::metadata(path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Probe::Missing,
            Err(e) if e.kind() == ErrorKind::PermissionDenied => return Probe::Denied,
            Err(_) => return Probe::Missing,
        };
        let sig = signature(&meta);
        match open_shared_read(Path::new(path)) {
            Ok(_) => Probe::Ready(sig),
            Err(e) if e.kind() == ErrorKind::PermissionDenied => Probe::Denied,
            Err(e) if e.kind() == ErrorKind::NotFound => Probe::Missing,
            Err(_) => Probe::Locked(sig),
        }
    }

    fn folder_exists(&mut self) -> bool {
        self.folder.is_dir()
    }
}

/// Joins a watcher-relative name onto the folder.
pub fn join(folder: &str, name: &str) -> String {
    format!("{}\\{}", folder.trim_end_matches(['\\', '/']), name)
}

/// One decoded `FILE_NOTIFY_INFORMATION` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub action: u32,
    pub name: String,
}

pub const ACTION_ADDED: u32 = 1;
pub const ACTION_REMOVED: u32 = 2;
pub const ACTION_MODIFIED: u32 = 3;
pub const ACTION_RENAMED_OLD: u32 = 4;
pub const ACTION_RENAMED_NEW: u32 = 5;

/// Decodes the buffer `ReadDirectoryChangesW` filled. Malformed records end the walk.
pub fn parse_notifications(buffer: &[u8]) -> Vec<Notification> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(header) = buffer.get(at..at + 12) {
        let next = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let action = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let length = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
        let Some(raw) = buffer.get(at + 12..at + 12 + length) else {
            break;
        };
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        out.push(Notification {
            action,
            name: String::from_utf16_lossy(&units),
        });
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

/// Feeds decoded notifications into the inference (renames arrive as an old/new pair).
pub fn apply_notifications(
    inference: &mut naby_core::downloads::DownloadInference,
    folder: &str,
    notifications: &[Notification],
    now: chrono::DateTime<chrono::Utc>,
) {
    let mut old: Option<String> = None;
    for n in notifications {
        let path = join(folder, &n.name);
        match n.action {
            ACTION_ADDED | ACTION_MODIFIED => inference.on_changed(&path, now),
            ACTION_REMOVED => inference.on_deleted(&path),
            ACTION_RENAMED_OLD => old = Some(path),
            ACTION_RENAMED_NEW => match old.take() {
                Some(from) => inference.on_renamed(&from, &path, now),
                None => inference.on_changed(&path, now),
            },
            _ => {}
        }
    }
    if let Some(from) = old {
        inference.on_deleted(&from);
    }
}

#[cfg(windows)]
pub use os::{known_folder, DownloadWatcher};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::svc_util::{pcwstr, wide, OwnedHandle};
    use crate::wake::Waker;
    use chrono::Utc;
    use naby_core::downloads::DownloadInference;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};
    use windows::core::Error;
    use windows::Win32::Foundation::{
        GetLastError, ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

    /// `SHGetKnownFolderPath(FOLDERID_Downloads)`.
    pub fn known_folder() -> Option<String> {
        // SAFETY: the returned task-allocated string is copied and then freed exactly once.
        unsafe {
            let path = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None).ok()?;
            let text = crate::svc_util::from_pwstr(path.0);
            CoTaskMemFree(Some(path.0.cast_const().cast()));
            (!text.is_empty()).then_some(text)
        }
    }

    struct Shared {
        events: Mutex<VecDeque<DownloadEvent>>,
        stop: OwnedHandle,
        wake: Waker,
    }

    impl Shared {
        fn push(&self, event: DownloadEvent) {
            self.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(event);
            (self.wake)();
        }
    }

    struct Running {
        shared: Arc<Shared>,
        thread: JoinHandle<()>,
        folder: String,
    }

    /// Watches one folder while monitoring is enabled; the thread exists only then.
    pub struct DownloadWatcher {
        wake: Waker,
        running: Option<Running>,
        pending: VecDeque<DownloadEvent>,
    }

    impl DownloadWatcher {
        pub fn start(wake: Waker) -> Self {
            Self {
                wake,
                running: None,
                pending: VecDeque::new(),
            }
        }

        pub fn running(&self) -> bool {
            self.running.is_some()
        }

        pub fn folder(&self) -> &str {
            self.running.as_ref().map_or("", |r| r.folder.as_str())
        }

        /// C# `Configure`: starts, retargets or stops monitoring. `folder` of `None` uses the
        /// Downloads known folder. Re-configuring the same folder while running is a no-op.
        /// Failures arrive as `DownloadEvent::Failed`.
        pub fn configure(&mut self, enabled: bool, folder: Option<&str>) {
            let target = folder.map(str::to_owned).or_else(known_folder);
            if enabled {
                if let (Some(r), Some(t)) = (&self.running, &target) {
                    if r.folder
                        .eq_ignore_ascii_case(t.trim_end_matches(['\\', '/']))
                    {
                        return;
                    }
                }
            }
            self.stop();
            if !enabled {
                return;
            }
            let Some(target) = target else {
                self.pending.push_back(DownloadEvent::Failed(
                    "Downloads folder unavailable.".to_owned(),
                ));
                (self.wake)();
                return;
            };
            let folder = target.trim_end_matches(['\\', '/']).to_owned();
            // SAFETY: creates an unnamed manual-reset event; ownership moves into OwnedHandle.
            let stop = match unsafe { CreateEventW(None, true, false, None) } {
                Ok(h) => OwnedHandle(h),
                Err(e) => {
                    self.pending.push_back(DownloadEvent::Failed(e.message()));
                    (self.wake)();
                    return;
                }
            };
            let shared = Arc::new(Shared {
                events: Mutex::new(VecDeque::new()),
                stop,
                wake: self.wake.clone(),
            });
            let thread_shared = shared.clone();
            let thread_folder = folder.clone();
            match std::thread::Builder::new()
                .name("NabyScape downloads".into())
                .spawn(move || run(thread_shared, thread_folder))
            {
                Ok(thread) => {
                    self.running = Some(Running {
                        shared,
                        thread,
                        folder,
                    });
                }
                Err(e) => {
                    self.pending.push_back(DownloadEvent::Failed(e.to_string()));
                    (self.wake)();
                }
            }
        }

        /// Events since the last call.
        pub fn drain(&mut self) -> Vec<DownloadEvent> {
            let mut out: Vec<DownloadEvent> = self.pending.drain(..).collect();
            if let Some(r) = &self.running {
                out.extend(
                    r.shared
                        .events
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .drain(..),
                );
                // A failed watcher has stopped itself; forget it so it can be restarted.
                if out.iter().any(|e| matches!(e, DownloadEvent::Failed(_)))
                    && r.thread.is_finished()
                {
                    self.stop();
                }
            }
            out
        }

        fn stop(&mut self) {
            if let Some(r) = self.running.take() {
                // SAFETY: the stop event is owned by `shared`, which outlives this call.
                unsafe {
                    let _ = SetEvent(r.shared.stop.0);
                }
                let _ = r.thread.join();
            }
        }
    }

    impl Drop for DownloadWatcher {
        fn drop(&mut self) {
            self.stop();
        }
    }

    /// An outstanding overlapped `ReadDirectoryChangesW`; cancelling and draining it before the
    /// buffer is freed is what makes the kernel's writes into `buffer` safe.
    struct Watch {
        dir: OwnedHandle,
        event: OwnedHandle,
        overlapped: Box<OVERLAPPED>,
        buffer: Box<[u32; 4096]>,
        in_flight: bool,
    }

    impl Watch {
        fn open(folder: &str) -> Result<Self, Error> {
            let path = wide(folder);
            // SAFETY: `path` is NUL-terminated; the handle is wrapped for closing.
            let dir = unsafe {
                CreateFileW(
                    pcwstr(&path),
                    FILE_LIST_DIRECTORY.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    None,
                )
            }?;
            let dir = OwnedHandle(dir);
            // SAFETY: unnamed manual-reset event.
            let event = OwnedHandle(unsafe { CreateEventW(None, true, false, None) }?);
            let mut watch = Self {
                dir,
                overlapped: Box::new(OVERLAPPED {
                    hEvent: event.0,
                    ..Default::default()
                }),
                event,
                buffer: Box::new([0; 4096]),
                in_flight: false,
            };
            watch.issue()?;
            Ok(watch)
        }

        fn issue(&mut self) -> Result<(), Error> {
            *self.overlapped = OVERLAPPED {
                hEvent: self.event.0,
                ..Default::default()
            };
            // SAFETY: `buffer` and `overlapped` are boxed (stable addresses) and stay alive
            // until the operation completes: `Drop` cancels and waits before freeing them.
            let result = unsafe {
                ReadDirectoryChangesW(
                    self.dir.0,
                    self.buffer.as_mut_ptr().cast(),
                    (self.buffer.len() * 4) as u32,
                    false,
                    FILE_NOTIFY_CHANGE_FILE_NAME
                        | FILE_NOTIFY_CHANGE_SIZE
                        | FILE_NOTIFY_CHANGE_LAST_WRITE,
                    None,
                    Some(&mut *self.overlapped),
                    None,
                )
            };
            // Overlapped calls report success immediately or via ERROR_IO_PENDING.
            match result {
                Ok(()) => {
                    self.in_flight = true;
                    Ok(())
                }
                // SAFETY: reads the calling thread's last error.
                Err(e) if unsafe { GetLastError() } == ERROR_IO_PENDING => {
                    let _ = e;
                    self.in_flight = true;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }

        /// Collects a finished read: decoded notifications, or `None` for a buffer overflow.
        fn complete(&mut self) -> Result<Option<Vec<Notification>>, Error> {
            let mut transferred = 0u32;
            // SAFETY: the operation was issued on this handle with this OVERLAPPED.
            unsafe { GetOverlappedResult(self.dir.0, &*self.overlapped, &mut transferred, false) }?;
            self.in_flight = false;
            let notifications = if transferred == 0 {
                None
            } else {
                let bytes: &[u8] = {
                    // SAFETY: the buffer is `len * 4` initialised bytes and the kernel is done
                    // writing to it (the operation completed).
                    unsafe {
                        std::slice::from_raw_parts(
                            self.buffer.as_ptr().cast::<u8>(),
                            (transferred as usize).min(self.buffer.len() * 4),
                        )
                    }
                };
                Some(parse_notifications(bytes))
            };
            self.issue()?;
            Ok(notifications)
        }
    }

    impl Drop for Watch {
        fn drop(&mut self) {
            if self.in_flight {
                let mut n = 0u32;
                // SAFETY: cancels our own outstanding operation, then waits for it to retire so
                // the kernel no longer references the buffer or OVERLAPPED being freed.
                unsafe {
                    let _ = CancelIoEx(self.dir.0, Some(&*self.overlapped));
                    let _ = GetOverlappedResult(self.dir.0, &*self.overlapped, &mut n, true);
                }
            }
        }
    }

    fn run(shared: Arc<Shared>, folder: String) {
        let mut fs = DiskFs::new(&folder);
        let mut inference = DownloadInference::new();
        let baseline = match fs.baseline() {
            Ok(b) => b,
            Err(e) => {
                shared.push(DownloadEvent::Failed(e.to_string()));
                return;
            }
        };
        if let Err(e) = inference.configure(true, &folder, baseline) {
            shared.push(DownloadEvent::Failed(e.to_string()));
            return;
        }
        let mut watch = match Watch::open(&folder) {
            Ok(w) => w,
            Err(e) => {
                shared.push(DownloadEvent::Failed(e.message()));
                return;
            }
        };
        let mut last_tick = Instant::now() - Duration::from_secs(1);
        loop {
            // A quarter second of precision only while a file is settling; otherwise the
            // thread sleeps long enough to still notice a disconnected folder.
            let wait_ms: u32 = if inference.candidates() > 0 {
                500
            } else {
                5000
            };
            let handles: [HANDLE; 2] = [shared.stop.0, watch.event.0];
            // SAFETY: both handles are valid events owned by this thread's state.
            let signal = unsafe { WaitForMultipleObjects(&handles, false, wait_ms) };
            if signal == WAIT_OBJECT_0 {
                return;
            }
            let mut force_tick = false;
            if signal.0 == WAIT_OBJECT_0.0 + 1 {
                match watch.complete() {
                    Ok(Some(list)) => {
                        apply_notifications(&mut inference, &folder, &list, Utc::now());
                    }
                    Ok(None) => {
                        inference.request_reconcile();
                        force_tick = true;
                    }
                    Err(e) => {
                        shared.push(DownloadEvent::Failed(e.message()));
                        return;
                    }
                }
            } else if signal != WAIT_TIMEOUT {
                shared.push(DownloadEvent::Failed(
                    "Download folder watcher failed.".to_owned(),
                ));
                return;
            }
            if force_tick
                || signal == WAIT_TIMEOUT
                || last_tick.elapsed() >= Duration::from_millis(500)
            {
                last_tick = Instant::now();
                match inference.tick(Utc::now(), &mut fs) {
                    Ok(batch) if !batch.is_empty() => shared.push(DownloadEvent::Arrived(batch)),
                    Ok(_) => {}
                    Err(e) => {
                        shared.push(DownloadEvent::Failed(e.to_string()));
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use naby_core::downloads::DownloadInference;

    #[test]
    fn disk_fs_lists_files_and_probes_readable_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("report.pdf");
        fs::write(&file, b"hello").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let mut disk = DiskFs::new(dir.path());
        let listed = disk.list().unwrap();
        assert_eq!(listed, vec![file.to_string_lossy().into_owned()]);
        match disk.probe(&listed[0]) {
            Probe::Ready(sig) => assert_eq!(sig.size, 5),
            other => panic!("expected ready, got {other:?}"),
        }
        assert_eq!(
            disk.probe(&dir.path().join("gone").to_string_lossy()),
            Probe::Missing
        );
        assert!(disk.folder_exists());
        let baseline = disk.baseline().unwrap();
        assert_eq!(baseline.len(), 1);
        assert_eq!(baseline[0].1.size, 5);
    }

    #[test]
    fn missing_folder_fails_listing_and_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let mut disk = DiskFs::new(dir.path().join("nope"));
        assert!(disk.list().is_err());
        assert!(matches!(disk.baseline(), Err(DownloadError::Io(_))));
        assert!(!disk.folder_exists());
    }

    #[test]
    fn notification_buffer_decodes_and_pairs_renames() {
        fn record(next: u32, action: u32, name: &str) -> Vec<u8> {
            let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let mut v = Vec::new();
            v.extend_from_slice(&next.to_le_bytes());
            v.extend_from_slice(&action.to_le_bytes());
            v.extend_from_slice(&(units.len() as u32).to_le_bytes());
            v.extend_from_slice(&units);
            while v.len() % 4 != 0 {
                v.push(0);
            }
            v
        }
        let first = record(0, ACTION_RENAMED_OLD, "a.crdownload");
        let mut buffer = record(first.len() as u32, ACTION_RENAMED_OLD, "a.crdownload");
        buffer.extend_from_slice(&record(0, ACTION_RENAMED_NEW, "a.zip"));
        let list = parse_notifications(&buffer);
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[1],
            Notification {
                action: ACTION_RENAMED_NEW,
                name: "a.zip".into()
            }
        );

        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let mut inference = DownloadInference::new();
        inference.configure(true, "C:\\Dl", Vec::new()).unwrap();
        apply_notifications(&mut inference, "C:\\Dl", &list, t0);
        assert_eq!(
            inference.candidates(),
            1,
            "the temporary name is dropped, the final one tracked"
        );
        assert!(parse_notifications(&[1, 2, 3]).is_empty());
        assert!(parse_notifications(&[]).is_empty());
    }

    #[test]
    fn a_settled_file_arrives_through_the_real_probe() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_string_lossy().into_owned();
        let mut disk = DiskFs::new(dir.path());
        let mut inference = DownloadInference::new();
        inference
            .configure(true, &folder, disk.baseline().unwrap())
            .unwrap();
        let file = dir.path().join("setup.exe");
        fs::write(&file, b"payload").unwrap();
        let path = file.to_string_lossy().into_owned();
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        inference.on_changed(&path, t0);
        assert!(
            inference.tick(t0, &mut disk).unwrap().is_empty(),
            "size seen, not yet stable"
        );
        assert!(inference
            .tick(t0 + Duration::seconds(1), &mut disk)
            .unwrap()
            .is_empty());
        assert_eq!(
            inference
                .tick(t0 + Duration::seconds(3), &mut disk)
                .unwrap(),
            vec![path]
        );
    }

    #[test]
    fn deleting_the_folder_disconnects() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("dl");
        fs::create_dir(&sub).unwrap();
        let mut disk = DiskFs::new(&sub);
        let mut inference = DownloadInference::new();
        inference
            .configure(true, &sub.to_string_lossy(), Vec::new())
            .unwrap();
        fs::remove_dir(&sub).unwrap();
        let t0 = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert!(inference.tick(t0, &mut disk).is_err());
        assert!(!inference.running());
    }
}
