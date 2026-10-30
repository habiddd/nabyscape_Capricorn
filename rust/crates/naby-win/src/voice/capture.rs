//! Microphone capture: WASAPI shared mode on the selected input (or the Windows default),
//! converted to mono floats at a target rate (16 kHz for Whistle and the detector, 24 kHz for
//! OpenAI Realtime) in 50 ms chunks. The thread exists only while capture is running; the
//! device is released as soon as [`Capture::stop`] returns.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    eCapture, IAudioCaptureClient, IAudioClient, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, WAVEFORMATEX,
};
use windows::Win32::System::Com::{CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::dsp::{downmix, Resampler};
use crate::devices::{self, ComApartment};
use crate::microphones;

/// Chunk length delivered to the callback.
pub const CHUNK_MS: u32 = 50;

pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy)]
enum Sample {
    F32,
    I16,
    I24,
    I32,
}

impl Capture {
    /// Opens `microphone_id` (empty: the Windows default) and starts delivering chunks of mono
    /// floats at `rate` Hz to `on_audio`. `on_fail` runs once if the device stops working.
    pub fn start(
        microphone_id: &str,
        rate: u32,
        mut on_audio: impl FnMut(&[f32]) + Send + 'static,
        on_fail: impl FnOnce(String) + Send + 'static,
    ) -> Result<Capture, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let id = microphone_id.to_owned();
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("NabyScape voice capture".into())
            .spawn(move || {
                let _com = ComApartment::mta();
                match Input::open(&id) {
                    Ok(mut input) => {
                        let _ = tx.send(Ok(()));
                        if let Err(e) = input.run(rate, &flag, &mut on_audio) {
                            if !flag.load(Ordering::SeqCst) {
                                on_fail(e);
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
            Ok(Ok(())) => Ok(Capture {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("The microphone did not start.".into()),
        }
    }

    /// Stops the thread and releases the device.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Input {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: HANDLE,
    rate: u32,
    channels: usize,
    sample: Sample,
}

impl Drop for Input {
    fn drop(&mut self) {
        // SAFETY: stops the started client and closes the event handle this struct created.
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

impl Input {
    fn open(id: &str) -> Result<Input, String> {
        let en = devices::enumerator().map_err(|e| e.to_string())?;
        let list = microphones::list();
        let device = match microphones::resolve(Some(id), &list) {
            Ok(Some(mic)) => devices::device_by_id(&en, &mic.id)
                .map_err(|_| microphones::UNAVAILABLE_MESSAGE.to_owned())?,
            Ok(None) => devices::microphone_endpoint(&en).map_err(|_| {
                "No microphone is available. Check Windows sound settings.".to_owned()
            })?,
            Err(message) => return Err(message.to_owned()),
        };
        let _ = eCapture;
        // SAFETY: standard WASAPI shared-mode setup; the mix format allocation is freed on every
        // path and the interfaces are owned by the returned Input.
        unsafe {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|_| {
                "The microphone could not be opened. Check Windows microphone permissions."
                    .to_owned()
            })?;
            let fmt = client.GetMixFormat().map_err(|e| e.to_string())?;
            let parsed = parse(fmt);
            let rate = (*fmt).nSamplesPerSec;
            let init = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                1_000_000,
                0,
                fmt,
                None,
            );
            CoTaskMemFree(Some(fmt as *const std::ffi::c_void));
            let (channels, sample) =
                parsed.ok_or("The microphone's audio format is not supported.")?;
            init.map_err(|_| {
                "The microphone could not be opened. Check Windows microphone permissions."
                    .to_owned()
            })?;
            let event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
            client.SetEventHandle(event).map_err(|e| e.to_string())?;
            let capture: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
            client.Start().map_err(|e| e.to_string())?;
            Ok(Input {
                client,
                capture,
                event,
                rate,
                channels,
                sample,
            })
        }
    }

    fn run(
        &mut self,
        target: u32,
        stop: &AtomicBool,
        on_audio: &mut dyn FnMut(&[f32]),
    ) -> Result<(), String> {
        let mut resampler = (self.rate != target).then(|| Resampler::new(self.rate, target));
        let chunk = (target * CHUNK_MS / 1000) as usize;
        let mut pending: Vec<f32> = Vec::with_capacity(chunk * 2);
        let mut silent_waits = 0u32;
        while !stop.load(Ordering::SeqCst) {
            // SAFETY: waits on the event this Input owns.
            let waited = unsafe { WaitForSingleObject(self.event, 250) };
            if waited.0 == 0x102 {
                // Timeout: a working capture endpoint signals every period.
                silent_waits += 1;
                if silent_waits > 40 {
                    return Err("The microphone stopped delivering audio.".into());
                }
                continue;
            }
            silent_waits = 0;
            let mono = self.drain()?;
            if mono.is_empty() {
                continue;
            }
            let converted = match &mut resampler {
                Some(r) => r.process(&mono),
                None => mono,
            };
            pending.extend_from_slice(&converted);
            while pending.len() >= chunk {
                let block: Vec<f32> = pending.drain(..chunk).collect();
                on_audio(&block);
            }
        }
        Ok(())
    }

    /// Reads every queued packet as mono floats at the device rate.
    fn drain(&mut self) -> Result<Vec<f32>, String> {
        let mut out = Vec::new();
        // SAFETY: GetBuffer/ReleaseBuffer pairs on a started capture client; the slice covers
        // exactly the frames the buffer reports and is dropped before ReleaseBuffer.
        unsafe {
            loop {
                match self.capture.GetNextPacketSize() {
                    Ok(0) => return Ok(out),
                    Ok(_) => {}
                    Err(e) => return Err(e.to_string()),
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| e.to_string())?;
                let width = match self.sample {
                    Sample::I16 => 2,
                    Sample::I24 => 3,
                    _ => 4,
                };
                let count = frames as usize;
                if flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null() {
                    out.extend(std::iter::repeat_n(0.0f32, count));
                } else {
                    let bytes = std::slice::from_raw_parts(data, count * self.channels * width);
                    let mut interleaved = Vec::with_capacity(count * self.channels);
                    for s in bytes.chunks_exact(width) {
                        interleaved.push(decode(s, self.sample));
                    }
                    out.extend(downmix(&interleaved, self.channels));
                }
                self.capture
                    .ReleaseBuffer(frames)
                    .map_err(|e| e.to_string())?;
            }
        }
    }
}

fn decode(b: &[u8], sample: Sample) -> f32 {
    match sample {
        Sample::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        Sample::I16 => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
        Sample::I24 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
        Sample::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
    }
}

/// `(channels, encoding)` of a mix format.
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
                let ext = fmt as *const u8;
                std::ptr::read_unaligned(ext.add(24) as *const u32) == 3
            }
            _ => return None,
        };
        let sample = match (float, bits) {
            (true, 32) => Sample::F32,
            (false, 16) => Sample::I16,
            (false, 24) => Sample::I24,
            (false, 32) => Sample::I32,
            _ => return None,
        };
        (channels > 0).then_some((channels, sample))
    }
}
