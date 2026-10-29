//! Capture device list and selection (`AssistantMicrophones.cs`) for later voice work.
//!
//! The list always starts with the Windows default entry. Saved ids from the C# app look
//! like `<guid>|<product name>` (legacy WaveIn names, cut to 31 characters); they still
//! resolve here by matching the product name against the endpoint names.
#![allow(dead_code)]

/// One selectable input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Microphone {
    /// Endpoint id; empty for "Windows default microphone".
    pub id: String,
    pub name: String,
    /// Position in the list; the default entry is 0.
    pub index: usize,
}

pub const DEFAULT_NAME: &str = "Windows default microphone";

pub const UNAVAILABLE_MESSAGE: &str = "Your selected microphone is unavailable or has a duplicate name. Reconnect it, or choose another input in Voice settings.";

/// WaveIn product names are limited to 31 characters.
const LEGACY_NAME_LIMIT: usize = 31;

/// List with the default entry first, then the given `(id, name)` pairs.
pub fn with_default(devices: impl IntoIterator<Item = (String, String)>) -> Vec<Microphone> {
    std::iter::once((String::new(), DEFAULT_NAME.to_owned()))
        .chain(devices)
        .enumerate()
        .map(|(index, (id, name))| Microphone { id, name, index })
        .collect()
}

/// Picks the device for a saved id. `Ok(None)` means "use the Windows default".
pub fn resolve<'a>(
    id: Option<&str>,
    devices: &'a [Microphone],
) -> Result<Option<&'a Microphone>, &'static str> {
    let id = match id {
        None | Some("") => return Ok(None),
        Some(id) => id,
    };
    let exact: Vec<_> = devices
        .iter()
        .filter(|d| d.index > 0 && d.id == id)
        .collect();
    if exact.len() == 1 {
        return Ok(Some(exact[0]));
    }
    if exact.len() > 1 {
        return Err(UNAVAILABLE_MESSAGE);
    }
    // Legacy `<guid>|<product name>` ids.
    if let Some((_, product)) = id.split_once('|') {
        let product = product.trim();
        if !product.is_empty() {
            let matches: Vec<_> = devices
                .iter()
                .filter(|d| d.index > 0 && legacy_name_matches(product, &d.name))
                .collect();
            if matches.len() == 1 {
                return Ok(Some(matches[0]));
            }
        }
    }
    Err(UNAVAILABLE_MESSAGE)
}

fn legacy_name_matches(product: &str, endpoint_name: &str) -> bool {
    let product = product.to_lowercase();
    let name = endpoint_name.to_lowercase();
    if name == product {
        return true;
    }
    // A truncated WaveIn name is a prefix of the full endpoint name.
    product.chars().count() >= LEGACY_NAME_LIMIT - 1 && name.starts_with(&product)
}

/// Enumerates active capture endpoints (Windows only).
#[cfg(windows)]
pub fn list() -> Vec<Microphone> {
    use crate::devices::{self, ComApartment};
    let _com = ComApartment::mta();
    let found = devices::enumerator()
        .and_then(|en| devices::active_devices(&en, windows::Win32::Media::Audio::eCapture))
        .map(|devs| {
            devs.iter()
                .filter_map(|d| Some((devices::device_id(d).ok()?, devices::friendly_name(d))))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    with_default(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mics() -> Vec<Microphone> {
        with_default([
            (
                "{0.0.1.00000000}.{a}".to_owned(),
                "Microphone (Realtek High Definition Audio)".to_owned(),
            ),
            (
                "{0.0.1.00000000}.{b}".to_owned(),
                "Headset (USB)".to_owned(),
            ),
        ])
    }

    #[test]
    fn list_starts_with_the_windows_default() {
        let m = mics();
        assert_eq!(m[0].name, DEFAULT_NAME);
        assert_eq!(m[0].id, "");
        assert_eq!(m.iter().map(|d| d.index).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn empty_or_missing_id_means_default() {
        let m = mics();
        assert_eq!(resolve(None, &m), Ok(None));
        assert_eq!(resolve(Some(""), &m), Ok(None));
    }

    #[test]
    fn exact_id_resolves_and_unknown_is_rejected() {
        let m = mics();
        assert_eq!(
            resolve(Some("{0.0.1.00000000}.{b}"), &m)
                .unwrap()
                .unwrap()
                .index,
            2
        );
        assert_eq!(resolve(Some("gone"), &m), Err(UNAVAILABLE_MESSAGE));
    }

    #[test]
    fn duplicate_ids_are_ambiguous() {
        let mut m = mics();
        m.push(Microphone {
            id: m[2].id.clone(),
            name: "Dup".into(),
            index: 3,
        });
        assert_eq!(
            resolve(Some("{0.0.1.00000000}.{b}"), &m),
            Err(UNAVAILABLE_MESSAGE)
        );
    }

    #[test]
    fn legacy_wavein_ids_match_by_product_name() {
        let m = mics();
        let legacy = "00000000-0000-0000-0000-000000000000|Headset (USB)";
        assert_eq!(resolve(Some(legacy), &m).unwrap().unwrap().index, 2);
        // Truncated to 31 chars by WaveIn.
        let truncated = "guid|Microphone (Realtek High Defini";
        assert_eq!(truncated.split_once('|').unwrap().1.chars().count(), 31);
        assert_eq!(resolve(Some(truncated), &m).unwrap().unwrap().index, 1);
        assert_eq!(
            resolve(Some("guid|Nothing like it"), &m),
            Err(UNAVAILABLE_MESSAGE)
        );
    }

    #[test]
    fn ambiguous_legacy_names_are_rejected() {
        let m = with_default([
            ("a".to_owned(), "USB Microphone".to_owned()),
            ("b".to_owned(), "USB Microphone".to_owned()),
        ]);
        assert_eq!(
            resolve(Some("guid|USB Microphone"), &m),
            Err(UNAVAILABLE_MESSAGE)
        );
    }
}
