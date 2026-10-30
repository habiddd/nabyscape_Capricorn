//! Countdown sound synthesis and WAV handling (`CountdownAudio.cs`). Pure: the
//! Windows player in `sounds.rs` only hands the resulting bytes to `PlaySound`.
#![allow(dead_code)]

use std::f64::consts::PI;

pub const SAMPLE_RATE: u32 = 22_050;
/// Custom sounds: file size and duration limits from the C# validation.
pub const CUSTOM_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const CUSTOM_MAX_SECONDS: f64 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountdownSound {
    Off,
    Bell,
    Chime,
    Custom,
}

impl CountdownSound {
    /// Unknown names behave like the C# `Tone` switch: anything that is not Bell/Off/Custom is the chime.
    pub fn parse(name: &str) -> Self {
        match name {
            "Off" => CountdownSound::Off,
            "Bell" => CountdownSound::Bell,
            "Custom" => CountdownSound::Custom,
            _ => CountdownSound::Chime,
        }
    }
}

/// 16-bit mono PCM, two seconds, scaled by `volume` (0..=1).
pub fn tone_pcm(kind: CountdownSound, volume: f32) -> Vec<i16> {
    let rate = SAMPLE_RATE as f64;
    let length = (SAMPLE_RATE * 2) as usize;
    let tones = [(0.0, 659.25), (0.16, 830.61), (0.32, 987.77)];
    let gain = f64::from(volume.clamp(0.0, 1.0));
    (0..length)
        .map(|i| {
            let t = i as f64 / rate;
            let value = if kind == CountdownSound::Bell {
                ((2.0 * PI * 660.0 * t).sin() + 0.35 * (2.0 * PI * 1323.0 * t).sin())
                    * (-t * 3.4).exp()
            } else {
                let mut v = 0.0;
                for (start, freq) in tones {
                    let elapsed = t - start;
                    if elapsed >= 0.0 {
                        v += 0.42
                            * (2.0 * PI * freq * elapsed).sin()
                            * (elapsed / 0.008).min(1.0)
                            * (-elapsed * 4.1).exp();
                    }
                }
                v
            };
            ((value * 0.45).clamp(-0.95, 0.95) * f64::from(i16::MAX) * gain) as i16
        })
        .collect()
}

