//! Output level capture for the waveform and ambient glow (`OutputSpectrum.cs`,
//! `MediaAudioMeter.cs`).
//!
//! Nothing runs until a consumer takes a lease with [`LoopbackService::acquire`]. While at
//! least one lease exists a single thread
//! - samples the peak of the media source's audio sessions (`IAudioMeterInformation`) at
//!   about 30 Hz through [`naby_core::media::PeakCache`], and
//! - when a [`Need::Spectrum`] lease exists, captures the default output in WASAPI shared
//!   loopback mode and feeds [`naby_core::media::Spectrum`].
//!
//! The last guard dropping stops the thread and releases every COM object. The waker is
//! called only when audible/silent or spectrum ready/not-ready flips; consumers read
//! [`LoopbackService::levels`] on their own animation frames.
#![allow(dead_code)] // consumed by the app shell, which is wired separately

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use naby_core::media::{PeakCache, Spectrum, BANDS};
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, IAudioCaptureClient, IAudioClient, IAudioSessionControl,
    IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator,
    ISimpleAudioVolume, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, INFINITE,
};

use crate::audio_logic::media_process_matches;
use crate::devices::{self, ComApartment};
use crate::wake::Waker;

/// Peak sampling cadence (the C# `MediaSampleGate` interval).
const PEAK_INTERVAL: Duration = Duration::from_millis(33);
/// Bands older than this count as "no signal" (C# `Merge` freshness).
const BAND_FRESHNESS: Duration = Duration::from_millis(200);
/// How often a running capture checks whether the default output moved.
const DEFAULT_CHECK: Duration = Duration::from_secs(2);
/// Wait before retrying a failed capture start.
const RETRY: Duration = Duration::from_secs(2);
/// Peak above this counts as audible for waker purposes.
const AUDIBLE: f32 = 0.002;

/// What a consumer needs while its lease is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Media session peak only (ambient glow): no capture stream, no FFT.
    Peak,
    /// Peak plus the 25 spectrum bands (closed-dock waveform).
    Spectrum,
}

/// Latest measurements; all zero when nothing is measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    pub bands: [f32; BANDS],
    /// Increases whenever `bands` was recomputed.
    pub band_sequence: u64,
    /// False when no fresh loopback data is available.
    pub spectrum_ready: bool,
    /// 0..=1 peak of the media source's audible sessions.
    pub peak: f32,
    pub peak_sequence: u64,
    /// The source has sessions but all are muted or at zero volume.
    pub muted: bool,
    pub sessions: u32,
    pub audible_sessions: u32,
}

impl Default for Levels {
    fn default() -> Self {
        Levels {
            bands: [0.0; BANDS],
            band_sequence: 0,
            spectrum_ready: false,
            peak: 0.0,
            peak_sequence: 0,
            muted: false,
            sessions: 0,
            audible_sessions: 0,
        }
    }
}

struct Event(HANDLE);

// SAFETY: a Win32 event handle may be signalled and waited on from any thread.
unsafe impl Send for Event {}
// SAFETY: see above; the handle value is immutable.
unsafe impl Sync for Event {}

