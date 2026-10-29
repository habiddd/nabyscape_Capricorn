//! Per-app and per-output volume mixer (`AudioMixer.cs`, `MixerAppIcon.cs`).
//!
//! A worker thread enumerates sessions on request (`refresh`) and applies writes, so the UI
//! thread never blocks on COM. Pure rules (bounds, adjust, controllable states) live in
//! `naby_core::media::mixer`.
#![allow(dead_code)] // consumed by the app shell, which is wired separately

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use naby_core::media::mixer::{
    adjust, bounded_volume, is_controllable_session, MixerItem, MixerResult, MixerScope,
    MixerSnapshot, MixerTarget,
};
use windows::core::{Interface, GUID};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
};
use windows::Win32::System::Com::CLSCTX_ALL;

use crate::audio_logic::session_display_name;
use crate::devices::{self, take_pwstr, ComApartment};
use crate::wake::Waker;

/// Largest icon edge kept (pixels).
const ICON_MAX: i32 = 64;
/// Cached process paths / icons are bounded like the C# dictionary.
const CACHE_LIMIT: usize = 256;
const ICON_CACHE_LIMIT: usize = 96;

/// A small decoded application icon, straight (non-premultiplied) RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconRgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

enum Msg {
    Refresh,
    Set {
        target: MixerTarget,
        level: Option<f64>,
        muted: Option<bool>,
    },
    AdjustAll {
        scope: MixerScope,
        delta: f64,
    },
    Icon(String),
    Shutdown,
}

#[derive(Default)]
struct Shared {
    snapshot: Mutex<MixerSnapshot>,
    last_result: Mutex<Option<MixerResult>>,
    last_error: Mutex<Option<String>>,
    icons: Mutex<HashMap<String, Option<Arc<IconRgba>>>>,
}

pub struct MixerService {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    requested: Mutex<HashSet<String>>,
    thread: Option<JoinHandle<()>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl MixerService {
    /// Starts the worker. Nothing is enumerated until `refresh` is called.
    pub fn start(wake: Waker) -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("naby-mixer".into())
            .spawn(move || worker(rx, worker_shared, wake))
            .ok();
        MixerService {
            shared,
            tx,
            requested: Mutex::new(HashSet::new()),
            thread,
        }
    }

    /// Latest enumeration (empty before the first `refresh`).
    pub fn snapshot(&self) -> MixerSnapshot {
        lock(&self.shared.snapshot).clone()
    }

    /// Re-enumerates outputs and app sessions; the waker fires when something changed.
    pub fn refresh(&self) {
        let _ = self.tx.send(Msg::Refresh);
    }

    /// Sets volume and/or mute of one target; the error text shows in `last_error`.
    pub fn set(&self, target: MixerTarget, level: Option<f64>, muted: Option<bool>) {
        let _ = self.tx.send(Msg::Set {
            target,
            level,
            muted,
        });
    }

    /// Adds `delta` to every item of `scope`; the outcome shows in `last_result`.
    pub fn adjust(&self, scope: MixerScope, delta: f64) {
        let _ = self.tx.send(Msg::AdjustAll { scope, delta });
    }

    pub fn last_result(&self) -> Option<MixerResult> {
        lock(&self.shared.last_result).clone()
    }

    pub fn last_error(&self) -> Option<String> {
        lock(&self.shared.last_error).clone()
    }

    /// Icon of a process image once extracted; the first call queues the extraction and the
    /// waker fires when it is ready. `None` means pending or unavailable (show the monogram).
    pub fn icon(&self, process_path: &str) -> Option<Arc<IconRgba>> {
        if let Some(entry) = lock(&self.shared.icons).get(process_path) {
            return entry.clone();
        }
        if lock(&self.requested).insert(process_path.to_owned()) {
            let _ = self.tx.send(Msg::Icon(process_path.to_owned()));
        }
        None
    }
}