/// Canonical 44-byte-header WAV around 16-bit PCM samples.
pub fn wav_from_pcm16(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Built-in tone as an in-memory WAV file.
pub fn tone_wav(kind: CountdownSound, volume: f32) -> Vec<u8> {
    wav_from_pcm16(SAMPLE_RATE, 1, &tone_pcm(kind, volume))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    NotWav,
    Truncated,
    Unsupported,
    TooLong,
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            WavError::NotWav => "Choose a WAV file under 10 MB.",
            WavError::Truncated => "The WAV file is damaged or incomplete.",
            WavError::Unsupported => "This WAV format is not supported. Use PCM or float audio.",
            WavError::TooLong => "Choose a sound no longer than 15 seconds.",
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Layout {
    format: u16,
    bits: u16,
    byte_rate: u32,
    data_start: usize,
    data_len: usize,
}

fn parse_layout(wav: &[u8]) -> Result<Layout, WavError> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err(WavError::NotWav);
    }
    let mut pos = 12;
    let mut fmt: Option<(u16, u16, u32)> = None;
    while pos + 8 <= wav.len() {
        let id = &wav[pos..pos + 4];
        let size =
            u32::from_le_bytes([wav[pos + 4], wav[pos + 5], wav[pos + 6], wav[pos + 7]]) as usize;
        let body = pos + 8;
        if id == b"fmt " {
            if size < 16 || body + size > wav.len() {
                return Err(WavError::Truncated);
            }
            let mut tag = u16::from_le_bytes([wav[body], wav[body + 1]]);
            let byte_rate =
                u32::from_le_bytes([wav[body + 8], wav[body + 9], wav[body + 10], wav[body + 11]]);
            let bits = u16::from_le_bytes([wav[body + 14], wav[body + 15]]);
            if tag == 0xFFFE {
                // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID starts with the real tag.
                if size < 40 {
                    return Err(WavError::Truncated);
                }
                tag = u16::from_le_bytes([wav[body + 24], wav[body + 25]]);
            }
            fmt = Some((tag, bits, byte_rate));
        } else if id == b"data" {
            let (format, bits, byte_rate) = fmt.ok_or(WavError::Truncated)?;
            let len = size.min(wav.len() - body);
            return Ok(Layout {
                format,
                bits,
                byte_rate,
                data_start: body,
                data_len: len,
            });
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    Err(WavError::Truncated)
}

/// Playing time in seconds of a PCM/float WAV image.
pub fn wav_duration(wav: &[u8]) -> Result<f64, WavError> {
    let layout = parse_layout(wav)?;
    if layout.byte_rate == 0 {
        return Err(WavError::Unsupported);
    }
    Ok(layout.data_len as f64 / f64::from(layout.byte_rate))
}

/// Validates a user-selected sound before reading it: `.wav`, at most 10 MiB.
pub fn custom_file_acceptable(file_name: &str, len: u64) -> Result<(), WavError> {
    let wav = file_name.to_ascii_lowercase().ends_with(".wav");
    if wav && len <= CUSTOM_MAX_BYTES {
        Ok(())
    } else {
        Err(WavError::NotWav)
    }
}

/// Copy of `wav` with every sample scaled by `volume`. Supports 8/16/24/32-bit PCM and
/// 32/64-bit float; other encodings are reported as unsupported. Rejects sounds over 15 s.
pub fn scale_wav(wav: &[u8], volume: f32) -> Result<Vec<u8>, WavError> {
    let layout = parse_layout(wav)?;
    if layout.byte_rate == 0 {
        return Err(WavError::Unsupported);
    }
    if layout.data_len as f64 / f64::from(layout.byte_rate) > CUSTOM_MAX_SECONDS {
        return Err(WavError::TooLong);
    }
    let gain = volume.clamp(0.0, 1.0);
    let mut out = wav.to_vec();
    let data = &mut out[layout.data_start..layout.data_start + layout.data_len];
    match (layout.format, layout.bits) {
        (1, 8) => {
            for b in data.iter_mut() {
                let centered = f32::from(*b) - 128.0;
                *b = (centered * gain + 128.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        (1, 16) => {
            for c in data.chunks_exact_mut(2) {
                let v = f32::from(i16::from_le_bytes([c[0], c[1]])) * gain;
                c.copy_from_slice(&(v.round() as i16).to_le_bytes());
            }
        }
        (1, 24) => {
            for c in data.chunks_exact_mut(3) {
                let raw = i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8;
                let v = (raw as f32 * gain).round() as i32;
                let b = v.to_le_bytes();
                c.copy_from_slice(&b[0..3]);
            }
        }
        (1, 32) => {
            for c in data.chunks_exact_mut(4) {
                let v = (f64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])) * f64::from(gain))
                    as i32;
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        (3, 32) => {
            for c in data.chunks_exact_mut(4) {
                let v = f32::from_le_bytes([c[0], c[1], c[2], c[3]]) * gain;
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        (3, 64) => {
            for c in data.chunks_exact_mut(8) {
                let mut b = [0u8; 8];
                b.copy_from_slice(c);
                let v = f64::from_le_bytes(b) * f64::from(gain);
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        _ => return Err(WavError::Unsupported),
    }
    Ok(out)
}

/// What `Play` should do for the current settings (`CountdownAudio.Play`).
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Silent,
    Tone(CountdownSound),
    Custom(String),
}

pub fn plan(sound: &str, volume: f64, custom_file: &str) -> Plan {
    let kind = CountdownSound::parse(sound);
    if kind == CountdownSound::Off || volume.is_nan() || volume <= 0.0 {
        return Plan::Silent;
    }
    if kind == CountdownSound::Custom {
        Plan::Custom(custom_file.to_owned())
    } else {
        Plan::Tone(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_are_two_seconds_and_bounded() {
        for kind in [CountdownSound::Bell, CountdownSound::Chime] {
            let pcm = tone_pcm(kind, 1.0);
            assert_eq!(pcm.len(), (SAMPLE_RATE * 2) as usize);
            let peak = pcm.iter().map(|s| i32::from(*s).abs()).max().unwrap();
            assert!(peak > 3000, "{kind:?} audible, got {peak}");
            assert!(peak <= (0.95 * 32767.0) as i32);
        }
    }

    #[test]
    fn bell_starts_silent_and_decays() {
        let pcm = tone_pcm(CountdownSound::Bell, 1.0);
        assert_eq!(pcm[0], 0);
        let early = pcm[..2000]
            .iter()
            .map(|s| i32::from(*s).abs())
            .max()
            .unwrap();
        let late = pcm[pcm.len() - 2000..]
            .iter()
            .map(|s| i32::from(*s).abs())
            .max()
            .unwrap();
        assert!(late * 10 < early);
    }

    #[test]
    fn chime_notes_enter_in_sequence() {
        let pcm = tone_pcm(CountdownSound::Chime, 1.0);
        let at = |sec: f64| {
            let i = (sec * f64::from(SAMPLE_RATE)) as usize;
            pcm[i..i + 400]
                .iter()
                .map(|s| i32::from(*s).abs())
                .max()
                .unwrap()
        };
        assert!(at(0.05) > 1000);
        assert!(at(0.2) > 1000);
        assert!(at(0.36) > 1000);
    }

    #[test]
    fn volume_scales_linearly_and_zero_is_silent() {
        let full = tone_pcm(CountdownSound::Chime, 1.0);
        let half = tone_pcm(CountdownSound::Chime, 0.5);
        let peak = |v: &[i16]| v.iter().map(|s| i32::from(*s).abs()).max().unwrap();
        assert!((peak(&full) / 2 - peak(&half)).abs() <= 1);
        assert!(tone_pcm(CountdownSound::Bell, 0.0).iter().all(|s| *s == 0));
    }

    #[test]
    fn parse_and_plan_follow_the_settings() {
        assert_eq!(CountdownSound::parse("Bell"), CountdownSound::Bell);
        assert_eq!(CountdownSound::parse("whatever"), CountdownSound::Chime);
        assert_eq!(plan("Off", 1.0, ""), Plan::Silent);
        assert_eq!(plan("Bell", 0.0, ""), Plan::Silent);
        assert_eq!(plan("Bell", f64::NAN, ""), Plan::Silent);
        assert_eq!(plan("Chime", 0.5, ""), Plan::Tone(CountdownSound::Chime));
        assert_eq!(plan("Custom", 0.5, "a.wav"), Plan::Custom("a.wav".into()));
    }

    #[test]
    fn wav_header_round_trips_through_the_parser() {
        let wav = tone_wav(CountdownSound::Bell, 1.0);
        assert_eq!(wav.len(), 44 + (SAMPLE_RATE * 2 * 2) as usize);
        let d = wav_duration(&wav).unwrap();
        assert!((d - 2.0).abs() < 1e-6);
    }

    #[test]
    fn scaling_halves_pcm16_and_keeps_the_header() {
        let wav = wav_from_pcm16(8000, 1, &[1000, -2000, 32767]);
        let out = scale_wav(&wav, 0.5).unwrap();
        assert_eq!(&out[..44], &wav[..44]);
        let s: Vec<i16> = out[44..]
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(s, [500, -1000, 16384]);
        let silent = scale_wav(&wav, 0.0).unwrap();
        assert!(silent[44..].iter().all(|b| *b == 0));
    }

    fn wav_with(format: u16, bits: u16, rate: u32, data: &[u8]) -> Vec<u8> {
        let ch = 1u16;
        let block = bits / 8 * ch;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&format.to_le_bytes());
        v.extend_from_slice(&ch.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&(rate * u32::from(block)).to_le_bytes());
        v.extend_from_slice(&block.to_le_bytes());
        v.extend_from_slice(&bits.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn scaling_handles_float_8bit_24bit_and_rejects_adpcm() {
        let f = wav_with(3, 32, 1000, &0.8f32.to_le_bytes());
        let out = scale_wav(&f, 0.5).unwrap();
        assert!((f32::from_le_bytes(out[44..48].try_into().unwrap()) - 0.4).abs() < 1e-6);
        let b8 = wav_with(1, 8, 1000, &[128, 255, 0]);
        assert_eq!(&scale_wav(&b8, 0.5).unwrap()[44..], &[128, 192, 64]);
        let raw24 = 0x400000i32.to_le_bytes();
        let b24 = wav_with(1, 24, 1000, &raw24[0..3]);
        let out = scale_wav(&b24, 0.5).unwrap();
        assert_eq!(
            i32::from_le_bytes([0, out[44], out[45], out[46]]) >> 8,
            0x200000
        );
        let adpcm = wav_with(2, 4, 1000, &[0, 0]);
        assert_eq!(scale_wav(&adpcm, 1.0), Err(WavError::Unsupported));
    }

    #[test]
    fn long_and_broken_files_are_rejected() {
        let long = wav_with(1, 16, 1000, &vec![0u8; 2000 * 16]);
        assert_eq!(scale_wav(&long, 1.0), Err(WavError::TooLong));
        assert_eq!(scale_wav(b"not a wav file", 1.0), Err(WavError::NotWav));
        let mut cut = wav_with(1, 16, 1000, &[0u8; 8]);
        cut.truncate(30);
        assert!(scale_wav(&cut, 1.0).is_err());
        assert_eq!(wav_duration(b"RIFF"), Err(WavError::NotWav));
    }

    #[test]
    fn custom_file_rules() {
        assert!(custom_file_acceptable("ding.WAV", 1024).is_ok());
        assert!(custom_file_acceptable("ding.mp3", 1024).is_err());
        assert!(custom_file_acceptable("ding.wav", CUSTOM_MAX_BYTES + 1).is_err());
    }
}