impl Event {
    fn new() -> Option<Event> {
        // SAFETY: creates an unnamed manual-reset event; closed in Drop.
        unsafe { CreateEventW(None, true, false, None) }
            .ok()
            .map(Event)
    }
    fn set(&self) {
        // SAFETY: valid event handle owned by `self`.
        unsafe {
            let _ = SetEvent(self.0);
        }
    }
    fn reset(&self) {
        // SAFETY: valid event handle owned by `self`.
        unsafe {
            let _ = ResetEvent(self.0);
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: closes the handle created in `new`, exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct Running {
    id: u64,
    event: Arc<Event>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
struct State {
    peak_leases: usize,
    spectrum_leases: usize,
    source: Option<String>,
    run_id: u64,
    running: Option<Running>,
}

struct Inner {
    state: Mutex<State>,
    levels: Mutex<Levels>,
    wake: Waker,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Lease-counted output metering.
pub struct LoopbackService {
    inner: Arc<Inner>,
}

/// Keeps capture alive; dropping the last one stops the thread.
pub struct LoopbackLease {
    inner: Arc<Inner>,
    need: Need,
}

impl LoopbackService {
    pub fn start(wake: Waker) -> Self {
        LoopbackService {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
                levels: Mutex::new(Levels::default()),
                wake,
            }),
        }
    }

    /// Registers a consumer; the capture thread starts with the first lease.
    pub fn acquire(&self, need: Need) -> LoopbackLease {
        let mut st = lock(&self.inner.state);
        match need {
            Need::Peak => st.peak_leases += 1,
            Need::Spectrum => st.spectrum_leases += 1,
        }
        match &st.running {
            Some(r) => r.event.set(),
            None => self.spawn(&mut st),
        }
        LoopbackLease {
            inner: Arc::clone(&self.inner),
            need,
        }
    }

    fn spawn(&self, st: &mut State) {
        let Some(event) = Event::new().map(Arc::new) else {
            return;
        };
        st.run_id += 1;
        let id = st.run_id;
        let inner = Arc::clone(&self.inner);
        let thread_event = Arc::clone(&event);
        if let Ok(thread) = std::thread::Builder::new()
            .name("naby-loopback".into())
            .spawn(move || run(inner, id, thread_event))
        {
            st.running = Some(Running { id, event, thread });
        }
    }

    /// Media source whose sessions are metered (SMTC app id); `None` meters nothing.
    pub fn set_source(&self, source: Option<&str>) {
        let mut st = lock(&self.inner.state);
        let next = source.filter(|s| !s.trim().is_empty()).map(str::to_owned);
        if st.source != next {
            st.source = next;
            if let Some(r) = &st.running {
                r.event.set();
            }
        }
    }

    pub fn levels(&self) -> Levels {
        *lock(&self.inner.levels)
    }

    pub fn peak(&self) -> f32 {
        lock(&self.inner.levels).peak
    }

    /// Latest bands, `None` while no fresh loopback data exists.
    pub fn bands(&self) -> Option<[f32; BANDS]> {
        let l = lock(&self.inner.levels);
        l.spectrum_ready.then_some(l.bands)
    }

    /// Whether the capture thread currently exists (for tests and diagnostics).
    pub fn active(&self) -> bool {
        lock(&self.inner.state).running.is_some()
    }
}

impl Drop for LoopbackLease {
    fn drop(&mut self) {
        let stopped = {
            let mut st = lock(&self.inner.state);
            match self.need {
                Need::Peak => st.peak_leases = st.peak_leases.saturating_sub(1),
                Need::Spectrum => st.spectrum_leases = st.spectrum_leases.saturating_sub(1),
            }
            if st.peak_leases + st.spectrum_leases == 0 {
                st.run_id += 1;
                st.running.take()
            } else {
                if let Some(r) = &st.running {
                    r.event.set();
                }
                None
            }
        };
        if let Some(r) = stopped {
            r.event.set();
            let _ = r.thread.join();
            *lock(&self.inner.levels) = Levels::default();
        }
    }
}

impl Drop for LoopbackService {
    fn drop(&mut self) {
        // Leases hold their own Arc and stop the thread; nothing else to release.
        let mut st = lock(&self.inner.state);
        st.run_id += 1;
        if let Some(r) = &st.running {
            r.event.set();
        }
    }
}

struct Config {
    spectrum: bool,
    source: Option<String>,
}

impl Inner {
    /// Current wishes, or `None` when this run was superseded or no lease remains.
    fn config(&self, id: u64) -> Option<Config> {
        let st = lock(&self.state);
        if st.run_id != id || st.peak_leases + st.spectrum_leases == 0 {
            return None;
        }
        Some(Config {
            spectrum: st.spectrum_leases > 0,
            source: st.source.clone(),
        })
    }
}

// ---- peak metering ----

struct SessionMeter {
    _control: IAudioSessionControl,
    volume: ISimpleAudioVolume,
    meter: IAudioMeterInformation,
}

struct MediaEndpoint {
    _device: IMMDevice,
    volume: IAudioEndpointVolume,
    sessions: Vec<SessionMeter>,
}

#[derive(Default)]
struct Counters {
    sessions: AtomicU32,
    audible: AtomicU32,
}

fn discover(en: &IMMDeviceEnumerator, source: &str) -> Result<Vec<MediaEndpoint>, String> {
    let mut found = Vec::new();
    for device in devices::active_devices(en, eRender).map_err(|e| e.to_string())? {
        let matches = (|| -> windows::core::Result<Vec<SessionMeter>> {
            // SAFETY: COM activation and enumeration on live interfaces.
            unsafe {
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                let list = manager.GetSessionEnumerator()?;
                let mut out = Vec::new();
                for i in 0..list.GetCount()? {
                    let Ok(control) = list.GetSession(i) else {
                        continue;
                    };
                    let Ok(control2) = control.cast::<IAudioSessionControl2>() else {
                        continue;
                    };
                    // Expired or system sessions have no readable process; skip them.
                    let Ok(pid) = control2.GetProcessId() else {
                        continue;
                    };
                    let Some(name) = devices::process_name(pid) else {
                        continue;
                    };
                    if !media_process_matches(source, &name) {
                        continue;
                    }
                    let (Ok(volume), Ok(meter)) = (
                        control.cast::<ISimpleAudioVolume>(),
                        control.cast::<IAudioMeterInformation>(),
                    ) else {
                        continue;
                    };
                    out.push(SessionMeter {
                        _control: control,
                        volume,
                        meter,
                    });
                }
                Ok(out)
            }
        })()
        .unwrap_or_default();
        if matches.is_empty() {
            continue;
        }
        // SAFETY: activation on a live endpoint.
        if let Ok(volume) = unsafe { device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) } {
            found.push(MediaEndpoint {
                _device: device,
                volume,
                sessions: matches,
            });
        }
    }
    Ok(found)
}

fn read_endpoint(endpoint: &mut MediaEndpoint, counters: &Counters) -> Result<f32, String> {
    // SAFETY: plain COM reads on live interfaces owned by `endpoint`.
    unsafe {
        let output_muted = endpoint
            .volume
            .GetMute()
            .map_err(|e| e.to_string())?
            .as_bool();
        let mut peak = 0.0f32;
        for s in &endpoint.sessions {
            counters.sessions.fetch_add(1, Ordering::Relaxed);
            let muted = s.volume.GetMute().map_err(|e| e.to_string())?.as_bool();
            let level = s.volume.GetMasterVolume().map_err(|e| e.to_string())?;
            if output_muted || muted || level <= 0.0 {
                continue;
            }
            counters.audible.fetch_add(1, Ordering::Relaxed);
            peak = peak.max(s.meter.GetPeakValue().map_err(|e| e.to_string())?);
        }
        Ok(peak)
    }
}

/// Lets the enumerator move into the `PeakCache` closure. The meter is created, used and
/// dropped on the capture thread only (a multithreaded-apartment, free-threaded object).
struct ThreadBound<T>(T);

impl<T> ThreadBound<T> {
    fn get(&self) -> &T {
        &self.0
    }
}

// SAFETY: see the type docs; the value never actually crosses threads.
unsafe impl<T> Send for ThreadBound<T> {}

struct PeakMeter {
    source: String,
    cache: PeakCache<MediaEndpoint>,
    counters: Arc<Counters>,
}

impl PeakMeter {
    fn new(en: IMMDeviceEnumerator, source: String) -> PeakMeter {
        let en = ThreadBound(en);
        let counters = Arc::new(Counters::default());
        let sample_counters = Arc::clone(&counters);
        let wanted = source.clone();
        let cache = PeakCache::new(
            move || discover(en.get(), &wanted),
            move |e: &mut MediaEndpoint| read_endpoint(e, &sample_counters),
            |_endpoint| Ok(()),
            |_error| {},
        );
        PeakMeter {
            source,
            cache,
            counters,
        }
    }

    /// `(peak, muted, sessions, audible)`.
    fn read(&mut self) -> (f32, bool, u32, u32) {
        self.counters.sessions.store(0, Ordering::Relaxed);
        self.counters.audible.store(0, Ordering::Relaxed);
        let peak = self.cache.read(Instant::now());
        let sessions = self.counters.sessions.load(Ordering::Relaxed);
        let audible = self.counters.audible.load(Ordering::Relaxed);
        (peak, sessions > 0 && audible == 0, sessions, audible)
    }
}

// ---- loopback capture ----

#[derive(Clone, Copy)]
enum Sample {
    F32,
    I16,
    I24,
    I32,
}

struct Stream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Event,
    spectrum: Spectrum,
    channels: usize,
    bytes: usize,
    sample: Sample,
    device_id: String,
    last_data: Instant,
    last_check: Instant,
    last_sequence: u64,
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SUBTYPE_FLOAT: windows::core::GUID =
    windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

impl Stream {
    fn open(en: &IMMDeviceEnumerator) -> Result<Stream, String> {
        let device =
            devices::default_endpoint(en, eRender, eMultimedia).map_err(|e| e.to_string())?;
        let device_id = devices::device_id(&device).map_err(|e| e.to_string())?;
        let event = Event::new().ok_or("event")?;
        // SAFETY: standard WASAPI loopback setup; the mix format allocation is freed on every
        // path, and the client/capture interfaces are owned by the returned Stream.
        unsafe {
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| e.to_string())?;
            let fmt = client.GetMixFormat().map_err(|e| e.to_string())?;
            let parsed = parse_format(fmt);
            let init = parsed.map(|_| {
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    2_000_000,
                    0,
                    fmt,
                    None,
                )
            });
            let rate = (*fmt).nSamplesPerSec;
            CoTaskMemFree(Some(fmt as *const c_void));
            let (channels, bytes, sample) = parsed.ok_or("Unsupported output sample format.")?;
            init.ok_or("format")?.map_err(|e| e.to_string())?;
            client.SetEventHandle(event.0).map_err(|e| e.to_string())?;
            let capture: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
            client.Start().map_err(|e| e.to_string())?;
            let now = Instant::now();
            Ok(Stream {
                client,
                capture,
                event,
                spectrum: Spectrum::new(rate),
                channels,
                bytes,
                sample,
                device_id,
                last_data: now,
                last_check: now,
                last_sequence: 0,
            })
        }
    }

