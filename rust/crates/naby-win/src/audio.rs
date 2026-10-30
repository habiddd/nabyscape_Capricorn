//! Default output volume/mute, default microphone mute and output switching
//! (`AudioService` in `Services.cs` plus `AudioDevices.cs` and `AudioNotifications.cs`).
//!
//! One worker thread owns every COM object. It sleeps on a channel; Core Audio callbacks
//! (endpoint volume, device arrival/removal/default change) post messages to it, so nothing
//! is polled except a slow safety re-read every minute.
#![allow(dead_code)] // consumed by the app shell, which is wired separately

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use naby_core::media::AudioSnapshot;
use windows::core::{implement, Result as WinResult, GUID};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eRender, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::CLSCTX_ALL;

use crate::audio_logic::{AudioOperation, CLOSED_MESSAGE};
use crate::devices::{self, ComApartment};
use crate::wake::Waker;

/// Slow safety net in case a notification was missed (the C# service re-read after 60 s).
const AUDIT: Duration = Duration::from_secs(60);

enum Msg {
    /// A device appeared, vanished, changed state or became default.
    Devices,
    /// A property of one endpoint changed (name, ...).
    Property(String),
    Volume {
        generation: u64,
        volume: f32,
        muted: bool,
    },
    Microphone {
        generation: u64,
        muted: bool,
    },
    SetVolume(f32),
    ToggleMute,
    ToggleMicrophoneMute,
    SetOutput(String),
    Refresh,
    Shutdown,
}

#[derive(Default)]
struct Shared {
    snapshot: Mutex<AudioSnapshot>,
    last_error: Mutex<Option<String>>,
}

