//! OLE drag and drop (C# `DragShelf.cs` drop half, `VisualShelf` / `NotificationOverlay` /
//! `DetachedPreviewViews` drag-out).
//!
//! * [`register`] makes a window an `IDropTarget` for **files** (`CF_HDROP`); the overlay host
//!   passes its HWND and a handler that decides, per event, whether the drop is acceptable
//!   (C# `CanShelfDrop`) and receives the dropped paths. Everything runs on the thread that
//!   registered (the UI thread), so the handler needs no `Send`.
//! * [`drag_files_out`] runs `DoDragDrop` with the shell's own data object for the given files
//!   (Copy | Link, as C#), blocking until the drag ends.
//!
//! Decoding `CF_HDROP` and the drag-continue rules are pure and tested everywhere.
#![cfg_attr(not(windows), allow(dead_code))]

/// What the target tells the handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropEvent {
    /// A drag with files entered the window. Paths are available for the preview.
    Enter {
        files: Vec<String>,
        x: i32,
        y: i32,
    },
    Over {
        x: i32,
        y: i32,
    },
    Leave,
    /// The files were dropped.
    Drop {
        files: Vec<String>,
        x: i32,
        y: i32,
    },
}

/// Decodes a `DROPFILES` block (`CF_HDROP`): header, then a double-NUL terminated list of paths
/// (UTF-16 when `fWide`, otherwise ANSI treated as Latin-1/UTF-8 lossy). Malformed data yields
/// what could be read.
pub fn parse_hdrop(data: &[u8]) -> Vec<String> {
    let Some(offset) = data
        .get(0..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    else {
        return Vec::new();
    };
    let wide = data
        .get(16..20)
        .is_some_and(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) != 0);
    let Some(list) = data.get(offset as usize..) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if wide {
        let units: Vec<u16> = list
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        for path in units.split(|&u| u == 0) {
            if path.is_empty() {
                break;
            }
            out.push(String::from_utf16_lossy(path));
        }
    } else {
        for path in list.split(|&b| b == 0) {
            if path.is_empty() {
                break;
            }
            out.push(String::from_utf8_lossy(path).into_owned());
        }
    }
    out
}

/// Removes duplicate paths (case-insensitive), keeping the first.
pub fn dedupe_paths(paths: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    paths
        .into_iter()
        .filter(|p| seen.insert(p.to_uppercase()))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragContinue {
    Continue,
    Drop,
    Cancel,
}

/// `IDropSource::QueryContinueDrag`: Esc cancels, releasing the left button drops.
pub fn continue_drag(escape_pressed: bool, key_state: u32) -> DragContinue {
    const MK_LBUTTON: u32 = 1;
    if escape_pressed {
        DragContinue::Cancel
    } else if key_state & MK_LBUTTON == 0 {
        DragContinue::Drop
    } else {
        DragContinue::Continue
    }
}

/// How a drag out ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    Dropped,
    Cancelled,
    /// The drag could not start (no valid paths, shell failure).
    Failed,
}