    /// Reads all queued packets into the spectrum. Returns false when the device is gone.
    fn drain(&mut self) -> bool {
        // SAFETY: GetBuffer/ReleaseBuffer pairs on a started capture client; the slice covers
        // exactly the frames the buffer reports and is dropped before ReleaseBuffer.
        unsafe {
            loop {
                match self.capture.GetNextPacketSize() {
                    Ok(0) => return true,
                    Ok(_) => {}
                    Err(_) => return false,
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if self
                    .capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    return false;
                }
                let frame_bytes = self.channels * self.bytes;
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    for _ in 0..frames {
                        self.spectrum.add(0.0);
                    }
                } else {
                    let buf = std::slice::from_raw_parts(data, frames as usize * frame_bytes);
                    for frame in buf.chunks_exact(frame_bytes) {
                        let mut mono = 0.0f32;
                        for ch in frame.chunks_exact(self.bytes) {
                            mono += decode(ch, self.sample);
                        }
                        self.spectrum.add(mono / self.channels as f32);
                    }
                }
                self.last_data = Instant::now();
                if self.capture.ReleaseBuffer(frames).is_err() {
                    return false;
                }
            }
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: stops the started client before its interfaces are released.
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

fn decode(bytes: &[u8], sample: Sample) -> f32 {
    match sample {
        Sample::F32 => f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        Sample::I16 => f32::from(i16::from_le_bytes([bytes[0], bytes[1]])) / 32768.0,
        Sample::I24 => {
            (i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]) >> 8) as f32 / 8_388_608.0
        }
        Sample::I32 => {
            i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32 / 2_147_483_648.0
        }
    }
}

