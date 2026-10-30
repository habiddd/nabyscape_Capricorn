//! App catalog and icons (C# `AppCatalog.cs`, `AppIconCache.cs`, `AppPicker.Search`).
//!
//! The catalog (UWP/desktop apps from `shell:AppsFolder`, plus `.lnk`/`.url` files under the
//! user and common Start Menu) is built lazily on a worker thread when the app picker opens and
//! dropped when it closes. Icons come from `IShellItemImageFactory` on a single STA worker and
//! land in a byte-bounded `ThumbnailCache`. The catalog rules (identity, validation, dedupe,
//! sorting, search) and the icon-path rules are pure and tested on every platform.
#![cfg_attr(not(windows), allow(dead_code))]

use naby_core::settings::Shortcut;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// One launchable app (C# `InstalledApp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    pub name: String,
    pub target: String,
    pub app_id: Option<String>,
}

impl InstalledApp {
    pub fn new(name: &str, target: &str, app_id: Option<&str>) -> Self {
        Self {
            name: name.to_owned(),
            target: target.to_owned(),
            app_id: app_id.map(str::to_owned),
        }
    }

    /// App user model id, or the full target path.
    pub fn identity(&self) -> String {
        match &self.app_id {
            Some(id) => id.clone(),
            None => full_path(&self.target),
        }
    }

    pub fn to_shortcut(&self) -> Shortcut {
        Shortcut {
            name: self.name.clone(),
            target: self.target.clone(),
            app_id: self.app_id.clone(),
        }
    }
}

/// `Path.GetFullPath` for already-qualified paths: unified separators, no trailing separator.
fn full_path(path: &str) -> String {
    let unified = path.replace('/', "\\");
    let trimmed = unified.trim_end_matches('\\');
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        unified
    } else {
        trimmed.to_owned()
    }
}

/// C# `ValidAppId`: `Package_hash!App` made of letters, digits and `. _ - !` with exactly one
/// `!` that is neither first nor last, at most 256 characters.
pub fn valid_app_id(id: &str) -> bool {
    let first = id.find('!');
    !id.trim().is_empty()
        && id.len() <= 256
        && first.is_some_and(|i| i > 0)
        && id.rfind('!') == first
        && !id.ends_with('!')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '!'))
}

/// `Path.IsPathFullyQualified` on Windows: `C:\x`, `C:/x` or `\\server\share`; not `\x`, `C:x`.
pub fn is_fully_qualified(path: &str) -> bool {
    let b = path.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/') {
        return true;
    }
    path.starts_with("\\\\") && !path.starts_with("\\\\.\\") || path.starts_with("\\\\?\\")
}

fn extension(path: &str) -> String {
    Path::new(path)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

/// C# `WindowsAppCatalog.Normalize`: drops blank names and invalid entries (an app id must be
/// valid; a path entry must be a fully qualified `.lnk`/`.url`), keeps the first of each
/// identity (case-insensitive), sorted by name (case-insensitive).
pub fn normalize(apps: impl IntoIterator<Item = InstalledApp>) -> Vec<InstalledApp> {
    let mut seen = HashSet::new();
    let mut out: Vec<InstalledApp> = apps
        .into_iter()
        .filter(|a| {
            !a.name.trim().is_empty()
                && match &a.app_id {
                    Some(id) => valid_app_id(id),
                    None => {
                        is_fully_qualified(&a.target)
                            && matches!(extension(&a.target).as_str(), ".lnk" | ".url")
                    }
                }
        })
        .filter(|a| seen.insert(a.identity().to_uppercase()))
        .collect();
    out.sort_by_cached_key(|a| a.name.to_lowercase());
    out
}

/// C# `AppPicker.Search`: case-insensitive substring of the name (empty query keeps all).
pub fn search<'a>(apps: &'a [InstalledApp], query: &str) -> Vec<&'a InstalledApp> {
    let needle = query.trim().to_lowercase();
    apps.iter()
        .filter(|a| a.name.to_lowercase().contains(&needle))
        .collect()
}

