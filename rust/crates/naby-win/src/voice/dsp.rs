//! Audio sample helpers that need no device: sample format conversion, a streaming
//! windowed-sinc resampler, level meters and WAV reading/writing. All pure, tested on Linux.

use std::f64::consts::PI;

/// Samples per kernel table step.
const TABLE_STEPS: usize = 128;
/// Kernel half width in zero crossings of the (band-limited) sinc.
const ZERO_CROSSINGS: f64 = 12.0;

/// Streaming resampler between any two rates (mono f32).
///
/// A Blackman-windowed sinc low-pass is applied at the lower of the two Nyquist frequencies, so
/// 48 kHz to 16 kHz capture does not alias. State carries across [`Resampler::process`] calls.
pub struct Resampler {
    step: f64,
    cutoff: f64,
    half: usize,
    table: Vec<f32>,
    /// Input history; `pos` indexes into it.
    buf: Vec<f32>,
    pos: f64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let from = from.max(1) as f64;
        let to = to.max(1) as f64;
        let cutoff = (to / from).min(1.0) * 0.95;
        let half = (ZERO_CROSSINGS / cutoff).ceil() as usize;
        let mut table = Vec::with_capacity(half * TABLE_STEPS + 2);
        for i in 0..=(half * TABLE_STEPS + 1) {
            let d = i as f64 / TABLE_STEPS as f64;
            table.push(kernel(d, cutoff, half as f64) as f32);
        }
        Self {
            step: from / to,
            cutoff,
            half,
            table,
            buf: vec![0.0; half],
            pos: half as f64,
        }
    }

    fn weight(&self, d: f64) -> f32 {
        let d = d.abs();
        let x = d * TABLE_STEPS as f64;
        let i = x as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let f = (x - i as f64) as f32;
        self.table[i] * (1.0 - f) + self.table[i + 1] * f
    }

    /// Converts a block; the output length follows the ratio over the whole stream.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.buf.extend_from_slice(input);
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 2);
        while self.pos + (self.half as f64) < self.buf.len() as f64 {
            let center = self.pos.floor() as isize;
            let frac = self.pos - center as f64;
            let mut sum = 0.0f32;
            let lo = center - self.half as isize + 1;
            let hi = center + self.half as isize;
            for k in lo..=hi {
                let d = k as f64 - self.pos;
                let _ = frac;
                sum += self.buf[k as usize] * self.weight(d);
            }
            out.push(sum);
            self.pos += self.step;
        }
        // Drop consumed input, keeping the history the next outputs need.
        let keep_from = (self.pos.floor() as usize).saturating_sub(self.half);
        if keep_from > 0 {
            self.buf.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
        let _ = self.cutoff;
        out
    }

    /// Forgets the stream (new session).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.buf.resize(self.half, 0.0);
        self.pos = self.half as f64;
    }
}

fn kernel(d: f64, cutoff: f64, half: f64) -> f64 {
    let x = d * cutoff;
    let sinc = if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    };
    let t = d / half;
    let window = if t.abs() >= 1.0 {
        0.0
    } else {
        0.42 + 0.5 * (PI * t).cos() + 0.08 * (2.0 * PI * t).cos()
    };
    cutoff * sinc * window
}

/// One-shot conversion of a whole buffer to 16 kHz (the rate Whistle and the VAD use).
pub fn resample_to_16k(input: &[f32], from: u32) -> Vec<f32> {
    if from == 16_000 {
        return input.to_vec();
    }
    let mut r = Resampler::new(from, 16_000);
    let mut out = r.process(input);
    // Flush the filter tail with silence so short clips keep their last samples.
    out.extend(r.process(&vec![0.0; r.half + 1]));
    out
}

/// Interleaved frames to mono by averaging channels.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// 16-bit little-endian PCM to floats in -1..1.
pub fn pcm16_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0)
        .collect()
}

/// Floats to 16-bit little-endian PCM, clamped; non-finite samples become silence.
pub fn f32_to_pcm16(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        let v = if s.is_finite() { *s } else { 0.0 };
        let i = (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
        out.extend_from_slice(&i.to_le_bytes());
    }
    out
}

/// Peak amplitude of float samples (0..=1).
pub fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .map(|s| if s.is_finite() { s.abs() } else { 0.0 })
        .fold(0.0, f32::max)
        .min(1.0)
}

/// Minimal RIFF/WAVE writer for 16-bit mono PCM.
pub fn wav_encode(rate: u32, pcm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// Decoded WAV audio, mono floats.
#[derive(Debug, Clone, PartialEq)]
pub struct Wav {
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// Reads PCM16 or float32 WAV with 1 or 2 channels (stereo is averaged), like the C# `ConvertWav`.
pub fn wav_decode(bytes: &[u8]) -> Result<Wav, String> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("The speech service did not return WAV audio.".into());
    }
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0u16, 0u32, 0u16);
    let mut pos = 12;
    let mut data: Option<&[u8]> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(bytes.len());
        if id == b"fmt " && end - body >= 16 {
            let f = &bytes[body..end];
            format = u16::from_le_bytes([f[0], f[1]]);
            channels = u16::from_le_bytes([f[2], f[3]]);
            rate = u32::from_le_bytes([f[4], f[5], f[6], f[7]]);
            bits = u16::from_le_bytes([f[14], f[15]]);
        } else if id == b"data" {
            data = Some(&bytes[body..end]);
            break;
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    let data = data.ok_or("The WAV audio has no samples.")?;
    if !(1..=2).contains(&channels) {
        return Err("TTS must return mono or stereo WAV audio.".into());
    }
    if !(8_000..=96_000).contains(&rate) {
        return Err("The audio has an unsupported sample rate.".into());
    }
    let interleaved: Vec<f32> = match (format, bits) {
        (1 | 0xFFFE, 16) => pcm16_to_f32(data),
        (3, 32) => data
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect(),
        _ => return Err("Unsupported WAV sample format.".into()),
    };
    Ok(Wav {
        rate,
        samples: downmix(&interleaved, channels as usize),
    })
}