impl Drop for MixerService {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn worker(rx: Receiver<Msg>, shared: Arc<Shared>, wake: Waker) {
    let _com = ComApartment::mta();
    let mut paths: HashMap<String, Option<String>> = HashMap::new();
    while let Ok(msg) = rx.recv() {
        let mut changed = false;
        match msg {
            Msg::Shutdown => break,
            Msg::Refresh => changed |= publish(&shared, snapshot(&mut paths)),
            Msg::Set {
                target,
                level,
                muted,
            } => {
                let error = set(&target, level, muted);
                *lock(&shared.last_error) = error;
                changed |= publish(&shared, snapshot(&mut paths));
            }
            Msg::AdjustAll { scope, delta } => {
                let snap = snapshot(&mut paths);
                let result = match adjust(&snap, scope, delta, |t, v| set(t, Some(v), None)) {
                    Ok(mut r) => {
                        let mut errors = snap.errors.clone();
                        errors.append(&mut r.errors);
                        r.errors = errors;
                        r
                    }
                    Err(e) => MixerResult {
                        changed: 0,
                        failed: 0,
                        errors: vec![e.to_string()],
                    },
                };
                *lock(&shared.last_result) = Some(result);
                publish(&shared, snapshot(&mut paths));
                changed = true;
            }
            Msg::Icon(path) => {
                let icon = extract_icon(&path, ICON_MAX).map(Arc::new);
                let mut icons = lock(&shared.icons);
                if icons.len() >= ICON_CACHE_LIMIT {
                    icons.clear();
                }
                icons.insert(path, icon);
                changed = true;
            }
        }
        if changed {
            wake();
        }
    }
}

fn publish(shared: &Shared, next: MixerSnapshot) -> bool {
    let mut current = lock(&shared.snapshot);
    if *current == next {
        false
    } else {
        *current = next;
        true
    }
}

/// Calls `visit` for every session of one output; `on_error` collects per-session failures.
fn with_sessions(
    en: &IMMDeviceEnumerator,
    output_id: &str,
    mut visit: impl FnMut(&IAudioSessionControl2, &ISimpleAudioVolume) -> Result<(), String>,
    mut on_error: impl FnMut(String),
) -> Result<(), String> {
    let device = devices::device_by_id(en, output_id).map_err(|e| e.to_string())?;
    // SAFETY: plain COM activation/enumeration on live interfaces; every interface is owned
    // and released when it goes out of scope.
    unsafe {
        let manager: IAudioSessionManager2 = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;
        let list = manager.GetSessionEnumerator().map_err(|e| e.to_string())?;
        let count = list.GetCount().map_err(|e| e.to_string())?;
        for i in 0..count {
            let control = match list.GetSession(i) {
                Ok(c) => c,
                Err(e) => {
                    on_error(e.to_string());
                    continue;
                }
            };
            let (Ok(control2), Ok(volume)) = (
                control.cast::<IAudioSessionControl2>(),
                control.cast::<ISimpleAudioVolume>(),
            ) else {
                on_error("session interfaces unavailable".to_owned());
                continue;
            };
            if let Err(e) = visit(&control2, &volume) {
                on_error(e);
            }
        }
    }
    Ok(())
}

fn snapshot(path_cache: &mut HashMap<String, Option<String>>) -> MixerSnapshot {
    let mut snap = MixerSnapshot::default();
    let en = match devices::enumerator() {
        Ok(en) => en,
        Err(e) => {
            snap.errors.push(format!("Audio unavailable: {e}"));
            return snap;
        }
    };
    let devs = match devices::active_devices(&en, windows::Win32::Media::Audio::eRender) {
        Ok(d) => d,
        Err(e) => {
            snap.errors.push(format!("Audio unavailable: {e}"));
            return snap;
        }
    };
    for device in devs {
        let mut id = String::new();
        let mut name = "Output".to_owned();
        let read = (|| -> windows::core::Result<()> {
            id = devices::device_id(&device)?;
            name = devices::friendly_name(&device);
            // SAFETY: activation and reads on a live endpoint.
            let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None)? };
            let (level, muted) =
                unsafe { (volume.GetMasterVolumeLevelScalar()?, volume.GetMute()?) };
            snap.outputs.push(MixerItem {
                target: MixerTarget::output(id.clone()),
                name: name.clone(),
                output_name: name.clone(),
                volume: f64::from(level),
                muted: muted.as_bool(),
                process_path: None,
            });
            Ok(())
        })();
        if let Err(e) = read {
            snap.errors.push(format!("{name}: {e}"));
        }
        if id.is_empty() {
            continue;
        }
        let mut apps = Vec::new();
        let mut errors = Vec::new();
        let output_name = name.clone();
        let result = with_sessions(
            &en,
            &id,
            |session, volume| {
                // SAFETY: plain COM reads on live session interfaces; strings are freed by `take_pwstr`.
                unsafe {
                    let state = session.GetState().map_err(|e| e.to_string())?;
                    if !is_controllable_session(state.0) {
                        return Ok(());
                    }
                    let instance = take_pwstr(
                        session
                            .GetSessionInstanceIdentifier()
                            .map_err(|e| e.to_string())?,
                    );
                    let display = take_pwstr(session.GetDisplayName().map_err(|e| e.to_string())?);
                    let pid = session.GetProcessId().map_err(|e| e.to_string())?;
                    let system = session.IsSystemSoundsSession().0 == 0;
                    let key = format!("{pid}|{instance}");
                    let path = path_cache
                        .entry(key)
                        .or_insert_with(|| devices::process_image(pid))
                        .clone();
                    let process_name = path
                        .as_deref()
                        .map(|p| crate::audio_logic::file_stem(p).to_owned());
                    let display =
                        session_display_name(&display, system, process_name.as_deref(), pid);
                    let level = volume.GetMasterVolume().map_err(|e| e.to_string())?;
                    let muted = volume.GetMute().map_err(|e| e.to_string())?.as_bool();
                    apps.push(MixerItem {
                        target: MixerTarget::app(id.clone(), instance),
                        name: display,
                        output_name: output_name.clone(),
                        volume: f64::from(level),
                        muted,
                        process_path: path,
                    });
                }
                Ok(())
            },
            |e| errors.push(format!("{name} session: {e}")),
        );
        if path_cache.len() >= CACHE_LIMIT {
            path_cache.clear();
        }
        snap.apps.append(&mut apps);
        snap.errors.append(&mut errors);
        if let Err(e) = result {
            snap.errors.push(format!("{name} apps: {e}"));
        }
    }
    snap.sort();
    snap
}