#[cfg(windows)]
pub use os::{drag_files_out, register, DropHandler, DropRegistration};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::svc_util::{pcwstr, wide};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use windows::core::{implement, Ref, BOOL};
    use windows::Win32::Foundation::{
        DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINTL, S_OK,
    };
    use windows::Win32::System::Com::{
        CoTaskMemFree, IDataObject, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{
        DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, OleInitialize,
        RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
        DROPEFFECT_LINK, DROPEFFECT_NONE,
    };
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    };

    /// Decides whether a drag/drop is acceptable. Called for `Enter`/`Over` (true = show the copy
    /// cursor) and for `Drop` (the files are consumed when it returns true).
    pub type DropHandler = Rc<dyn Fn(&DropEvent) -> bool>;

    /// Unregisters the drop target when dropped.
    pub struct DropRegistration {
        hwnd: HWND,
    }

    impl Drop for DropRegistration {
        fn drop(&mut self) {
            // SAFETY: revokes the registration made for this same window handle.
            unsafe {
                let _ = RevokeDragDrop(self.hwnd);
            }
        }
    }

    fn files_of(data: &IDataObject) -> Vec<String> {
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        // SAFETY: GetData fills an STGMEDIUM we own and release; the HGLOBAL is locked only
        // while its GlobalSize bytes are copied.
        unsafe {
            let Ok(mut medium) = data.GetData(&format) else {
                return Vec::new();
            };
            let memory = medium.u.hGlobal;
            let size = GlobalSize(memory);
            let mut bytes = Vec::new();
            if size > 0 && size < 64 * 1024 * 1024 {
                let source = GlobalLock(memory);
                if !source.is_null() {
                    bytes = std::slice::from_raw_parts(source.cast::<u8>(), size).to_vec();
                    let _ = GlobalUnlock(memory);
                }
            }
            ReleaseStgMedium(&mut medium as *mut STGMEDIUM);
            dedupe_paths(parse_hdrop(&bytes))
        }
    }

    fn has_files(data: &IDataObject) -> bool {
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        // SAFETY: a format query on a live data object.
        unsafe { data.QueryGetData(&format) }.0 == 0
    }

    #[implement(IDropTarget)]
    struct DropTarget {
        handler: DropHandler,
        files: Cell<bool>,
        paths: RefCell<Vec<String>>,
    }

    #[allow(non_snake_case)]
    impl IDropTarget_Impl for DropTarget_Impl {
        fn DragEnter(
            &self,
            pdataobj: Ref<'_, IDataObject>,
            _keys: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            let has = pdataobj.as_ref().is_some_and(has_files);
            self.files.set(has);
            let accepted = has && {
                let files = pdataobj.as_ref().map(files_of).unwrap_or_default();
                *self.paths.borrow_mut() = files.clone();
                (self.handler)(&DropEvent::Enter {
                    files,
                    x: pt.x,
                    y: pt.y,
                })
            };
            // SAFETY: OLE passes a valid effect pointer.
            unsafe {
                *effect = if accepted {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                }
            };
            Ok(())
        }

        fn DragOver(
            &self,
            _keys: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            let accepted =
                self.files.get() && (self.handler)(&DropEvent::Over { x: pt.x, y: pt.y });
            // SAFETY: OLE passes a valid effect pointer.
            unsafe {
                *effect = if accepted {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                }
            };
            Ok(())
        }

        fn DragLeave(&self) -> windows::core::Result<()> {
            if self.files.replace(false) {
                (self.handler)(&DropEvent::Leave);
            }
            self.paths.borrow_mut().clear();
            Ok(())
        }

        fn Drop(
            &self,
            pdataobj: Ref<'_, IDataObject>,
            _keys: MODIFIERKEYS_FLAGS,
            pt: &POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            let mut files = pdataobj.as_ref().map(files_of).unwrap_or_default();
            if files.is_empty() {
                files = std::mem::take(&mut *self.paths.borrow_mut());
            }
            let accepted = !files.is_empty()
                && (self.handler)(&DropEvent::Drop {
                    files,
                    x: pt.x,
                    y: pt.y,
                });
            self.files.set(false);
            self.paths.borrow_mut().clear();
            // SAFETY: OLE passes a valid effect pointer.
            unsafe {
                *effect = if accepted {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                }
            };
            Ok(())
        }
    }

    /// Registers `hwnd` as a file drop target. Call on the UI thread that owns the window; OLE is
    /// initialised for that thread if it is not already.
    pub fn register(hwnd: HWND, handler: DropHandler) -> Result<DropRegistration, String> {
        // SAFETY: OLE initialisation and registration for a window of this thread. S_FALSE
        // (already initialised) is not an error for OleInitialize's Result mapping.
        unsafe {
            let _ = OleInitialize(None);
            let target: IDropTarget = DropTarget {
                handler,
                files: Cell::new(false),
                paths: RefCell::new(Vec::new()),
            }
            .into();
            RegisterDragDrop(hwnd, &target).map_err(|e| e.message())?;
        }
        Ok(DropRegistration { hwnd })
    }

    #[implement(IDropSource)]
    struct DropSource;

    #[allow(non_snake_case)]
    impl IDropSource_Impl for DropSource_Impl {
        fn QueryContinueDrag(
            &self,
            escape: BOOL,
            keys: MODIFIERKEYS_FLAGS,
        ) -> windows::core::HRESULT {
            match continue_drag(escape.as_bool(), keys.0) {
                DragContinue::Continue => S_OK,
                DragContinue::Drop => DRAGDROP_S_DROP,
                DragContinue::Cancel => DRAGDROP_S_CANCEL,
            }
        }

        fn GiveFeedback(&self, _effect: DROPEFFECT) -> windows::core::HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    /// Drags `paths` out of the dock (C# `DoDragDrop(FileDrop, Copy | Link)`). Missing paths are
    /// skipped. Blocks in OLE's modal loop until the button is released or Esc is pressed, so
    /// call it from a mouse handler on the UI thread.
    pub fn drag_files_out(paths: &[String]) -> DragOutcome {
        let existing: Vec<&String> = paths
            .iter()
            .filter(|p| std::path::Path::new(p.as_str()).exists())
            .collect();
        if existing.is_empty() {
            return DragOutcome::Failed;
        }
        // SAFETY: shell PIDLs are parsed, wrapped in a shell item array and freed after the data
        // object is created; the drag runs on this thread after OLE initialisation.
        unsafe {
            let _ = OleInitialize(None);
            let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
            for path in &existing {
                let w = wide(path);
                let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
                if SHParseDisplayName(pcwstr(&w), None, &mut pidl, 0, None).is_ok()
                    && !pidl.is_null()
                {
                    pidls.push(pidl);
                }
            }
            let free = |list: &[*mut ITEMIDLIST]| {
                for &p in list {
                    CoTaskMemFree(Some(p.cast_const().cast()));
                }
            };
            if pidls.is_empty() {
                return DragOutcome::Failed;
            }
            let constant: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.cast_const()).collect();
            let data: Result<IDataObject, _> = SHCreateShellItemArrayFromIDLists(&constant)
                .and_then(|array| array.BindToHandler(None, &BHID_DataObject));
            free(&pidls);
            let Ok(data) = data else {
                return DragOutcome::Failed;
            };
            let source: IDropSource = DropSource.into();
            let mut effect = DROPEFFECT_NONE;
            let result = DoDragDrop(
                &data,
                &source,
                DROPEFFECT_COPY | DROPEFFECT_LINK,
                &mut effect,
            );
            if result == DRAGDROP_S_DROP {
                DragOutcome::Dropped
            } else if result == DRAGDROP_S_CANCEL {
                DragOutcome::Cancelled
            } else {
                DragOutcome::Failed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrop(paths: &[&str], wide: bool) -> Vec<u8> {
        let mut v = vec![0u8; 20];
        v[0..4].copy_from_slice(&20u32.to_le_bytes());
        v[16..20].copy_from_slice(&u32::from(wide).to_le_bytes());
        for p in paths {
            if wide {
                p.encode_utf16()
                    .chain([0])
                    .for_each(|u| v.extend_from_slice(&u.to_le_bytes()));
            } else {
                v.extend_from_slice(p.as_bytes());
                v.push(0);
            }
        }
        v.extend_from_slice(if wide { &[0, 0] } else { &[0] });
        v
    }

    #[test]
    fn hdrop_lists_wide_and_ansi_paths() {
        let wide = hdrop(&[r"C:\a\one.txt", r"C:\b\zwei ü.png"], true);
        assert_eq!(
            parse_hdrop(&wide),
            vec![r"C:\a\one.txt", r"C:\b\zwei ü.png"]
        );
        let ansi = hdrop(&[r"C:\a\one.txt"], false);
        assert_eq!(parse_hdrop(&ansi), vec![r"C:\a\one.txt"]);
        assert_eq!(parse_hdrop(&hdrop(&[], true)), Vec::<String>::new());
    }

    #[test]
    fn malformed_hdrop_never_panics() {
        assert!(parse_hdrop(&[]).is_empty());
        assert!(parse_hdrop(&[1, 2, 3]).is_empty());
        let mut bad = hdrop(&["x"], true);
        bad[0..4].copy_from_slice(&9999u32.to_le_bytes());
        assert!(parse_hdrop(&bad).is_empty());
        let truncated = &hdrop(&[r"C:\file"], true)[..25];
        let _ = parse_hdrop(truncated);
    }

    #[test]
    fn duplicate_drops_collapse_case_insensitively() {
        let paths = dedupe_paths(vec![
            r"C:\A.txt".into(),
            r"c:\a.TXT".into(),
            r"C:\b.txt".into(),
        ]);
        assert_eq!(paths, vec![r"C:\A.txt", r"C:\b.txt"]);
    }

    #[test]
    fn drag_source_rules() {
        assert_eq!(continue_drag(false, 1), DragContinue::Continue);
        assert_eq!(
            continue_drag(false, 0),
            DragContinue::Drop,
            "left button released"
        );
        assert_eq!(continue_drag(true, 1), DragContinue::Cancel, "Esc wins");
        assert_eq!(
            continue_drag(false, 1 | 8),
            DragContinue::Continue,
            "ctrl held while dragging"
        );
    }
}