/// `(channels, bytes per sample, encoding)` of a mix format, if supported.
///
/// # Safety
/// `fmt` must point to a valid `WAVEFORMATEX` (and a full `WAVEFORMATEXTENSIBLE` when tagged so).
unsafe fn parse_format(fmt: *const WAVEFORMATEX) -> Option<(usize, usize, Sample)> {
    // SAFETY: the caller guarantees validity; the struct is packed so fields are read by value.
    unsafe {
        let tag = std::ptr::addr_of!((*fmt).wFormatTag).read_unaligned();
        let channels = std::ptr::addr_of!((*fmt).nChannels).read_unaligned() as usize;
        let bits = std::ptr::addr_of!((*fmt).wBitsPerSample).read_unaligned();
        let float = match tag {
            WAVE_FORMAT_FLOAT => true,
            WAVE_FORMAT_PCM => false,
            WAVE_FORMAT_EXTENSIBLE => {
                let ext = fmt as *const WAVEFORMATEXTENSIBLE;
                std::ptr::addr_of!((*ext).SubFormat).read_unaligned() == SUBTYPE_FLOAT
            }
            _ => return None,
        };
        if channels == 0 {
            return None;
        }
        let sample = match (float, bits) {
            (true, 32) => Sample::F32,
            (false, 16) => Sample::I16,
            (false, 24) => Sample::I24,
            (false, 32) => Sample::I32,
            _ => return None,
        };
        Some((channels, usize::from(bits / 8), sample))
    }
}

// ---- worker ----

