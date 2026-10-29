//! The two system file dialogs the tool pages use: "Add files..." for the shelf (C#
//! `OpenFileDialog { Multiselect }`) and "Save as PNG..." for clipboard images
//! (`SaveFileDialog`). Both run their own modal loop on the UI thread.
//!
//! The buffer rules of the classic `GetOpenFileName` multi-select format are pure functions
//! tested on every platform.

/// Splits the `OPENFILENAME` multi-select buffer: one path, or a directory followed by file
/// names, each separated by NUL, ended by a double NUL.
pub fn parse_multi_select(buffer: &[u16]) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0;
    for i in 0..buffer.len() {
        if buffer[i] == 0 {
            if i == start {
                break;
            }
            parts.push(String::from_utf16_lossy(&buffer[start..i]));
            start = i + 1;
        }
    }
    match parts.len() {
        0 => Vec::new(),
        1 => parts,
        _ => {
            let dir = parts.remove(0);
            let dir = dir.trim_end_matches('\\');
            parts
                .into_iter()
                .map(|name| format!("{dir}\\{name}"))
                .collect()
        }
    }
}

/// Appends `.png` when the user typed a name without an extension (`AddExtension`).
pub fn with_png_extension(path: &str) -> String {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    if name.contains('.') {
        path.to_owned()
    } else {
        format!("{path}.png")
    }
}

#[cfg(windows)]
pub use os::{pick_files, save_png_as};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::util::wide;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, GetSaveFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST,
        OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    const BUFFER: usize = 32 * 1024;

    /// Shows the open dialog and returns the chosen paths (empty when cancelled).
    pub fn pick_files(title: &str) -> Vec<String> {
        let mut buffer = vec![0u16; BUFFER];
        let title = wide(title);
        let mut dialog = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFile: windows::core::PWSTR(buffer.as_mut_ptr()),
            nMaxFile: BUFFER as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            Flags: OFN_ALLOWMULTISELECT
                | OFN_EXPLORER
                | OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR,
            ..Default::default()
        };
        // SAFETY: `dialog` points at buffers that outlive the modal call.
        let chosen = unsafe { GetOpenFileNameW(&mut dialog) }.as_bool();
        if chosen {
            parse_multi_select(&buffer)
        } else {
            Vec::new()
        }
    }

    /// Shows the save dialog for a PNG named `file_name`; `None` when cancelled.
    pub fn save_png_as(title: &str, file_name: &str) -> Option<String> {
        let mut buffer = vec![0u16; 1024];
        for (slot, unit) in buffer.iter_mut().zip(file_name.encode_utf16()).take(1023) {
            *slot = unit;
        }
        let title = wide(title);
        let filter: Vec<u16> = "PNG image (*.png)\0*.png\0\0".encode_utf16().collect();
        let ext = wide("png");
        let mut dialog = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(buffer.as_mut_ptr()),
            nMaxFile: buffer.len() as u32,
            lpstrTitle: PCWSTR(title.as_ptr()),
            lpstrDefExt: PCWSTR(ext.as_ptr()),
            Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER,
            ..Default::default()
        };
        // SAFETY: `dialog` points at buffers that outlive the modal call.
        let chosen = unsafe { GetSaveFileNameW(&mut dialog) }.as_bool();
        chosen.then(|| with_png_extension(&crate::util::from_wide(&buffer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(parts: &[&str]) -> Vec<u16> {
        let mut out: Vec<u16> = Vec::new();
        for p in parts {
            out.extend(p.encode_utf16());
            out.push(0);
        }
        out.push(0);
        out.extend([0, 0, 0]);
        out
    }

    #[test]
    fn a_single_selection_is_the_full_path() {
        assert_eq!(
            parse_multi_select(&buf(&["C:\\a\\b.txt"])),
            ["C:\\a\\b.txt"]
        );
    }

    #[test]
    fn several_selections_join_the_directory_and_names() {
        assert_eq!(
            parse_multi_select(&buf(&["C:\\dir", "a.txt", "b c.png"])),
            ["C:\\dir\\a.txt", "C:\\dir\\b c.png"]
        );
        // A drive root keeps one separator.
        assert_eq!(parse_multi_select(&buf(&["C:\\", "a.txt"])), ["C:\\a.txt"]);
    }

    #[test]
    fn an_empty_or_cancelled_buffer_selects_nothing() {
        assert!(parse_multi_select(&[0, 0, 0]).is_empty());
        assert!(parse_multi_select(&[]).is_empty());
    }

    #[test]
    fn png_names_get_their_extension() {
        assert_eq!(
            with_png_extension("C:\\x\\Clipboard image"),
            "C:\\x\\Clipboard image.png"
        );
        assert_eq!(with_png_extension("C:\\x.dir\\shot"), "C:\\x.dir\\shot.png");
        assert_eq!(with_png_extension("C:\\x\\shot.png"), "C:\\x\\shot.png");
    }
}
