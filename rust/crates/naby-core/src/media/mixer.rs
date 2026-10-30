//! Volume mixer models and the pure adjust/validation rules (`AudioMixer.cs`).
//! Enumeration and writes happen in the Windows host; it passes snapshots in and
//! supplies the writer closure.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MixerScope {
    #[default]
    Apps,
    Outputs,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct MixerTarget {
    pub scope: MixerScope,
    pub output_id: String,
    /// Session instance id; empty for outputs.
    pub session_id: String,
}

impl MixerTarget {
    pub fn output(output_id: impl Into<String>) -> Self {
        MixerTarget {
            scope: MixerScope::Outputs,
            output_id: output_id.into(),
            session_id: String::new(),
        }
    }

    pub fn app(output_id: impl Into<String>, session_id: impl Into<String>) -> Self {
        MixerTarget {
            scope: MixerScope::Apps,
            output_id: output_id.into(),
            session_id: session_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MixerItem {
    pub target: MixerTarget,
    pub name: String,
    pub output_name: String,
    /// 0..=1 scalar.
    pub volume: f64,
    pub muted: bool,
    pub process_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MixerSnapshot {
    pub outputs: Vec<MixerItem>,
    pub apps: Vec<MixerItem>,
    pub errors: Vec<String>,
}

impl MixerSnapshot {
    /// Orders like the C# `OrderBy(Name)` (outputs) / `OrderBy(Name).ThenBy(OutputName)` (apps).
    pub fn sort(&mut self) {
        let key = |s: &str| s.to_lowercase();
        self.outputs.sort_by_key(|i| key(&i.name));
        self.apps
            .sort_by_key(|i| (key(&i.name), key(&i.output_name)));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MixerResult {
    pub changed: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

impl MixerResult {
    pub fn summary(&self) -> String {
        let mut s = format!("{} changed \u{b7} {} failed", self.changed, self.failed);
        if !self.errors.is_empty() {
            s.push_str(" \u{b7} ");
            s.push_str(&self.errors.join("; "));
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MixerError {
    NotFinite,
}

impl std::fmt::Display for MixerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Volume must be finite.")
    }
}

impl std::error::Error for MixerError {}

/// Clamps to 0..=1; NaN and infinities are rejected.
pub fn bounded_volume(value: f64) -> Result<f64, MixerError> {
    if value.is_finite() {
        Ok(value.clamp(0.0, 1.0))
    } else {
        Err(MixerError::NotFinite)
    }
}

/// Only running (0) and inactive (1) sessions can be controlled; expired (2) cannot.
pub fn is_controllable_session(state: i32) -> bool {
    matches!(state, 0 | 1)
}

/// Mouse capture, or a focused slider with a volume key held, freezes list refresh.
pub fn interaction_active(
    mouse_captured: bool,
    slider_focused: bool,
    volume_key_down: bool,
) -> bool {
    mouse_captured || (slider_focused && volume_key_down)
}

/// Applies `delta` to every item of `scope` through `write` (which returns an error
/// message on failure). One failing target never stops the others, and duplicate
/// targets are written once.
pub fn adjust(
    snapshot: &MixerSnapshot,
    scope: MixerScope,
    delta: f64,
    mut write: impl FnMut(&MixerTarget, f64) -> Option<String>,
) -> Result<MixerResult, MixerError> {
    if !delta.is_finite() {
        return Err(MixerError::NotFinite);
    }
    let items = match scope {
        MixerScope::Apps => &snapshot.apps,
        MixerScope::Outputs => &snapshot.outputs,
    };
    let mut seen: HashSet<&MixerTarget> = HashSet::new();
    let mut result = MixerResult::default();
    for item in items {
        if !seen.insert(&item.target) {
            continue;
        }
        let outcome = bounded_volume(item.volume + delta)
            .map_err(|e| e.to_string())
            .map(|value| write(&item.target, value));
        match outcome {
            Ok(None) => result.changed += 1,
            Ok(Some(error)) | Err(error) => {
                result.failed += 1;
                result.errors.push(format!("{}: {error}", item.name));
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(target: MixerTarget, name: &str, out: &str, volume: f64, muted: bool) -> MixerItem {
        MixerItem {
            target,
            name: name.into(),
            output_name: out.into(),
            volume,
            muted,
            process_path: None,
        }
    }

    fn fixture() -> (MixerSnapshot, MixerItem, MixerItem, MixerItem) {
        let output = item(
            MixerTarget::output("speakers"),
            "Speakers",
            "Speakers",
            0.98,
            false,
        );
        let first = item(
            MixerTarget::app("speakers", "instance-a"),
            "App A",
            "Speakers",
            0.01,
            false,
        );
        let second = item(
            MixerTarget::app("headphones", "instance-b"),
            "App B",
            "Headphones",
            0.6,
            true,
        );
        let snap = MixerSnapshot {
            outputs: vec![output.clone()],
            apps: vec![first.clone(), second.clone()],
            errors: vec![],
        };
        (snap, output, first, second)
    }

    #[test]
    fn gain_clamps_and_rejects_non_finite() {
        assert_eq!(bounded_volume(-0.1), Ok(0.0));
        assert_eq!(bounded_volume(1.1), Ok(1.0));
        assert_eq!(bounded_volume(0.5), Ok(0.5));
        assert!(bounded_volume(f64::NAN).is_err());
        assert!(bounded_volume(f64::INFINITY).is_err());
    }

    #[test]
    fn all_apps_adjusts_sessions_without_endpoint_gain() {
        let (snap, _, _, _) = fixture();
        let mut writes = vec![];
        let r = adjust(&snap, MixerScope::Apps, -0.05, |t, v| {
            writes.push((t.clone(), v));
            None
        })
        .unwrap();
        assert_eq!(r.changed, 2);
        assert_eq!(writes.len(), 2);
        assert!(writes.iter().all(|w| w.0.scope == MixerScope::Apps));
        assert_eq!(writes[0].1, 0.0);
        assert!((writes[1].1 - 0.55).abs() < 1e-5);
    }

    #[test]
    fn all_outputs_adjusts_only_endpoint_masters() {
        let (snap, output, _, _) = fixture();
        let mut writes = vec![];
        let r = adjust(&snap, MixerScope::Outputs, 0.05, |t, v| {
            writes.push((t.clone(), v));
            None
        })
        .unwrap();
        assert_eq!((r.changed, writes.len()), (1, 1));
        assert_eq!(writes[0].0, output.target);
        assert_eq!(writes[0].1, 1.0);
    }

    #[test]
    fn stale_session_failure_preserves_remaining_operations() {
        let (snap, _, first, second) = fixture();
        let mut writes = vec![];
        let r = adjust(&snap, MixerScope::Apps, 0.05, |t, v| {
            if *t == first.target {
                Some("Session ended".into())
            } else {
                writes.push((t.clone(), v));
                None
            }
        })
        .unwrap();
        assert_eq!((r.changed, r.failed), (1, 1));
        assert!(r.errors[0].contains("Session ended") && r.errors[0].starts_with("App A: "));
        assert_eq!(writes[0].0, second.target);
        assert_eq!(
            r.summary(),
            "1 changed \u{b7} 1 failed \u{b7} App A: Session ended"
        );
    }

    #[test]
    fn duplicate_session_instances_never_double_scale() {
        let (mut snap, _, first, _) = fixture();
        snap.apps = vec![first.clone(), first];
        let mut n = 0;
        adjust(&snap, MixerScope::Apps, 0.05, |_, _| {
            n += 1;
            None
        })
        .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn non_finite_delta_or_volume_never_writes() {
        let (mut snap, _, _, _) = fixture();
        assert!(adjust(&snap, MixerScope::Apps, f64::NAN, |_, _| panic!("write")).is_err());
        snap.apps[0].volume = f64::NAN;
        let r = adjust(&snap, MixerScope::Apps, 0.1, |_, _| None).unwrap();
        assert_eq!((r.changed, r.failed), (1, 1));
    }

    #[test]
    fn session_state_and_interaction_rules() {
        assert!(is_controllable_session(0) && is_controllable_session(1));
        assert!(!is_controllable_session(2) && !is_controllable_session(-1));
        assert!(interaction_active(true, false, false) && interaction_active(false, true, true));
        assert!(!interaction_active(false, true, false) && !interaction_active(false, false, true));
    }

    #[test]
    fn snapshot_sorts_by_name_then_output() {
        let (mut snap, _, _, _) = fixture();
        snap.apps
            .push(item(MixerTarget::app("x", "c"), "app a", "Zed", 0.0, false));
        snap.apps.reverse();
        snap.sort();
        let names: Vec<_> = snap
            .apps
            .iter()
            .map(|i| (i.name.as_str(), i.output_name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("App A", "Speakers"),
                ("app a", "Zed"),
                ("App B", "Headphones")
            ]
        );
    }
}