fn run(inner: Arc<Inner>, id: u64, control: Arc<Event>) {
    let _com = ComApartment::mta();
    let en = devices::enumerator().ok();
    let mut meter: Option<PeakMeter> = None;
    let mut stream: Option<Stream> = None;
    let mut retry_at = Instant::now();
    let mut next_peak = Instant::now();
    let mut was_audible = false;
    let mut was_ready = false;
    while let Some(cfg) = inner.config(id) {
        control.reset();
        let now = Instant::now();

        // Peak meter follows the media source.
        match (&cfg.source, &en) {
            (Some(src), Some(en)) => {
                if meter.as_ref().map(|m| m.source.as_str()) != Some(src.as_str()) {
                    meter = Some(PeakMeter::new(en.clone(), src.clone()));
                    next_peak = now;
                }
            }
            _ => meter = None,
        }

        // Loopback stream follows the spectrum demand and the default output.
        if cfg.spectrum {
            if let Some(en) = &en {
                if let Some(s) = &mut stream {
                    if now.duration_since(s.last_check) >= DEFAULT_CHECK {
                        s.last_check = now;
                        let current = devices::default_endpoint(en, eRender, eMultimedia)
                            .and_then(|d| devices::device_id(&d))
                            .ok();
                        if current.as_deref() != Some(s.device_id.as_str()) {
                            stream = None;
                            retry_at = now;
                        }
                    }
                }
                if stream.is_none() && now >= retry_at {
                    match Stream::open(en) {
                        Ok(s) => stream = Some(s),
                        Err(_) => retry_at = now + RETRY,
                    }
                }
            }
        } else if stream.take().is_some() {
            set_bands(&inner, None, &mut was_ready);
        }

        // Peak sample when due.
        if let Some(m) = &mut meter {
            if now >= next_peak {
                let (peak, muted, sessions, audible) = m.read();
                next_peak = now + PEAK_INTERVAL;
                let mut l = lock(&inner.levels);
                l.peak = peak;
                l.peak_sequence += 1;
                l.muted = muted;
                l.sessions = sessions;
                l.audible_sessions = audible;
                drop(l);
                let is_audible = peak > AUDIBLE;
                if is_audible != was_audible {
                    was_audible = is_audible;
                    (inner.wake)();
                }
            }
        } else {
            let mut l = lock(&inner.levels);
            if l.peak != 0.0 || l.sessions != 0 {
                l.peak = 0.0;
                l.muted = false;
                l.sessions = 0;
                l.audible_sessions = 0;
                drop(l);
                if was_audible {
                    was_audible = false;
                    (inner.wake)();
                }
            }
        }

        // Wait for a packet, a lease/source change, or the next due task.
        let mut deadline: Option<Instant> = None;
        let mut want = |t: Instant| deadline = Some(deadline.map_or(t, |d| d.min(t)));
        if meter.is_some() {
            want(next_peak);
        }
        if cfg.spectrum && stream.is_none() && en.is_some() {
            want(retry_at);
        }
        if let Some(s) = &stream {
            if was_ready {
                want(s.last_data + BAND_FRESHNESS);
            }
            want(s.last_check + DEFAULT_CHECK);
        }
        let timeout = deadline.map_or(INFINITE, |d| {
            d.saturating_duration_since(Instant::now())
                .as_millis()
                .min(u128::from(u32::MAX - 1)) as u32
        });
        let mut handles = vec![control.0];
        if let Some(s) = &stream {
            handles.push(s.event.0);
        }
        // SAFETY: both handles are live events owned by this thread's state for the call.
        unsafe { WaitForMultipleObjects(&handles, false, timeout) };

        if let Some(s) = &mut stream {
            // Draining on every wake-up is cheap and also catches packets whose event was missed.
            if !s.drain() {
                stream = None;
                retry_at = Instant::now() + Duration::from_secs(1);
                set_bands(&inner, None, &mut was_ready);
                continue;
            }
            if let Some(s) = &mut stream {
                if s.spectrum.sequence() != s.last_sequence
                    && Instant::now().duration_since(s.last_data) < BAND_FRESHNESS
                {
                    s.last_sequence = s.spectrum.sequence();
                    set_bands(
                        &inner,
                        Some((*s.spectrum.bands(), s.last_sequence)),
                        &mut was_ready,
                    );
                } else if was_ready && Instant::now().duration_since(s.last_data) >= BAND_FRESHNESS
                {
                    set_bands(&inner, None, &mut was_ready);
                }
            }
        }
    }
    drop(stream);
    drop(meter);
    *lock(&inner.levels) = Levels::default();
}

/// Publishes bands (`Some`) or marks the spectrum stale (`None`); wakes on readiness flips.
fn set_bands(inner: &Inner, bands: Option<([f32; BANDS], u64)>, was_ready: &mut bool) {
    let ready = bands.is_some();
    {
        let mut l = lock(&inner.levels);
        match bands {
            Some((b, seq)) => {
                l.bands = b;
                l.band_sequence = seq;
            }
            None => l.bands = [0.0; BANDS],
        }
        l.spectrum_ready = ready;
    }
    if ready != *was_ready {
        *was_ready = ready;
        (inner.wake)();
    }
}