/// The single structured argument that launches an installed app through Explorer
/// (`ShortcutService.InstalledAppStartInfo`): `shell:AppsFolder\<id>`, or an error for an
/// invalid id.
pub fn installed_app_argument(app_id: &str) -> Result<String, &'static str> {
    if valid_app_id(app_id) {
        Ok(format!("shell:AppsFolder\\{app_id}"))
    } else {
        Err("This installed app shortcut is invalid. Remove it and add the app again in Settings.")
    }
}

fn is_reparse_point(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

/// Recursively adds `.lnk`/`.url` files under `directory` (names without extension), skipping
/// reparse points and unreadable folders. Stops early when `cancel` is set.
pub fn scan_shortcuts(directory: &Path, found: &mut Vec<InstalledApp>, cancel: &AtomicBool) {
    if cancel.load(Ordering::SeqCst) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut folders = Vec::new();
    for entry in entries.flatten() {
        if cancel.load(Ordering::SeqCst) {
            return;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            if !is_reparse_point(&meta) {
                folders.push(path);
            }
        } else if matches!(extension(&path.to_string_lossy()).as_str(), ".lnk" | ".url") {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            found.push(InstalledApp {
                name,
                target: path.to_string_lossy().into_owned(),
                app_id: None,
            });
        }
    }
    for folder in folders {
        scan_shortcuts(&folder, found, cancel);
    }
}

/// Where an icon comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IconSource {
    /// `shell:AppsFolder\<id>`.
    AppsFolder(String),
    /// A local file (`.lnk` targets are resolved by the extractor).
    File(String),
}

impl IconSource {
    /// The shell parsing name.
    pub fn parsing_name(&self) -> String {
        match self {
            Self::AppsFolder(id) => format!("shell:AppsFolder\\{id}"),
            Self::File(path) => path.clone(),
        }
    }
}

