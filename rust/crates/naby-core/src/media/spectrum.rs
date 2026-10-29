//! Log-spaced FFT bands of the output mix (`SpectrumTransform`).
//!
//! All buffers are allocated in [`Spectrum::new`]; feeding samples never allocates.

use std::f64::consts::PI;

/// Number of log-spaced bands (40 Hz .. 16 kHz).
pub const BANDS: usize = 25;
const SIZE: usize = 2048;
const LOG2: u32 = 11;

pub struct Spectrum {
    rate: u32,
    hop: u32,
    ring: Box<[f32; SIZE]>,
    window: Box<[f32; SIZE]>,
    re: Box<[f32; SIZE]>,
    im: Box<[f32; SIZE]>,
    cos: Box<[f32; SIZE / 2]>,
    sin: Box<[f32; SIZE / 2]>,
    reverse: Box<[u16; SIZE]>,
    first_bin: [usize; BANDS],
    last_bin: [usize; BANDS],
    bands: [f32; BANDS],
    cursor: usize,
    filled: usize,
    since: u32,
    sequence: u64,
}

impl std::fmt::Debug for Spectrum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spectrum")
            .field("rate", &self.rate)
            .field("sequence", &self.sequence)
            .finish()
    }
}

fn boxed<T: Copy, const N: usize>(fill: T) -> Box<[T; N]> {
    vec![fill; N]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("length is N"))
}

