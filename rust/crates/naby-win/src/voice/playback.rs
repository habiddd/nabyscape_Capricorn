//! Spoken audio out (`AssistantVoiceAudio.MeteredPcm`): a bounded queue of 24 kHz mono 16-bit
//! PCM that a WASAPI shared-mode render thread plays. The queue is pure and tested on Linux; the
//! render thread starts only while audio is queued and stops again when it drains, so an idle
//! voice session has no audio timers.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use naby_core::assistant::voice::MAXIMUM_SPOKEN_PCM_BYTES;

use super::pipeline::Player;

/// Bytes per millisecond of 24 kHz mono 16-bit audio.
const BYTES_PER_MS: usize = 48;
/// Audio the device already holds is reported conservatively as not yet heard.
const DEVICE_LATENCY_MS: u64 = 100;

/// The bounded playback queue shared by the producer (voice session) and the render thread.
#[derive(Default)]
pub struct PcmQueue {
    inner: Mutex<Inner>,
    ready: Condvar,
    consumed: AtomicU64,
    device_ms: AtomicU32,
    level: AtomicU32,
}

#[derive(Default)]
struct Inner {
    bytes: VecDeque<u8>,
    generation: u64,
    closed: bool,
}

impl PcmQueue {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Queues audio. False when it would exceed two minutes (nothing is queued then).
    pub fn push(&self, pcm: &[u8]) -> bool {
        let mut inner = self.lock();
        if inner.closed
            || pcm.len() & 1 != 0
            || pcm.len() > MAXIMUM_SPOKEN_PCM_BYTES - inner.bytes.len()
        {
            return false;
        }
        inner.bytes.extend(pcm.iter().copied());
        drop(inner);
        self.ready.notify_all();
        true
    }

    /// Drops everything queued (an interruption) and the playing-position counter.
    pub fn flush(&self) {
        let mut inner = self.lock();
        inner.bytes = VecDeque::new();
        inner.generation += 1;
        self.consumed.store(0, Ordering::SeqCst);
        self.level.store(0, Ordering::Relaxed);
    }