/// Output volume, microphone mute and default-output switching.
pub struct AudioService {
    shared: Arc<Shared>,
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl AudioService {
    /// Starts the worker; `wake` is called whenever the snapshot changed.
    pub fn start(wake: Waker) -> Self {
        let shared = Arc::new(Shared::default());
        let (tx, rx) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let worker_tx = tx.clone();
        let thread = std::thread::Builder::new()
            .name("naby-audio".into())
            .spawn(move || worker(rx, worker_tx, worker_shared, wake))
            .ok();
        AudioService { shared, tx, thread }
    }

    /// Cheap copy of the latest state; the unavailable form when no endpoint exists.
    pub fn snapshot(&self) -> AudioSnapshot {
        lock(&self.shared.snapshot).clone()
    }

    /// Operation-specific guidance for the last failure, cleared by the next successful read.
    pub fn last_error(&self) -> Option<String> {
        lock(&self.shared.last_error).clone()
    }

    /// Master volume of the default output, 0..=1.
    pub fn set_volume(&self, value: f64) {
        let v = if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.send(Msg::SetVolume(v as f32));
    }

    pub fn toggle_mute(&self) {
        self.send(Msg::ToggleMute);
    }

    /// Endpoint mute only: never opens a recording stream.
    pub fn toggle_microphone_mute(&self) {
        self.send(Msg::ToggleMicrophoneMute);
    }

    /// Makes `id` the default for Console, Multimedia and Communications.
    pub fn set_output(&self, id: &str) {
        self.send(Msg::SetOutput(id.to_owned()));
    }

    /// Forces a re-read (for example when the audio page opens).
    pub fn refresh(&self) {
        self.send(Msg::Refresh);
    }

    fn send(&self, msg: Msg) {
        if self.tx.send(msg).is_err() {
            *lock(&self.shared.last_error) = Some(CLOSED_MESSAGE.to_owned());
        }
    }
}

impl Drop for AudioService {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    tx: Sender<Msg>,
    generation: u64,
    microphone: bool,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, pnotify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> WinResult<()> {
        // SAFETY: Core Audio passes a valid notification record for the duration of the call.
        if let Some(data) = unsafe { pnotify.as_ref() } {
            let msg = if self.microphone {
                Msg::Microphone {
                    generation: self.generation,
                    muted: data.bMuted.as_bool(),
                }
            } else {
                Msg::Volume {
                    generation: self.generation,
                    volume: data.fMasterVolume,
                    muted: data.bMuted.as_bool(),
                }
            };
            let _ = self.tx.send(msg);
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceNotifications {
    tx: Sender<Msg>,
}

impl IMMNotificationClient_Impl for DeviceNotifications_Impl {
    fn OnDeviceStateChanged(
        &self,
        _id: &windows::core::PCWSTR,
        _state: DEVICE_STATE,
    ) -> WinResult<()> {
        let _ = self.tx.send(Msg::Devices);
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &windows::core::PCWSTR) -> WinResult<()> {
        let _ = self.tx.send(Msg::Devices);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &windows::core::PCWSTR) -> WinResult<()> {
        let _ = self.tx.send(Msg::Devices);
        Ok(())
    }
    fn OnDefaultDeviceChanged(
        &self,
        _flow: EDataFlow,
        _role: ERole,
        _id: &windows::core::PCWSTR,
    ) -> WinResult<()> {
        let _ = self.tx.send(Msg::Devices);
        Ok(())
    }
    fn OnPropertyValueChanged(
        &self,
        id: &windows::core::PCWSTR,
        _key: &PROPERTYKEY,
    ) -> WinResult<()> {
        let _ = self.tx.send(Msg::Property(devices::read_pcwstr(id)));
        Ok(())
    }
}

/// A default endpoint with its volume interface and change callback.
struct Endpoint {
    id: String,
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl Endpoint {
    fn open(
        device: &IMMDevice,
        id: String,
        tx: &Sender<Msg>,
        generation: u64,
        microphone: bool,
    ) -> WinResult<Endpoint> {
        // SAFETY: activating the endpoint volume interface on a live device.
        let volume: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None)? };
        let callback: IAudioEndpointVolumeCallback = VolumeCallback {
            tx: tx.clone(),
            generation,
            microphone,
        }
        .into();
        // SAFETY: `callback` is a live COM object; it is unregistered in `close` before release.
        unsafe { volume.RegisterControlChangeNotify(&callback)? };
        Ok(Endpoint {
            id,
            volume,
            callback,
        })
    }

    fn close(self) {
        // SAFETY: unregisters the callback registered in `open`; errors mean the device is gone.
        unsafe {
            let _ = self.volume.UnregisterControlChangeNotify(&self.callback);
        }
    }

    fn scalar(&self) -> WinResult<f32> {
        // SAFETY: plain COM call.
        unsafe { self.volume.GetMasterVolumeLevelScalar() }
    }

    fn muted(&self) -> WinResult<bool> {
        // SAFETY: plain COM call.
        unsafe { self.volume.GetMute().map(|b| b.as_bool()) }
    }
}

struct Worker {
    en: Option<IMMDeviceEnumerator>,
    tx: Sender<Msg>,
    shared: Arc<Shared>,
    wake: Waker,
    render: Option<Endpoint>,
    mic: Option<Endpoint>,
    generation: u64,
    snap: AudioSnapshot,
    /// Command failures, applied after the re-read so the read does not clear them.
    deferred: Vec<AudioOperation>,
}

fn worker(rx: Receiver<Msg>, tx: Sender<Msg>, shared: Arc<Shared>, wake: Waker) {
    let _com = ComApartment::mta();
    let mut w = Worker {
        en: devices::enumerator().ok(),
        tx: tx.clone(),
        shared,
        wake,
        render: None,
        mic: None,
        generation: 0,
        snap: AudioSnapshot::default(),
        deferred: Vec::new(),
    };
    let notifications: Option<IMMNotificationClient> = w.en.as_ref().and_then(|en| {
        let client: IMMNotificationClient = DeviceNotifications { tx }.into();
        // SAFETY: the client stays alive until it is unregistered below.
        unsafe { en.RegisterEndpointNotificationCallback(&client) }
            .ok()
            .map(|()| client)
    });
    w.read_all(true);
    loop {
        let first = match rx.recv_timeout(AUDIT) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => {
                w.read_all(false);
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let mut batch = vec![first];
        batch.extend(rx.try_iter());
        if w.process(batch) {
            break;
        }
    }
    if let (Some(en), Some(client)) = (&w.en, &notifications) {
        // SAFETY: unregisters the client registered above.
        unsafe {
            let _ = en.UnregisterEndpointNotificationCallback(client);
        }
    }
    w.close_endpoints();
}

impl Worker {
    /// Handles queued messages in order, then one combined re-read. Returns true on shutdown.
    fn process(&mut self, batch: Vec<Msg>) -> bool {
        let mut reread = false;
        let mut outputs = false;
        let mut publish = false;
        for msg in batch {
            match msg {
                Msg::Shutdown => return true,
                Msg::Devices => {
                    reread = true;
                    outputs = true;
                }
                Msg::Refresh => {
                    reread = true;
                    outputs = true;
                }
                Msg::Property(id) => {
                    let ours = self.render.as_ref().is_some_and(|e| e.id == id)
                        || self.mic.as_ref().is_some_and(|e| e.id == id);
                    reread |= ours;
                }
                Msg::Volume {
                    generation,
                    volume,
                    muted,
                } => {
                    if generation == self.generation && self.render.is_some() {
                        self.snap.volume = volume.clamp(0.0, 1.0);
                        self.snap.muted = muted;
                        publish = true;
                    }
                }
                Msg::Microphone { generation, muted } => {
                    if generation == self.generation && self.mic.is_some() {
                        self.snap.mic_muted = muted;
                        publish = true;
                    }
                }
                Msg::SetVolume(v) => {
                    if self.set_volume(v) {
                        reread = true;
                    }
                }
                Msg::ToggleMute => {
                    if self.toggle(false) {
                        reread = true;
                    }
                }
                Msg::ToggleMicrophoneMute => {
                    if self.toggle(true) {
                        reread = true;
                    }
                }
                Msg::SetOutput(id) => {
                    self.set_output(&id);
                    reread = true;
                    outputs = true;
                }
            }
        }
        if reread {
            self.read_all(outputs);
        } else if publish {
            self.publish();
        }
        for op in std::mem::take(&mut self.deferred) {
            self.fail(op);
        }
        false
    }

    fn fail(&self, op: AudioOperation) {
        *lock(&self.shared.last_error) = Some(op.recovery_message().to_owned());
    }

    fn publish(&mut self) {
        let changed = {
            let mut current = lock(&self.shared.snapshot);
            if *current == self.snap {
                false
            } else {
                *current = self.snap.clone();
                true
            }
        };
        if changed {
            (self.wake)();
        }
    }

    fn close_endpoints(&mut self) {
        if let Some(e) = self.render.take() {
            e.close();
        }
        if let Some(e) = self.mic.take() {
            e.close();
        }
    }

    /// Re-acquires both default endpoints (a stale COM endpoint can never survive an unplug).
    fn read_all(&mut self, list_outputs: bool) {
        *lock(&self.shared.last_error) = None;
        self.read_render();
        if list_outputs {
            self.read_outputs();
        }
        self.read_microphone();
        self.publish();
    }

    fn read_outputs(&mut self) {
        let Some(en) = &self.en else {
            self.snap.outputs.clear();
            return;
        };
        match devices::render_outputs(en) {
            Ok(list) => self.snap.outputs = list,
            Err(_) => {
                self.snap.outputs.clear();
                self.fail(AudioOperation::ListOutputs);
            }
        }
    }

    fn read_render(&mut self) {
        let result = self.try_read_render();
        if result.is_err() {
            if let Some(e) = self.render.take() {
                e.close();
            }
            self.fail(AudioOperation::ReadOutput);
            let outputs = std::mem::take(&mut self.snap.outputs);
            let mic = (
                self.snap.mic_available,
                self.snap.mic_muted,
                std::mem::take(&mut self.snap.mic_name),
            );
            self.snap = AudioSnapshot {
                outputs,
                mic_available: mic.0,
                mic_muted: mic.1,
                mic_name: mic.2,
                ..AudioSnapshot::default()
            };
        }
    }

    fn try_read_render(&mut self) -> WinResult<()> {
        let en = self
            .en
            .as_ref()
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        let device =
            devices::default_endpoint(en, eRender, windows::Win32::Media::Audio::eMultimedia)?;
        let id = devices::device_id(&device)?;
        if self.render.as_ref().map(|e| e.id.as_str()) != Some(id.as_str()) {
            if let Some(old) = self.render.take() {
                old.close();
            }
            self.generation += 1;
            self.render = Some(Endpoint::open(
                &device,
                id.clone(),
                &self.tx,
                self.generation,
                false,
            )?);
        }
        let endpoint = self
            .render
            .as_ref()
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        self.snap.volume = endpoint.scalar()?.clamp(0.0, 1.0);
        self.snap.muted = endpoint.muted()?;
        self.snap.device = devices::friendly_name(&device);
        self.snap.default_output_id = id;
        self.snap.available = true;
        Ok(())
    }

    fn read_microphone(&mut self) {
        match self.try_read_microphone() {
            Ok(()) => {}
            Err(_) => {
                if let Some(e) = self.mic.take() {
                    e.close();
                }
                self.snap.mic_available = false;
                self.snap.mic_muted = false;
                self.snap.mic_name = AudioSnapshot::default().mic_name;
                self.fail(AudioOperation::ReadMicrophone);
            }
        }
    }

    fn try_read_microphone(&mut self) -> WinResult<()> {
        let en = self
            .en
            .as_ref()
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        let device = devices::microphone_endpoint(en)?;
        let id = devices::device_id(&device)?;
        if self.mic.as_ref().map(|e| e.id.as_str()) != Some(id.as_str()) {
            if let Some(old) = self.mic.take() {
                old.close();
            }
            self.generation += 1;
            self.mic = Some(Endpoint::open(
                &device,
                id,
                &self.tx,
                self.generation,
                true,
            )?);
        }
        let endpoint = self
            .mic
            .as_ref()
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        self.snap.mic_muted = endpoint.muted()?;
        self.snap.mic_name = devices::friendly_name(&device);
        self.snap.mic_available = true;
        Ok(())
    }

    fn set_volume(&mut self, value: f32) -> bool {
        let Some(e) = &self.render else {
            self.deferred.push(AudioOperation::SetVolume);
            return false;
        };
        // SAFETY: plain COM call; a null-equivalent (zero) event context is allowed.
        let r = unsafe { e.volume.SetMasterVolumeLevelScalar(value, &GUID::zeroed()) };
        match r {
            Ok(()) => true,
            Err(_) => {
                self.deferred.push(AudioOperation::SetVolume);
                self.snap.available = false;
                true
            }
        }
    }

    fn toggle(&mut self, microphone: bool) -> bool {
        let (endpoint, op) = if microphone {
            (&self.mic, AudioOperation::ToggleMicrophoneMute)
        } else {
            (&self.render, AudioOperation::ToggleMute)
        };
        let Some(e) = endpoint else {
            self.deferred.push(op);
            return false;
        };
        let result = e.muted().and_then(|m| {
            // SAFETY: plain COM call.
            unsafe { e.volume.SetMute(!m, &GUID::zeroed()) }
        });
        if result.is_err() {
            self.deferred.push(op);
            if !microphone {
                self.snap.available = false;
            }
        }
        true
    }

    fn set_output(&mut self, id: &str) {
        if id.trim().is_empty() {
            self.deferred.push(AudioOperation::SetOutput);
            return;
        }
        let ok = match &self.en {
            // Reject stale or capture endpoint ids before invoking the policy interface.
            Some(en) if devices::is_active_render(en, id) => {
                devices::set_default_output(id).is_ok()
            }
            _ => false,
        };
        if !ok {
            self.deferred.push(AudioOperation::SetOutput);
        }
    }
}
