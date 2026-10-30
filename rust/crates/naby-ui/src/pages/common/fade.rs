//! Cross-fades between two values (`SwapText` 220 ms, `SwapArtwork` 320 ms), and the page
//! presence heuristic.

/// Holds the current value and, while a swap is running, the outgoing one.
#[derive(Debug, Clone)]
pub struct Crossfade<T: Clone + PartialEq> {
    cur: T,
    prev: Option<T>,
    since: f64,
    secs: f64,
}

impl<T: Clone + PartialEq> Crossfade<T> {
    pub fn new(value: T, secs: f64) -> Self {
        Crossfade {
            cur: value,
            prev: None,
            since: f64::NEG_INFINITY,
            secs,
        }
    }

    /// Stores `value`; with `animate` the previous one fades out. True when it changed.
    pub fn set(&mut self, value: T, now: f64, animate: bool) -> bool {
        if value == self.cur {
            return false;
        }
        let old = std::mem::replace(&mut self.cur, value);
        if animate {
            self.prev = Some(old);
            self.since = now;
        } else {
            self.prev = None;
        }
        true
    }

    pub fn current(&self) -> &T {
        &self.cur
    }

    fn t(&self, now: f64) -> f64 {
        ((now - self.since) / self.secs).clamp(0.0, 1.0)
    }

    /// Opacity of the current value (cubic ease-out, `Motion.Fade`).
    pub fn alpha(&self, now: f64) -> f32 {
        if self.prev.is_none() {
            return 1.0;
        }
        let t = self.t(now);
        (1.0 - (1.0 - t).powi(3)) as f32
    }

    /// The outgoing value and its opacity while the swap runs.
    pub fn outgoing(&self, now: f64) -> Option<(&T, f32)> {
        let prev = self.prev.as_ref()?;
        let t = self.t(now);
        (t < 1.0).then(|| (prev, (1.0 - t).powi(3) as f32))
    }

    pub fn animating(&self, now: f64) -> bool {
        self.prev.is_some() && self.t(now) < 1.0
    }

    /// Drops the outgoing value once the swap is over.
    pub fn settle(&mut self, now: f64) {
        if self.prev.is_some() && self.t(now) >= 1.0 {
            self.prev = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_fades_out_the_old_value() {
        let mut f = Crossfade::new("a".to_owned(), 0.2);
        assert!(!f.set("a".into(), 1.0, true));
        assert!(f.set("b".into(), 1.0, true));
        assert_eq!(f.alpha(1.0), 0.0);
        let (old, a) = f.outgoing(1.0).unwrap();
        assert_eq!((old.as_str(), a), ("a", 1.0));
        assert!(f.animating(1.1));
        assert!(!f.animating(1.3));
        assert_eq!(f.alpha(1.3), 1.0);
        f.settle(1.3);
        assert!(f.outgoing(1.3).is_none());
        // Without motion the swap is instant.
        assert!(f.set("c".into(), 2.0, false));
        assert!(f.outgoing(2.0).is_none());
    }
}