    /// No more audio; wakes the render thread so it can exit.
    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        inner.bytes = VecDeque::new();
        drop(inner);
        self.ready.notify_all();
    }

    /// Takes up to `max` bytes (whole samples).
    pub fn take(&self, max: usize) -> Vec<u8> {
        let mut inner = self.lock();
        let n = max.min(inner.bytes.len()) & !1;
        let out: Vec<u8> = inner.bytes.drain(..n).collect();
        self.consumed.fetch_add(n as u64, Ordering::SeqCst);
        out
    }

    pub fn is_empty(&self) -> bool {
        self.lock().bytes.is_empty()
    }

    /// Blocks until audio is queued. False when the queue was closed.
    pub fn wait_for_audio(&self) -> bool {
        let mut inner = self.lock();
        loop {
            if inner.closed {
                return false;
            }
            if !inner.bytes.is_empty() {
                return true;
            }
            inner = self.ready.wait(inner).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Milliseconds queued but not yet handed to the device.
    pub fn queued_ms(&self) -> u32 {
        (self.lock().bytes.len() / BYTES_PER_MS) as u32
    }

    /// The render thread reports how much the device still holds.
    pub fn set_device_ms(&self, ms: u32) {
        self.device_ms.store(ms, Ordering::Relaxed);
    }

    /// Everything not yet heard: queued plus in the device.
    pub fn pending_ms(&self) -> u32 {
        self.queued_ms() + self.device_ms.load(Ordering::Relaxed)
    }

    /// Conservatively heard audio since the last flush (`PlayedMilliseconds`).
    pub fn played_ms(&self) -> u64 {
        (self.consumed.load(Ordering::SeqCst) / BYTES_PER_MS as u64)
            .saturating_sub(DEVICE_LATENCY_MS)
    }

    pub fn set_level(&self, peak: f32) {
        self.level.store(peak.to_bits(), Ordering::Relaxed);
    }

    /// Peak of the last block played (0..=1).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

impl Player for PcmQueue {
    fn append(&self, pcm: &[u8]) -> bool {
        self.push(pcm)
    }
    fn clear(&self) {
        self.flush();
    }
    fn pending_ms(&self) -> u32 {
        PcmQueue::pending_ms(self)
    }
}

#[cfg(windows)]
pub use render::Playback;

#[cfg(windows)]
mod render {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        eConsole, eRender, IAudioClient, IAudioRenderClient, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX,
    };
    use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    use super::PcmQueue;
    use crate::devices::{self, ComApartment};
    use crate::voice::dsp::{f32_to_pcm16, pcm16_to_f32, Resampler, PLAYBACK_RATE};

    /// The WASAPI render thread for one voice session.
    pub struct Playback {
        pub queue: Arc<PcmQueue>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Sample {
        F32,
        I16,
    }

    impl Playback {
        /// Opens the default output. `fail` is called once if the device stops working.
        pub fn start(fail: impl Fn(String) + Send + 'static) -> Result<Playback, String> {
            let queue = Arc::new(PcmQueue::new());
            let stop = Arc::new(AtomicBool::new(false));
            let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
            let (q, s) = (queue.clone(), stop.clone());
            let thread = std::thread::Builder::new()
                .name("NabyScape voice playback".into())
                .spawn(move || {
                    let _com = ComApartment::mta();
                    match open() {
                        Ok(mut out) => {
                            let _ = tx.send(Ok(()));
                            if let Err(e) = out.run(&q, &s) {
                                if !s.load(Ordering::SeqCst) {
                                    fail(e);
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            match rx.recv() {
                Ok(Ok(())) => Ok(Playback {
                    queue,
                    stop,
                    thread: Some(thread),
                }),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(e)
                }
                Err(_) => Err("The audio output did not start.".into()),
            }
        }

        pub fn stop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.queue.close();
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    impl Drop for Playback {
        fn drop(&mut self) {
            self.stop();
        }
    }

    struct Output {
        client: IAudioClient,
        render: IAudioRenderClient,
        event: HANDLE,
        rate: u32,
        channels: usize,
        sample: Sample,
        frames: u32,
    }

    impl Drop for Output {
        fn drop(&mut self) {
            // SAFETY: stops the client, then closes the event handle this struct created.
            unsafe {
                let _ = self.client.Stop();
                let _ = CloseHandle(self.event);
            }
        }
    }

    fn open() -> Result<Output, String> {
        let en = devices::enumerator().map_err(|e| e.to_string())?;
        let device = devices::default_endpoint(&en, eRender, eConsole)
            .map_err(|_| "No audio output device is available.".to_owned())?;
        // SAFETY: standard WASAPI shared-mode setup; the mix format allocation is freed on every
        // path and the interfaces are owned by the returned Output.
        unsafe {
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| e.to_string())?;
            let fmt = client.GetMixFormat().map_err(|e| e.to_string())?;
            let parsed = parse(fmt);
            let rate = (*fmt).nSamplesPerSec;
            let init = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                2_000_000,
                0,
                fmt,
                None,
            );
            CoTaskMemFree(Some(fmt as *const std::ffi::c_void));
            let (channels, sample) = parsed.ok_or("The audio output format is not supported.")?;
            init.map_err(|e| e.to_string())?;
            let event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
            client.SetEventHandle(event).map_err(|e| e.to_string())?;
            let frames = client.GetBufferSize().map_err(|e| e.to_string())?;
            let render: IAudioRenderClient = client.GetService().map_err(|e| e.to_string())?;
            Ok(Output {
                client,
                render,
                event,
                rate,
                channels,
                sample,
                frames,
            })
        }
    }

    /// `(channels, sample encoding)` of a mix format (float32 or 16-bit PCM).
    ///
    /// # Safety
    /// `fmt` must point to a valid `WAVEFORMATEX` (a full extensible one when tagged so).
    unsafe fn parse(fmt: *const WAVEFORMATEX) -> Option<(usize, Sample)> {
        // SAFETY: the caller guarantees validity; the struct is packed so fields are read by value.
        unsafe {
            let tag = std::ptr::addr_of!((*fmt).wFormatTag).read_unaligned();
            let channels = std::ptr::addr_of!((*fmt).nChannels).read_unaligned() as usize;
            let bits = std::ptr::addr_of!((*fmt).wBitsPerSample).read_unaligned();
            let float = match tag {
                3 => true,
                1 => false,
                0xFFFE => {
                    // The sub-format GUID's first field is the format tag (3 float, 1 PCM).
                    let ext = fmt as *const u8;
                    let first = std::ptr::read_unaligned(ext.add(24) as *const u32);
                    first == 3
                }
                _ => return None,
            };
            match (float, bits) {
                (true, 32) => Some((channels, Sample::F32)),
                (false, 16) => Some((channels, Sample::I16)),
                _ => None,
            }
        }
    }

    impl Output {
        fn run(&mut self, queue: &PcmQueue, stop: &AtomicBool) -> Result<(), String> {
            let mut resampler = Resampler::new(PLAYBACK_RATE, self.rate);
            let mut carry: std::collections::VecDeque<f32> = Default::default();
            loop {
                if !queue.wait_for_audio() || stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                // SAFETY: the client was initialized in `open`; Start/Stop bracket the loop.
                unsafe { self.client.Start() }.map_err(|e| e.to_string())?;
                let result = self.play(queue, stop, &mut resampler, &mut carry);
                // SAFETY: stops and resets the started client before waiting for more audio.
                unsafe {
                    let _ = self.client.Stop();
                    let _ = self.client.Reset();
                }
                queue.set_device_ms(0);
                queue.set_level(0.0);
                result?;
                if stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
            }
        }

        fn play(
            &self,
            queue: &PcmQueue,
            stop: &AtomicBool,
            resampler: &mut Resampler,
            carry: &mut std::collections::VecDeque<f32>,
        ) -> Result<(), String> {
            let mut idle_blocks = 0;
            loop {
                if stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                // SAFETY: waits on the event this Output owns.
                let waited = unsafe { WaitForSingleObject(self.event, 200) };
                if waited != WAIT_OBJECT_0 && waited.0 != 0x102 {
                    return Err("The audio output stopped.".into());
                }
                // SAFETY: padding and buffer calls on the started client; the written slice
                // covers exactly the frames GetBuffer granted and is released right after.
                unsafe {
                    let padding = self.client.GetCurrentPadding().map_err(|e| e.to_string())?;
                    queue.set_device_ms((padding as u64 * 1000 / self.rate as u64) as u32);
                    let room = self.frames.saturating_sub(padding) as usize;
                    // Refill the device-rate carry from the queue.
                    let want = room.saturating_sub(carry.len());
                    if want > 0 {
                        let input = (want as u64 * u64::from(PLAYBACK_RATE) / u64::from(self.rate))
                            as usize
                            + 8;
                        let bytes = queue.take(input * 2);
                        if !bytes.is_empty() {
                            carry.extend(resampler.process(&pcm16_to_f32(&bytes)));
                        }
                    }
                    let n = room.min(carry.len());
                    if n > 0 {
                        let data = self.render.GetBuffer(n as u32).map_err(|e| e.to_string())?;
                        let block: Vec<f32> = carry.drain(..n).collect();
                        let peak = block.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                        match self.sample {
                            Sample::F32 => {
                                let out = std::slice::from_raw_parts_mut(
                                    data as *mut f32,
                                    n * self.channels,
                                );
                                for (i, v) in block.iter().enumerate() {
                                    for c in 0..self.channels {
                                        out[i * self.channels + c] = *v;
                                    }
                                }
                            }
                            Sample::I16 => {
                                let pcm = f32_to_pcm16(&block);
                                let out = std::slice::from_raw_parts_mut(
                                    data as *mut i16,
                                    n * self.channels,
                                );
                                for i in 0..n {
                                    let v = i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]);
                                    for c in 0..self.channels {
                                        out[i * self.channels + c] = v;
                                    }
                                }
                            }
                        }
                        self.render
                            .ReleaseBuffer(n as u32, 0)
                            .map_err(|e| e.to_string())?;
                        queue.set_level(peak);
                        idle_blocks = 0;
                    } else if queue.is_empty() && padding == 0 && carry.is_empty() {
                        idle_blocks += 1;
                        if idle_blocks >= 2 {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_is_bounded_to_two_minutes_and_whole_samples() {
        let q = PcmQueue::new();
        assert!(q.push(&vec![0u8; MAXIMUM_SPOKEN_PCM_BYTES - 10]));
        assert!(!q.push(&[0u8; 12]), "over the limit is refused whole");
        assert!(q.push(&[0u8; 10]));
        assert!(!q.push(&[0u8; 3]), "odd byte counts are not samples");
        assert_eq!(q.queued_ms(), (MAXIMUM_SPOKEN_PCM_BYTES / 48) as u32);
    }

    #[test]
    fn reading_consumes_in_order_and_reports_played_time_conservatively() {
        let q = PcmQueue::new();
        q.push(&(0..=255u8).cycle().take(96_000).collect::<Vec<_>>());
        let first = q.take(4);
        assert_eq!(first, vec![0, 1, 2, 3]);
        assert_eq!(q.take(3).len(), 2, "never splits a sample");
        // 2000 ms queued; consuming 1000 ms reports 100 ms less than consumed (device latency).
        q.take(48_000 - 6);
        assert_eq!(q.played_ms(), 1000 - 100);
        assert!(q.pending_ms() >= 990);
        q.set_device_ms(50);
        assert!(q.pending_ms() >= 1040);
    }

    #[test]
    fn flush_drops_audio_and_resets_the_clock() {
        let q = PcmQueue::new();
        q.push(&[1u8; 9600]);
        q.take(4800);
        q.flush();
        assert!(q.is_empty());
        assert_eq!(q.played_ms(), 0);
        assert_eq!(q.queued_ms(), 0);
        assert!(Player::append(&q, &[0u8; 2]));
    }

    #[test]
    fn the_render_thread_waits_without_polling_and_wakes_on_audio_or_close() {
        let q = std::sync::Arc::new(PcmQueue::new());
        let q2 = q.clone();
        let waiter = std::thread::spawn(move || q2.wait_for_audio());
        std::thread::sleep(std::time::Duration::from_millis(30));
        q.push(&[0u8; 4]);
        assert!(waiter.join().unwrap());
        q.take(4);
        let q3 = q.clone();
        let closer = std::thread::spawn(move || q3.wait_for_audio());
        std::thread::sleep(std::time::Duration::from_millis(30));
        q.close();
        assert!(!closer.join().unwrap());
        assert!(!q.push(&[0u8; 2]), "closed queues refuse audio");
    }
}
