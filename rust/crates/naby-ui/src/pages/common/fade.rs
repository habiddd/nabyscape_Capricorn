//! Cross-fades between two values (`SwapText` 220 ms, `SwapArtwork` 320 ms), and the page
//! presence heuristic.

use std::cell::Cell;

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

/// Knows whether a page was drawn recently. The overlay keeps every visited page alive and asks
/// all of them for `next_frame` and `demand`; a page that is no longer shown must go quiet.
#[derive(Debug, Default)]
pub struct Presence {
    seen: Cell<f64>,
    clock: Cell<f64>,
    shown: Cell<bool>,
}

/// A page that has not been drawn for this long is treated as hidden.
const STALE: f64 = 3.5;

impl Presence {
    pub fn new() -> Self {
        Presence::default()
    }

    /// Called from `frame` / `peek` with the host clock.
    pub fn mark(&self, now: f64) {
        self.seen.set(now);
        self.clock.set(now);
        self.shown.set(true);
    }

    /// Called from `next_frame` with the host clock.
    pub fn observe(&self, now: f64) {
        if now > self.clock.get() {
            self.clock.set(now);
        }
    }

    pub fn active(&self) -> bool {
        self.shown.get() && self.clock.get() - self.seen.get() < STALE
    }

    pub fn hide(&self) {
        self.shown.set(false);
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

    #[test]
    fn presence_goes_stale_when_frames_stop() {
        let p = Presence::new();
        assert!(!p.active());
        p.mark(10.0);
        assert!(p.active());
        p.observe(12.0);
        assert!(p.active());
        p.observe(14.0);
        assert!(!p.active());
        p.mark(14.5);
        assert!(p.active());
        p.hide();
        assert!(!p.active());
    }
}