impl Spectrum {
    /// `rate` is the sample rate of the mono stream passed to [`add`](Self::add).
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1);
        let mut window = boxed::<f32, SIZE>(0.0);
        for (i, w) in window.iter_mut().enumerate() {
            *w = (0.5 - 0.5 * (2.0 * PI * i as f64 / (SIZE - 1) as f64).cos()) as f32;
        }
        let mut cos = boxed::<f32, { SIZE / 2 }>(0.0);
        let mut sin = boxed::<f32, { SIZE / 2 }>(0.0);
        for i in 0..SIZE / 2 {
            let angle = -2.0 * PI * i as f64 / SIZE as f64;
            cos[i] = angle.cos() as f32;
            sin[i] = angle.sin() as f32;
        }
        let mut reverse = boxed::<u16, SIZE>(0);
        for (i, r) in reverse.iter_mut().enumerate() {
            *r = ((i as u32).reverse_bits() >> (32 - LOG2)) as u16;
        }
        let mut first_bin = [0usize; BANDS];
        let mut last_bin = [0usize; BANDS];
        for band in 0..BANDS {
            let low = 40.0 * 400f64.powf(band as f64 / BANDS as f64);
            let high = 40.0 * 400f64.powf((band + 1) as f64 / BANDS as f64);
            let first = ((low * SIZE as f64 / rate as f64).floor() as usize).clamp(1, SIZE / 2 - 1);
            let last =
                ((high * SIZE as f64 / rate as f64).ceil() as usize).clamp(first + 1, SIZE / 2);
            first_bin[band] = first;
            last_bin[band] = last;
        }
        Spectrum {
            rate,
            hop: (rate / 30).max(1),
            ring: boxed(0.0),
            window,
            re: boxed(0.0),
            im: boxed(0.0),
            cos,
            sin,
            reverse,
            first_bin,
            last_bin,
            bands: [0.0; BANDS],
            cursor: 0,
            filled: 0,
            since: 0,
            sequence: 0,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// Latest bands, each 0..=1.
    pub fn bands(&self) -> &[f32; BANDS] {
        &self.bands
    }

    /// Number of completed transforms; 0 means "no measurement yet".
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Feeds mono samples and returns the latest bands.
    pub fn push(&mut self, samples: &[f32]) -> &[f32] {
        for &s in samples {
            self.add(s);
        }
        &self.bands
    }

    /// Feeds interleaved frames, averaging the channels.
    pub fn push_interleaved(&mut self, data: &[f32], channels: usize) -> &[f32] {
        if channels > 0 {
            for frame in data.chunks_exact(channels) {
                let mono = frame.iter().sum::<f32>() / channels as f32;
                self.add(mono);
            }
        }
        &self.bands
    }

    pub fn add(&mut self, sample: f32) {
        self.ring[self.cursor] = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.cursor = (self.cursor + 1) % SIZE;
        self.filled = (self.filled + 1).min(SIZE);
        self.since = self.since.saturating_add(1);
        if self.filled < SIZE || self.since < self.hop {
            return;
        }
        self.since = 0;
        self.transform();
    }

    fn transform(&mut self) {
        for i in 0..SIZE {
            let src = self.ring[(self.cursor + i) % SIZE] * self.window[i];
            let dst = self.reverse[i] as usize;
            self.re[dst] = src;
            self.im[dst] = 0.0;
        }
        let mut len = 2;
        while len <= SIZE {
            let half = len / 2;
            let step = SIZE / len;
            let mut start = 0;
            while start < SIZE {
                for k in 0..half {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (start + k, start + k + half);
                    let tr = self.re[b] * wr - self.im[b] * wi;
                    let ti = self.re[b] * wi + self.im[b] * wr;
                    self.re[b] = self.re[a] - tr;
                    self.im[b] = self.im[a] - ti;
                    self.re[a] += tr;
                    self.im[a] += ti;
                }
                start += len;
            }
            len *= 2;
        }
        let scale = 1.0 / SIZE as f32;
        for band in 0..BANDS {
            let mut energy = 0.0f64;
            for i in self.first_bin[band]..self.last_bin[band] {
                let (r, m) = (self.re[i] * scale, self.im[i] * scale);
                energy += (r * r + m * m) as f64;
            }
            let db = 20.0 * energy.sqrt().max(1e-9).log10();
            self.bands[band] = ((db + 68.0) / 60.0).clamp(0.0, 1.0) as f32;
        }
        self.sequence += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (0.3 * (2.0 * PI * hz * i as f64 / rate as f64).sin()) as f32)
            .collect()
    }

    fn argmax(b: &[f32]) -> usize {
        b.iter()
            .enumerate()
            .fold((0, f32::MIN), |m, (i, &v)| if v > m.1 { (i, v) } else { m })
            .0
    }

    #[test]
    fn spectrum_silence_is_flat() {
        let mut s = Spectrum::new(48_000);
        assert_eq!(s.sequence(), 0);
        let bands = s.push(&vec![0.0; 8000]).to_vec();
        assert_eq!(bands.len(), BANDS);
        assert!(bands.iter().all(|&v| v == 0.0));
        assert!(s.sequence() > 0);
    }

    #[test]
    fn real_fft_separates_bass_from_treble_with_bounded_bands() {
        let mut bass = Spectrum::new(48_000);
        let b = bass.push(&tone(110.0, 48_000, 16_000)).to_vec();
        let mut treble = Spectrum::new(48_000);
        let t = treble.push(&tone(4000.0, 48_000, 16_000)).to_vec();
        let (m1, m2) = (argmax(&b), argmax(&t));
        assert!(m2 as i32 - m1 as i32 > 10, "{m1} {m2}");
        assert!(b
            .iter()
            .chain(&t)
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
        assert!(t[m2] > 0.7, "{}", t[m2]);
    }

    #[test]
    fn invalid_and_out_of_range_samples_are_sanitized() {
        let mut s = Spectrum::new(44_100);
        let mut data = vec![f32::NAN, f32::INFINITY, -f32::INFINITY, 9.0, -9.0];
        data.extend(tone(1000.0, 44_100, 6000));
        let bands = s.push(&data);
        assert!(bands
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
    }

    #[test]
    fn hop_follows_sample_rate_and_odd_rates_are_safe() {
        let mut s = Spectrum::new(48_000);
        s.push(&vec![0.0; SIZE - 1]);
        assert_eq!(s.sequence(), 0);
        s.push(&[0.0]);
        assert_eq!(s.sequence(), 1);
        s.push(&vec![0.0; 1599]);
        assert_eq!(s.sequence(), 1);
        s.push(&[0.0]);
        assert_eq!(s.sequence(), 2);
        for rate in [0, 1, 100, 8000, 192_000, u32::MAX] {
            let mut s = Spectrum::new(rate);
            s.push(&vec![0.1; 4096]);
        }
    }

    #[test]
    fn interleaved_downmix() {
        let mono = tone(440.0, 48_000, 8192);
        let stereo: Vec<f32> = mono.iter().flat_map(|&v| [v, v]).collect();
        let mut a = Spectrum::new(48_000);
        let mut b = Spectrum::new(48_000);
        let x = a.push(&mono).to_vec();
        let y = b.push_interleaved(&stereo, 2).to_vec();
        assert_eq!(x, y);
        assert_eq!(b.push_interleaved(&stereo, 0).len(), BANDS);
    }
}
