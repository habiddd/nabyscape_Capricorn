//! Endpoint peak sampling on a two-second discovery lease (`PeakCache<T>`).
//!
//! A broken endpoint is released immediately and the next lease retries discovery,
//! so neither errors nor silence cause per-frame discovery. `read` does not allocate.

use std::time::{Duration, Instant};

const LEASE: Duration = Duration::from_secs(2);

type Enumerate<T> = Box<dyn FnMut() -> Result<Vec<T>, String> + Send>;
type Sample<T> = Box<dyn FnMut(&mut T) -> Result<f32, String> + Send>;
type Release<T> = Box<dyn FnMut(T) -> Result<(), String> + Send>;
type OnError = Box<dyn FnMut(&str) + Send>;

pub struct PeakCache<T> {
    enumerate: Enumerate<T>,
    sample: Sample<T>,
    release: Release<T>,
    error: OnError,
    endpoints: Vec<T>,
    refresh_at: Option<Instant>,
}

impl<T> PeakCache<T> {
    pub fn new(
        enumerate: impl FnMut() -> Result<Vec<T>, String> + Send + 'static,
        sample: impl FnMut(&mut T) -> Result<f32, String> + Send + 'static,
        release: impl FnMut(T) -> Result<(), String> + Send + 'static,
        error: impl FnMut(&str) + Send + 'static,
    ) -> Self {
        PeakCache {
            enumerate: Box::new(enumerate),
            sample: Box::new(sample),
            release: Box::new(release),
            error: Box::new(error),
            endpoints: Vec::new(),
            refresh_at: None,
        }
    }

    pub fn endpoint_count(&self) -> usize {
        self.endpoints.len()
    }

    /// Highest finite peak (clamped 0..=1) across endpoints; re-enumerates every 2 s.
    pub fn read(&mut self, now: Instant) -> f32 {
        if self.refresh_at.is_none_or(|at| now >= at) {
            self.dispose();
            self.refresh_at = Some(now + LEASE);
            match (self.enumerate)() {
                Ok(found) => self.endpoints = found,
                Err(e) => (self.error)(&e),
            }
        }
        let mut peak = 0.0f32;
        for i in (0..self.endpoints.len()).rev() {
            match (self.sample)(&mut self.endpoints[i]) {
                Ok(v) => {
                    if v.is_finite() {
                        peak = peak.max(v.clamp(0.0, 1.0));
                    }
                }
                Err(e) => {
                    (self.error)(&e);
                    let endpoint = self.endpoints.remove(i);
                    if let Err(e) = (self.release)(endpoint) {
                        (self.error)(&e);
                    }
                }
            }
        }
        peak
    }

    /// Releases every endpoint; the next `read` enumerates again only after the lease.
    pub fn dispose(&mut self) {
        for endpoint in self.endpoints.drain(..) {
            if let Err(e) = (self.release)(endpoint) {
                (self.error)(&e);
            }
        }
    }
}

impl<T> Drop for PeakCache<T> {
    fn drop(&mut self) {
        self.dispose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Probe {
        enumerated: AtomicUsize,
        released: AtomicUsize,
        errors: Mutex<Vec<String>>,
    }

    fn cache(probe: &Arc<Probe>, values: Vec<Result<f32, String>>) -> PeakCache<usize> {
        let (p1, p2, p3) = (probe.clone(), probe.clone(), probe.clone());
        let n = values.len();
        PeakCache::new(
            move || {
                p1.enumerated.fetch_add(1, SeqCst);
                Ok((0..n).collect())
            },
            move |i| values[*i].clone(),
            move |_| {
                p2.released.fetch_add(1, SeqCst);
                Ok(())
            },
            move |e| p3.errors.lock().unwrap().push(e.to_owned()),
        )
    }

    #[test]
    fn takes_max_of_clamped_finite_values_on_a_two_second_lease() {
        let probe = Arc::new(Probe::default());
        let mut c = cache(&probe, vec![Ok(0.2), Ok(1.7), Ok(f32::NAN), Ok(-3.0)]);
        let t = Instant::now();
        assert_eq!(c.read(t), 1.0);
        assert_eq!(c.read(t + Duration::from_millis(1999)), 1.0);
        assert_eq!(probe.enumerated.load(SeqCst), 1);
        c.read(t + Duration::from_secs(2));
        assert_eq!(probe.enumerated.load(SeqCst), 2);
        assert_eq!(probe.released.load(SeqCst), 4);
    }

    #[test]
    fn broken_endpoint_is_removed_and_reported_then_rediscovered() {
        let probe = Arc::new(Probe::default());
        let mut c = cache(&probe, vec![Ok(0.4), Err("gone".into())]);
        let t = Instant::now();
        assert_eq!(c.read(t), 0.4);
        assert_eq!(c.endpoint_count(), 1);
        assert_eq!(probe.errors.lock().unwrap().as_slice(), ["gone"]);
        assert_eq!(probe.released.load(SeqCst), 1);
        c.read(t + Duration::from_secs(3));
        assert_eq!(probe.enumerated.load(SeqCst), 2);
    }

    #[test]
    fn enumeration_failure_yields_silence_without_panicking() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let e = errors.clone();
        let mut c: PeakCache<u8> = PeakCache::new(
            || Err("no audio".into()),
            |_| Ok(1.0),
            |_| Ok(()),
            move |m| e.lock().unwrap().push(m.to_owned()),
        );
        assert_eq!(c.read(Instant::now()), 0.0);
        assert_eq!(errors.lock().unwrap().len(), 1);
    }

    #[test]
    fn drop_releases_endpoints() {
        let probe = Arc::new(Probe::default());
        {
            let mut c = cache(&probe, vec![Ok(0.1), Ok(0.2)]);
            c.read(Instant::now());
        }
        assert_eq!(probe.released.load(SeqCst), 2);
    }
}