/// Writes volume and/or mute. Returns the error text, or `None` on success.
fn set(target: &MixerTarget, level: Option<f64>, muted: Option<bool>) -> Option<String> {
    let level = match level.map(bounded_volume).transpose() {
        Ok(l) => l,
        Err(e) => return Some(e.to_string()),
    };
    let en = match devices::enumerator() {
        Ok(en) => en,
        Err(e) => return Some(e.to_string()),
    };
    let device = match devices::device_by_id(&en, &target.output_id) {
        Ok(d) => d,
        Err(e) => return Some(e.to_string()),
    };
    if !devices::is_active_render(&en, &target.output_id) {
        return Some("Output disconnected. Refresh and try again.".to_owned());
    }
    let context = GUID::zeroed();
    if target.scope == MixerScope::Outputs {
        // SAFETY: plain COM calls on the live endpoint.
        let r = unsafe {
            device
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                .and_then(|v| {
                    if let Some(l) = level {
                        v.SetMasterVolumeLevelScalar(l as f32, &context)?;
                    }
                    if let Some(m) = muted {
                        v.SetMute(m, &context)?;
                    }
                    Ok(())
                })
        };
        return r.err().map(|e| e.to_string());
    }
    let mut found = false;
    let mut failure: Option<String> = None;
    let walked = with_sessions(
        &en,
        &target.output_id,
        |session, volume| {
            // SAFETY: plain COM calls on live session interfaces.
            unsafe {
                let id = take_pwstr(
                    session
                        .GetSessionInstanceIdentifier()
                        .map_err(|e| e.to_string())?,
                );
                if id != target.session_id {
                    return Ok(());
                }
                let state = session.GetState().map_err(|e| e.to_string())?;
                if !is_controllable_session(state.0) {
                    return Ok(());
                }
                found = true;
                if let Some(l) = level {
                    volume
                        .SetMasterVolume(l as f32, &context)
                        .map_err(|e| e.to_string())?;
                }
                if let Some(m) = muted {
                    volume.SetMute(m, &context).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        },
        |e| failure = failure.take().or(Some(e)),
    );
    if let Err(e) = walked {
        return Some(e);
    }
    if let Some(e) = failure {
        return Some(e);
    }
    if found {
        None
    } else {
        Some("App session ended. Refresh and try again.".to_owned())
    }
}

/// Large shell icon of `path` as RGBA, at most `max` pixels per edge.
pub fn extract_icon(path: &str, max: i32) -> Option<IconRgba> {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

    let wide_path = devices::wide(path);
    let mut info = SHFILEINFOW::default();
    // SAFETY: SHGetFileInfoW with SHGFI_ICON fills `info` and returns an icon the caller
    // owns; every GDI object obtained below is released on all paths before returning.
    unsafe {
        let hit = SHGetFileInfoW(
            windows::core::PCWSTR(wide_path.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if hit == 0 || info.hIcon.is_invalid() {
            return None;
        }
        let icon = info.hIcon;
        let mut icon_info = ICONINFO::default();
        let mut out = None;
        if GetIconInfo(icon, &mut icon_info).is_ok() {
            let mut bmp = BITMAP::default();
            let got = GetObjectW(
                HGDIOBJ(icon_info.hbmColor.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bmp as *mut BITMAP as *mut _),
            );
            let (w, h) = (bmp.bmWidth, bmp.bmHeight);
            if got != 0 && w > 0 && h > 0 && w <= max && h <= max {
                let dc = CreateCompatibleDC(None);
                let mut header = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut color = vec![0u8; (w * h * 4) as usize];
                let rows = GetDIBits(
                    dc,
                    icon_info.hbmColor,
                    0,
                    h as u32,
                    Some(color.as_mut_ptr() as *mut _),
                    &mut header,
                    DIB_RGB_COLORS,
                );
                if rows != 0 {
                    if color.chunks_exact(4).all(|p| p[3] == 0) {
                        // Old-style icon without alpha: take transparency from the mask.
                        let mut mask = vec![0u8; (w * h * 4) as usize];
                        let mut mask_header = header;
                        let mrows = GetDIBits(
                            dc,
                            icon_info.hbmMask,
                            0,
                            h as u32,
                            Some(mask.as_mut_ptr() as *mut _),
                            &mut mask_header,
                            DIB_RGB_COLORS,
                        );
                        for (c, m) in color.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                            c[3] = if mrows != 0 && m[0] != 0 { 0 } else { 255 };
                        }
                    }
                    for p in color.chunks_exact_mut(4) {
                        p.swap(0, 2);
                    }
                    out = Some(IconRgba {
                        width: w as u32,
                        height: h as u32,
                        rgba: color,
                    });
                }
                let _ = DeleteDC(dc);
            }
            let _ = DeleteObject(HGDIOBJ(icon_info.hbmColor.0));
            let _ = DeleteObject(HGDIOBJ(icon_info.hbmMask.0));
        }
        let _ = DestroyIcon(icon);
        out
    }
}