/// `%NAME%` expansion (`Environment.ExpandEnvironmentVariables`); unknown names stay as written.
pub fn expand_env(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) if end > 0 => match lookup(&after[..end]) {
                Some(value) => {
                    out.push_str(&value);
                    rest = &after[end + 1..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The syntactic half of C# `LocalPath`: a fully qualified drive path, never UNC or a URL.
pub fn is_drive_path(path: &str) -> bool {
    let b = path.as_bytes();
    is_fully_qualified(path)
        && !path.starts_with("\\\\")
        && b.len() > 2
        && b[1] == b':'
        && b[0].is_ascii_alphabetic()
}

/// C# `AppIconCache.GetAsync` source rules: `explorer.exe` and `ms-settings:` get their real
/// executables, an app id must be valid, and a path must be local. `windows_dir` is the Windows
/// folder; `local_drive` answers whether the drive is fixed/removable/optical.
pub fn icon_source(
    shortcut: &Shortcut,
    windows_dir: &str,
    expand: impl Fn(&str) -> String,
    local_drive: impl Fn(&str) -> bool,
) -> Option<IconSource> {
    if let Some(id) = &shortcut.app_id {
        return valid_app_id(id).then(|| IconSource::AppsFolder(id.clone()));
    }
    let mut target = expand(&shortcut.target);
    if target.eq_ignore_ascii_case("explorer.exe") {
        target = format!("{}\\explorer.exe", windows_dir.trim_end_matches('\\'));
    }
    if target.to_ascii_lowercase().starts_with("ms-settings:") {
        target = format!(
            "{}\\ImmersiveControlPanel\\SystemSettings.exe",
            windows_dir.trim_end_matches('\\')
        );
    }
    (is_drive_path(&target) && local_drive(&target)).then_some(IconSource::File(target))
}

/// Straight-alpha RGBA from a premultiplied top-down BGRA buffer (what the shell returns for
/// icons). A buffer without any alpha is treated as opaque.
pub fn bgra_premultiplied_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let any_alpha = bgra.chunks_exact(4).any(|p| p[3] != 0);
    let mut out = Vec::with_capacity(bgra.len());
    for p in bgra.chunks_exact(4) {
        let a = if any_alpha { p[3] } else { 255 };
        let un = |c: u8| -> u8 {
            if !any_alpha || a == 255 || a == 0 {
                c
            } else {
                ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
            }
        };
        out.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
    }
    out
}

#[cfg(windows)]
pub use os::{CatalogState, LauncherService};

#[cfg(windows)]
mod os {
    use super::*;
    use crate::svc_util::{from_pwstr, pcwstr, wide, ComGuard};
    use crate::wake::Waker;
    use naby_render::{Image, ThumbnailCache};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use windows::core::{Interface, GUID, PCWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, WIN32_FIND_DATAW};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
    };
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_CommonStartMenu, FOLDERID_StartMenu, IEnumShellItems, IShellItem,
        IShellItem2, IShellItemImageFactory, IShellLinkW, SHCreateItemFromParsingName,
        SHGetKnownFolderPath, ShellLink, KF_FLAG_DEFAULT, SIGDN_NORMALDISPLAY, SIIGBF,
    };

    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_CDROM: u32 = 5;
    /// `System.AppUserModel.ID`.
    const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3),
        pid: 5,
    };
    const MAX_ICON_PIXELS: u32 = 768;
    const ICON_CACHE_BYTES: usize = 4 * 1024 * 1024;
    const NEGATIVE_LIMIT: usize = 256;

    /// What the app picker shows.
    #[derive(Debug, Clone)]
    pub enum CatalogState {
        /// The picker is closed: no catalog is held.
        Closed,
        Loading,
        Ready(Arc<Vec<InstalledApp>>),
        Failed(String),
    }

    struct Catalog {
        epoch: u64,
        state: CatalogState,
        cancel: Arc<AtomicBool>,
    }

    struct IconRequest {
        key: String,
        source: IconSource,
        pixels: u32,
        thumbnail: bool,
    }

    #[derive(Default)]
    struct Icons {
        cache: Option<ThumbnailCache>,
        pending: HashSet<String>,
        absent: HashSet<String>,
    }

    struct Shared {
        catalog: Mutex<Catalog>,
        icons: Mutex<Icons>,
        wake: Waker,
    }

    pub struct LauncherService {
        shared: Arc<Shared>,
        catalog_thread: Option<JoinHandle<()>>,
        icon_tx: Option<Sender<IconRequest>>,
        icon_thread: Option<JoinHandle<()>>,
    }

    impl LauncherService {
        pub fn start(wake: Waker) -> Self {
            Self {
                shared: Arc::new(Shared {
                    catalog: Mutex::new(Catalog {
                        epoch: 0,
                        state: CatalogState::Closed,
                        cancel: Arc::new(AtomicBool::new(false)),
                    }),
                    icons: Mutex::new(Icons::default()),
                    wake,
                }),
                catalog_thread: None,
                icon_tx: None,
                icon_thread: None,
            }
        }

        /// Starts building the catalog (the app picker opened).
        pub fn open_picker(&mut self) {
            self.stop_catalog();
            let cancel = Arc::new(AtomicBool::new(false));
            let epoch = {
                let mut c = self
                    .shared
                    .catalog
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                c.epoch += 1;
                c.state = CatalogState::Loading;
                c.cancel = cancel.clone();
                c.epoch
            };
            let shared = self.shared.clone();
            self.catalog_thread = std::thread::Builder::new()
                .name("NabyScape app catalog".into())
                .spawn(move || {
                    let result = discover(&cancel);
                    let mut c = shared.catalog.lock().unwrap_or_else(|e| e.into_inner());
                    // A closed or reopened picker ignores a late result.
                    if c.epoch == epoch && !cancel.load(Ordering::SeqCst) {
                        c.state = match result {
                            Ok(apps) => CatalogState::Ready(Arc::new(apps)),
                            Err(message) => CatalogState::Failed(message),
                        };
                        drop(c);
                        (shared.wake)();
                    }
                })
                .ok();
            if self.catalog_thread.is_none() {
                let mut c = self
                    .shared
                    .catalog
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                c.state = CatalogState::Failed("Installed apps could not be listed.".to_owned());
            }
        }

        /// The picker closed: cancels the scan and drops the catalog.
        pub fn close_picker(&mut self) {
            self.stop_catalog();
            let mut c = self
                .shared
                .catalog
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            c.epoch += 1;
            c.state = CatalogState::Closed;
        }

        pub fn snapshot(&self) -> CatalogState {
            self.shared
                .catalog
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .state
                .clone()
        }

        fn stop_catalog(&mut self) {
            self.shared
                .catalog
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .cancel
                .store(true, Ordering::SeqCst);
            if let Some(t) = self.catalog_thread.take() {
                let _ = t.join();
            }
        }

        /// The icon for a shortcut at `pixels` (32 for the dock), or `None` while it loads or
        /// when none exists. The first miss queues extraction and wakes the UI when done.
        pub fn icon(&mut self, shortcut: &Shortcut, pixels: u32) -> Option<Arc<Image>> {
            let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
            let source = icon_source(
                shortcut,
                &windows_dir,
                |t| expand_env(t, |n| std::env::var(n).ok()),
                local_drive,
            )?;
            self.request(source, pixels.clamp(16, MAX_ICON_PIXELS), false)
        }

        /// A larger preview of a local file (shelf thumbnails), 64..=768 px.
        pub fn file_thumbnail(&mut self, path: &str, pixels: u32) -> Option<Arc<Image>> {
            if !is_drive_path(path) || !local_drive(path) {
                return None;
            }
            self.request(
                IconSource::File(path.to_owned()),
                pixels.clamp(64, 768),
                true,
            )
        }

        fn request(
            &mut self,
            source: IconSource,
            pixels: u32,
            thumbnail: bool,
        ) -> Option<Arc<Image>> {
            let key = format!(
                "{}|{}|{}",
                if thumbnail { "t" } else { "i" },
                source.parsing_name(),
                pixels
            );
            {
                let mut icons = self.shared.icons.lock().unwrap_or_else(|e| e.into_inner());
                let cache = icons
                    .cache
                    .get_or_insert_with(|| ThumbnailCache::new(ICON_CACHE_BYTES));
                if let Some(hit) = cache.get(&key) {
                    return Some(hit);
                }
                if icons.absent.contains(&key) || !icons.pending.insert(key.clone()) {
                    return None;
                }
            }
            let tx = self.icon_sender()?;
            if tx
                .send(IconRequest {
                    key: key.clone(),
                    source,
                    pixels,
                    thumbnail,
                })
                .is_err()
            {
                self.shared
                    .icons
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pending
                    .remove(&key);
            }
            None
        }

        fn icon_sender(&mut self) -> Option<Sender<IconRequest>> {
            if self.icon_tx.is_none() {
                let (tx, rx) = channel::<IconRequest>();
                let shared = self.shared.clone();
                self.icon_thread = std::thread::Builder::new()
                    .name("NabyScape icons".into())
                    .spawn(move || {
                        let _com = ComGuard::sta();
                        for request in rx {
                            let image = extract(&request.source, request.pixels, request.thumbnail);
                            let mut icons = shared.icons.lock().unwrap_or_else(|e| e.into_inner());
                            icons.pending.remove(&request.key);
                            match image {
                                Some(image) => {
                                    icons
                                        .cache
                                        .get_or_insert_with(|| {
                                            ThumbnailCache::new(ICON_CACHE_BYTES)
                                        })
                                        .insert(&request.key, Arc::new(image));
                                }
                                None => {
                                    if icons.absent.len() >= NEGATIVE_LIMIT {
                                        icons.absent.clear();
                                    }
                                    icons.absent.insert(request.key);
                                }
                            }
                            drop(icons);
                            (shared.wake)();
                        }
                    })
                    .ok();
                self.icon_tx = self.icon_thread.is_some().then_some(tx);
            }
            self.icon_tx.clone()
        }

        /// Drops cached icons (the owning surface hid).
        pub fn clear_icons(&self) {
            let mut icons = self.shared.icons.lock().unwrap_or_else(|e| e.into_inner());
            icons.cache = None;
            icons.pending.clear();
            icons.absent.clear();
        }
    }

    impl Drop for LauncherService {
        fn drop(&mut self) {
            self.stop_catalog();
            self.icon_tx = None; // the worker's receive loop ends
            if let Some(t) = self.icon_thread.take() {
                let _ = t.join();
            }
        }
    }

    fn local_drive(path: &str) -> bool {
        let Some(root) = path.get(..3) else {
            return false;
        };
        let w = wide(root);
        // SAFETY: `w` is a NUL-terminated drive root.
        let kind = unsafe { GetDriveTypeW(pcwstr(&w)) };
        matches!(kind, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_CDROM)
    }

    fn known_folder(id: &GUID) -> Option<String> {
        // SAFETY: the task-allocated path is copied and freed exactly once.
        unsafe {
            let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let text = from_pwstr(path.0);
            CoTaskMemFree(Some(path.0.cast_const().cast()));
            (!text.is_empty()).then_some(text)
        }
    }

    fn apps_folder_entries(
        cancel: &AtomicBool,
        found: &mut Vec<InstalledApp>,
    ) -> Result<(), String> {
        let name = wide("shell:AppsFolder");
        // SAFETY: shell COM calls on this STA thread; every returned string is freed once.
        unsafe {
            let folder: IShellItem =
                SHCreateItemFromParsingName(pcwstr(&name), None).map_err(|e| e.message())?;
            let items: IEnumShellItems = folder
                .BindToHandler(None, &BHID_EnumItems)
                .map_err(|e| e.message())?;
            loop {
                if cancel.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let mut batch: [Option<IShellItem>; 1] = [None];
                let mut fetched = 0u32;
                if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                let Some(item) = batch[0].take() else { break };
                let Ok(item2) = item.cast::<IShellItem2>() else {
                    continue;
                };
                let id = match item2.GetString(&PKEY_APP_USER_MODEL_ID) {
                    Ok(p) => {
                        let text = from_pwstr(p.0);
                        CoTaskMemFree(Some(p.0.cast_const().cast()));
                        text
                    }
                    Err(_) => continue,
                };
                if !valid_app_id(&id) {
                    continue;
                }
                let display = match item.GetDisplayName(SIGDN_NORMALDISPLAY) {
                    Ok(p) => {
                        let text = from_pwstr(p.0);
                        CoTaskMemFree(Some(p.0.cast_const().cast()));
                        text
                    }
                    Err(_) => String::new(),
                };
                found.push(InstalledApp {
                    name: if display.is_empty() {
                        id.clone()
                    } else {
                        display
                    },
                    target: format!("shell:AppsFolder\\{id}"),
                    app_id: Some(id),
                });
            }
        }
        Ok(())
    }

    fn discover(cancel: &AtomicBool) -> Result<Vec<InstalledApp>, String> {
        let _com = ComGuard::sta();
        let mut found = Vec::new();
        // A failure to enumerate the apps folder still leaves the Start Menu shortcuts usable.
        let apps_error = apps_folder_entries(cancel, &mut found).err();
        for id in [&FOLDERID_StartMenu, &FOLDERID_CommonStartMenu] {
            if let Some(dir) = known_folder(id) {
                scan_shortcuts(Path::new(&dir), &mut found, cancel);
            }
        }
        if cancel.load(Ordering::SeqCst) {
            return Ok(Vec::new());
        }
        let apps = normalize(found);
        match (apps.is_empty(), apps_error) {
            (true, Some(message)) => Err(message),
            _ => Ok(apps),
        }
    }

    /// Resolves a `.lnk` to its target path (WScript `TargetPath`), if it has one.
    fn link_target(path: &str) -> Option<String> {
        let w = wide(path);
        // SAFETY: COM calls on an STA thread; buffers are sized for the API.
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let file: IPersistFile = link.cast().ok()?;
            file.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
            let mut buffer = [0u16; 520];
            let mut data = WIN32_FIND_DATAW::default();
            link.GetPath(&mut buffer, &mut data, 0).ok()?;
            let text = crate::svc_util::from_wide(&buffer);
            (!text.is_empty()).then_some(text)
        }
    }

    fn extract(source: &IconSource, pixels: u32, thumbnail: bool) -> Option<Image> {
        let name = match source {
            IconSource::AppsFolder(_) => source.parsing_name(),
            IconSource::File(path) => {
                let ext = extension(path);
                if ext == ".url" {
                    return None;
                }
                let mut target = path.clone();
                if ext == ".lnk" {
                    if !Path::new(path).is_file() {
                        return None;
                    }
                    // A shortcut without a plain target (store apps) is drawn from the .lnk.
                    if let Some(resolved) = link_target(path) {
                        if is_drive_path(&resolved) && local_drive(&resolved) {
                            target = resolved;
                        }
                    }
                }
                target
            }
        };
        let w = wide(&name);
        // SAFETY: shell COM calls on this STA thread; the HBITMAP is deleted exactly once.
        unsafe {
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(pcwstr(&w), None).ok()?;
            let flags = if thumbnail {
                SIIGBF(0x1)
            } else {
                SIIGBF(0x104)
            };
            let bitmap = factory
                .GetImage(
                    SIZE {
                        cx: pixels as i32,
                        cy: pixels as i32,
                    },
                    flags,
                )
                .ok()?;
            let image = bitmap_to_image(bitmap);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            image
        }
    }

    /// Reads a 32-bit shell bitmap as an [`Image`].
    unsafe fn bitmap_to_image(bitmap: HBITMAP) -> Option<Image> {
        let mut info = BITMAP::default();
        // SAFETY: `info` is a BITMAP-sized writable buffer; the DC is released before returning.
        unsafe {
            if GetObjectW(
                HGDIOBJ(bitmap.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some((&mut info as *mut BITMAP).cast()),
            ) == 0
            {
                return None;
            }
            let (w, h) = (info.bmWidth, info.bmHeight.abs());
            if w <= 0 || h <= 0 || w as u32 > MAX_ICON_PIXELS * 2 || h as u32 > MAX_ICON_PIXELS * 2
            {
                return None;
            }
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; w as usize * h as usize * 4];
            let dc = GetDC(None);
            let lines = GetDIBits(
                dc,
                bitmap,
                0,
                h as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, dc);
            if lines == 0 {
                return None;
            }
            Image::from_rgba(w as u32, h as u32, bgra_premultiplied_to_rgba(&pixels))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, target: &str, id: Option<&str>) -> InstalledApp {
        InstalledApp::new(name, target, id)
    }

    #[test]
    fn catalog_deduplicates_identities_and_excludes_invalid_apps() {
        // Port of "Installed catalog deduplicates identities and excludes invalid apps".
        let apps = normalize([
            app(
                "Mail",
                "shell:AppsFolder\\Vendor.Mail_abc!App",
                Some("Vendor.Mail_abc!App"),
            ),
            app("Duplicate", "ignored", Some("vendor.mail_ABC!App")),
            app("Bad", "ignored", Some("bad! invalid")),
            app("Local", r"C:\Apps\Local.lnk", None),
            app("Website", r"C:\Apps\Website.url", None),
            app("Relative", "local.lnk", None),
        ]);
        assert_eq!(apps.len(), 3);
        assert_eq!(apps.iter().filter(|a| a.app_id.is_some()).count(), 1);
        assert!(apps.iter().any(|a| a.name == "Website"));
        assert_eq!(search(&apps, "  MAIL ").len(), 1);
        assert_eq!(search(&apps, "missing").len(), 0);
        assert_eq!(search(&apps, "").len(), 3);
    }

    #[test]
    fn catalog_is_sorted_and_drops_blank_names_and_other_extensions() {
        let apps = normalize([
            app("zeta", r"C:\a\z.lnk", None),
            app("Alpha", r"C:\a\a.lnk", None),
            app("  ", r"C:\a\blank.lnk", None),
            app("Tool", r"C:\a\tool.exe", None),
            app("same path", r"c:/a/A.LNK", None),
        ]);
        let names: Vec<_> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "zeta"]);
    }

    #[test]
    fn app_ids_are_validated_strictly() {
        assert!(valid_app_id("Vendor.App_abc!App"));
        for bad in [
            "",
            "!",
            "Vendor!",
            "!App",
            "Vendor!!App",
            "Vendor!App extra",
            "Vendor!App\"",
            "Vendor!App;extra",
        ] {
            assert!(!valid_app_id(bad), "{bad:?}");
            assert!(installed_app_argument(bad).is_err(), "{bad:?}");
        }
        assert!(!valid_app_id(&format!("a!{}", "b".repeat(300))));
        assert_eq!(
            installed_app_argument("Vendor.App_abc!App").unwrap(),
            "shell:AppsFolder\\Vendor.App_abc!App"
        );
    }

    #[test]
    fn installed_app_identity_round_trips_into_a_shortcut() {
        let shortcut = app(
            "Mail",
            "shell:AppsFolder\\Vendor.Mail_abc!App",
            Some("Vendor.Mail_abc!App"),
        )
        .to_shortcut();
        let json = serde_json::to_string(&shortcut).unwrap();
        let read: Shortcut = serde_json::from_str(&json).unwrap();
        assert_eq!(read.app_id.as_deref(), Some("Vendor.Mail_abc!App"));
        assert_eq!(read.target, shortcut.target);
        let old: Shortcut =
            serde_json::from_str(r#"{"Name":"Browser","Target":"https://example.com"}"#).unwrap();
        assert!(old.app_id.is_none() && old.name == "Browser");
    }

    #[test]
    fn icon_lookup_rejects_web_and_network_paths() {
        let windows = r"C:\Windows";
        let all_local = |_: &str| true;
        let id = |t: &str| Shortcut {
            name: "x".into(),
            target: t.into(),
            app_id: None,
        };
        for rejected in [
            "https://example.com/index.html",
            "file://server/share/app.exe",
            r"\\server\share\app.lnk",
            "relative.lnk",
        ] {
            assert_eq!(
                icon_source(&id(rejected), windows, |t| t.to_owned(), all_local),
                None,
                "{rejected}"
            );
        }
        assert_eq!(
            icon_source(
                &id(r"D:\Apps\Tool.lnk"),
                windows,
                |t| t.to_owned(),
                |_| false
            ),
            None,
            "a network drive is not local"
        );
        assert_eq!(
            icon_source(
                &id(r"D:\Apps\Tool.lnk"),
                windows,
                |t| t.to_owned(),
                all_local
            ),
            Some(IconSource::File(r"D:\Apps\Tool.lnk".into()))
        );
    }

    #[test]
    fn icon_sources_map_special_targets_and_app_ids() {
        let windows = r"C:\Windows\";
        let sc = |t: &str, id: Option<&str>| Shortcut {
            name: "x".into(),
            target: t.into(),
            app_id: id.map(Into::into),
        };
        let e = |t: &str| t.to_owned();
        assert_eq!(
            icon_source(&sc("Explorer.EXE", None), windows, e, |_| true),
            Some(IconSource::File(r"C:\Windows\explorer.exe".into()))
        );
        assert_eq!(
            icon_source(&sc("ms-settings:display", None), windows, e, |_| true),
            Some(IconSource::File(
                r"C:\Windows\ImmersiveControlPanel\SystemSettings.exe".into()
            ))
        );
        assert_eq!(
            icon_source(
                &sc("ignored", Some("Vendor.App_abc!App")),
                windows,
                e,
                |_| false
            ),
            Some(IconSource::AppsFolder("Vendor.App_abc!App".into()))
        );
        assert_eq!(
            icon_source(&sc("ignored", Some("bad")), windows, e, |_| true),
            None
        );
        assert_eq!(
            IconSource::AppsFolder("a!b".into()).parsing_name(),
            "shell:AppsFolder\\a!b"
        );
    }

    #[test]
    fn environment_variables_expand_like_dotnet() {
        let lookup = |n: &str| (n == "WINDIR").then(|| r"C:\Windows".to_owned());
        assert_eq!(expand_env(r"%WINDIR%\x.exe", lookup), r"C:\Windows\x.exe");
        assert_eq!(expand_env("%NOPE%\\x", lookup), "%NOPE%\\x");
        assert_eq!(expand_env("100%", lookup), "100%");
        assert_eq!(expand_env("a%%b", lookup), "a%%b");
    }

    #[test]
    fn start_menu_scan_finds_shortcuts_recursively_and_honors_cancel() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Notepad.lnk"), b"").unwrap();
        fs::write(dir.path().join("readme.txt"), b"").unwrap();
        fs::create_dir_all(dir.path().join("Programs").join("Dev")).unwrap();
        fs::write(
            dir.path().join("Programs").join("Dev").join("Editor.URL"),
            b"",
        )
        .unwrap();
        let mut found = Vec::new();
        scan_shortcuts(dir.path(), &mut found, &AtomicBool::new(false));
        let mut names: Vec<_> = found.iter().map(|a| a.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["Editor", "Notepad"]);
        assert!(found.iter().all(|a| a.app_id.is_none()));

        let mut none = Vec::new();
        scan_shortcuts(dir.path(), &mut none, &AtomicBool::new(true));
        assert!(none.is_empty());
        scan_shortcuts(
            &dir.path().join("missing"),
            &mut none,
            &AtomicBool::new(false),
        );
        assert!(none.is_empty(), "a missing folder is skipped, not an error");
    }

    #[cfg(unix)]
    #[test]
    fn start_menu_scan_skips_reparse_points() {
        let dir = tempfile::tempdir().unwrap();
        let real = tempfile::tempdir().unwrap();
        fs::write(real.path().join("Hidden.lnk"), b"").unwrap();
        std::os::unix::fs::symlink(real.path(), dir.path().join("link")).unwrap();
        let mut found = Vec::new();
        scan_shortcuts(dir.path(), &mut found, &AtomicBool::new(false));
        assert!(found.is_empty());
    }

    #[test]
    fn premultiplied_icon_pixels_become_straight_alpha() {
        // Half-transparent pure red, premultiplied BGRA = (0,0,128,128).
        let rgba = bgra_premultiplied_to_rgba(&[0, 0, 128, 128]);
        assert_eq!(rgba, vec![255, 0, 0, 128]);
        // No alpha anywhere: opaque.
        assert_eq!(
            bgra_premultiplied_to_rgba(&[10, 20, 30, 0]),
            vec![30, 20, 10, 255]
        );
        // Opaque stays.
        assert_eq!(
            bgra_premultiplied_to_rgba(&[1, 2, 3, 255, 0, 0, 0, 0]),
            vec![3, 2, 1, 255, 0, 0, 0, 0]
        );
    }
}