/// Output audio the speaking pipeline plays: 24 kHz mono 16-bit.
pub const PLAYBACK_RATE: u32 = 24_000;

/// Converts any decoded audio to 24 kHz PCM16 mono bytes, capped at two minutes.
pub fn to_playback_pcm(wav: &Wav) -> Result<Vec<u8>, String> {
    let samples = if wav.rate == PLAYBACK_RATE {
        wav.samples.clone()
    } else {
        let mut r = Resampler::new(wav.rate, PLAYBACK_RATE);
        let mut out = r.process(&wav.samples);
        out.extend(r.process(&vec![0.0; r.half + 1]));
        out
    };
    let bytes = f32_to_pcm16(&samples);
    if bytes.len() > naby_core::assistant::voice::MAXIMUM_SPOKEN_PCM_BYTES {
        return Err("TTS reply exceeds two minutes.".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, seconds: f64) -> Vec<f32> {
        (0..(rate as f64 * seconds) as usize)
            .map(|i| (2.0 * PI * freq * i as f64 / rate as f64).sin() as f32 * 0.5)
            .collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
    }

    #[test]
    fn downsampling_keeps_a_tone_accurate() {
        let out = resample_to_16k(&sine(1000.0, 48_000, 1.0), 48_000);
        assert!((out.len() as i64 - 16_000).abs() <= 40, "{}", out.len());
        // Compare the steady part against the ideal 16 kHz sine (allowing the fixed filter delay
        // to be found by phase search).
        let ideal = sine(1000.0, 16_000, 1.0);
        let mid = 2_000..14_000;
        let mut best = f64::MAX;
        for shift in 0..32usize {
            let err: f64 = mid
                .clone()
                .map(|i| ((out[i + shift.min(out.len() - i - 1)] - ideal[i]) as f64).powi(2))
                .sum::<f64>()
                / 12_000.0;
            best = best.min(err.sqrt());
        }
        // Delay is zero for this centered kernel, so shift 0 must already be accurate.
        let direct: f64 = (2_000..14_000)
            .map(|i| ((out[i] - ideal[i]) as f64).powi(2))
            .sum::<f64>()
            / 12_000.0;
        assert!(direct.sqrt() < 0.01, "rms error {}", direct.sqrt());
        assert!(best < 0.01);
    }

    #[test]
    fn frequencies_above_the_new_nyquist_are_rejected() {
        // 12 kHz would alias to 4 kHz at 16 kHz output.
        let out = resample_to_16k(&sine(12_000.0, 48_000, 1.0), 48_000);
        assert!(
            rms(&out[1000..15_000]) < 0.5 * std::f64::consts::FRAC_1_SQRT_2 * 0.01,
            "{}",
            rms(&out)
        );
    }

    #[test]
    fn streaming_in_small_blocks_matches_one_shot() {
        let input = sine(440.0, 44_100, 0.5);
        let mut one = Resampler::new(44_100, 16_000);
        let whole = one.process(&input);
        let mut many = Resampler::new(44_100, 16_000);
        let mut pieces = Vec::new();
        for chunk in input.chunks(441) {
            pieces.extend(many.process(chunk));
        }
        assert_eq!(whole.len(), pieces.len());
        for (a, b) in whole.iter().zip(&pieces) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn upsampling_to_24k_preserves_the_tone() {
        let mut r = Resampler::new(16_000, 24_000);
        let out = r.process(&sine(500.0, 16_000, 0.5));
        let ideal = sine(500.0, 24_000, 0.5);
        let err: f64 = (1000..6000)
            .map(|i| ((out[i] - ideal[i]) as f64).powi(2))
            .sum::<f64>()
            / 5000.0;
        assert!(err.sqrt() < 0.01, "{}", err.sqrt());
    }

    #[test]
    fn sixteen_k_input_is_untouched() {
        let x = sine(300.0, 16_000, 0.1);
        assert_eq!(resample_to_16k(&x, 16_000), x);
    }

    #[test]
    fn pcm_float_round_trip_and_clamping() {
        let pcm = f32_to_pcm16(&[0.0, 0.5, -1.0, 2.0, f32::NAN]);
        let back = pcm16_to_f32(&pcm);
        assert_eq!(back.len(), 5);
        assert!((back[1] - 0.5).abs() < 1e-3);
        assert!((back[2] + 1.0).abs() < 1e-3);
        assert!(back[3] > 0.999);
        assert_eq!(back[4], 0.0);
        assert_eq!(peak(&[0.1, -0.7, 0.3]), 0.7);
        assert_eq!(peak(&[f32::INFINITY]), 0.0);
    }

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.25, 0.75], 1), vec![0.25, 0.75]);
    }

    #[test]
    fn wav_round_trip_and_rejections() {
        let pcm = f32_to_pcm16(&[0.0, 0.25, -0.25, 0.5]);
        let wav = wav_encode(16_000, &pcm);
        let decoded = wav_decode(&wav).unwrap();
        assert_eq!(decoded.rate, 16_000);
        assert_eq!(decoded.samples.len(), 4);
        assert!((decoded.samples[1] - 0.25).abs() < 1e-3);
        assert!(wav_decode(b"not a wav file at all").is_err());
        let mut stereo = wav.clone();
        stereo[22] = 3; // three channels
        assert!(wav_decode(&stereo).is_err());
        let playback = to_playback_pcm(&decoded).unwrap();
        assert!(playback.len() >= 4 * 2);
    }
}
