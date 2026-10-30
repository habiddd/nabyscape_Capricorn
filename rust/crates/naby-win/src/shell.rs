//! Launching things the way the shell does (C# `ShortcutService`, `Process.Start` with
//! `UseShellExecute`): URLs, files, folders, `ms-settings:` pages, installed apps by
//! AppUserModelID, and "show in Explorer".
//!
//! `resolve_shortcut` is the pure validation step; `open_*` / `launch` (Windows only) execute.

use std::fmt;

/// A user-facing launch failure; the text is shown in a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellError(pub String);

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ShellError {}

/// Non-file URL schemes a shortcut may open.
pub const ALLOWED_SCHEMES: [&str; 5] = ["http", "https", "ms-settings", "spotify", "mailto"];

/// What to start for a shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// An installed (Store or desktop) app; run `explorer.exe shell:AppsFolder\<id>`.
    AppsFolder(String),
    /// Anything `ShellExecute("open")` understands: path, folder, URL, program name.
    Open(String),
}

/// C# `WindowsAppCatalog.ValidAppId`: `PackageFamily!AppId`, ASCII letters, digits, `. _ - !`.
pub fn valid_app_id(id: &str) -> bool {
    !id.trim().is_empty()
        && id.len() <= 256
        && id.find('!').is_some_and(|i| i > 0)
        && id.find('!') == id.rfind('!')
        && !id.ends_with('!')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '!'))
}

/// The scheme of an absolute URI like `https://x` or `ms-settings:display`; `None` for plain
/// words, relative paths and drive paths (`C:\` has a one-letter "scheme").
pub fn url_scheme(text: &str) -> Option<String> {
    let colon = text.find(':')?;
    let scheme = &text[..colon];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if scheme.len() < 2
        || !first.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

fn is_rooted(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with(['\\', '/']) || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// Validates a stored shortcut (C# `ShortcutService.Open` before `Process.Start`).
/// `expand` expands `%VARS%`; `exists` tells whether a file or folder exists.
pub fn resolve_shortcut(
    target: &str,
    app_id: Option<&str>,
    expand: &dyn Fn(&str) -> String,
    exists: &dyn Fn(&str) -> bool,
) -> Result<Launch, ShellError> {
    if let Some(id) = app_id {
        if !valid_app_id(id) {
            return Err(ShellError(
                "This installed app shortcut is invalid. Remove it and add the app again in Settings."
                    .into(),
            ));
        }
        return Ok(Launch::AppsFolder(id.to_string()));
    }
    let target = expand(target.trim());
    if target.trim().is_empty() {
        return Err(ShellError("The shortcut destination is empty.".into()));
    }
    match url_scheme(&target) {
        Some(scheme) if scheme != "file" => {
            if !ALLOWED_SCHEMES.contains(&scheme.as_str()) {
                return Err(ShellError(
                    "Use a file, folder, application, or http/https URL.".into(),
                ));
            }
        }
        _ => {
            if (is_rooted(&target) || target.contains('\\')) && !exists(&target) {
                return Err(ShellError(
                    "The shortcut file or folder could not be found.".into(),
                ));
            }
        }
    }
    Ok(Launch::Open(target))
}

/// Arguments for `explorer.exe` that select a file in its folder.
pub fn reveal_args(path: &str) -> [String; 2] {
    ["/select,".to_string(), path.to_string()]
}

/// Argument for `explorer.exe` that starts an installed app.
pub fn apps_folder_arg(app_id: &str) -> String {
    format!("shell:AppsFolder\\{app_id}")
}

#[cfg(windows)]
pub use os::*;

#[cfg(windows)]
mod os {
    use super::*;
    use crate::util::{expand_env, wide};
    use std::os::windows::process::CommandExt;
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHParseDisplayName, ShellExecuteW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// `ShellExecuteW(..., "open", target)`; fails with Windows' own error when it refuses.
    pub fn shell_open(target: &str) -> Result<(), ShellError> {
        let file = wide(target);
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // Values above 32 mean success (documented ShellExecute contract).
        if result.0 as isize > 32 {
            Ok(())
        } else {
            Err(ShellError(format!(
                "Windows could not open this shortcut (error {}).",
                result.0 as isize
            )))
        }
    }

    pub fn open_url(url: &str) -> Result<(), ShellError> {
        match url_scheme(url).as_deref() {
            Some(s) if ALLOWED_SCHEMES.contains(&s) => shell_open(url),
            _ => Err(ShellError(
                "Use a file, folder, application, or http/https URL.".into(),
            )),
        }
    }

    pub fn open_path(path: &str) -> Result<(), ShellError> {
        shell_open(path)
    }

    pub fn open_folder(path: &str) -> Result<(), ShellError> {
        shell_open(path)
    }

    fn explorer() -> std::process::Command {
        let windows = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        std::process::Command::new(std::path::Path::new(&windows).join("explorer.exe"))
    }

    /// Opens Explorer with `path` selected.
    pub fn reveal_in_explorer(path: &str) -> Result<(), ShellError> {
        if !std::path::Path::new(path).exists() {
            return Err(ShellError("The file could not be found.".into()));
        }
        let [flag, file] = reveal_args(path);
        explorer()
            .arg(flag)
            .arg(file)
            .spawn()
            .map(|_| ())
            .map_err(|e| ShellError(e.to_string()))
    }

    /// Whether the shell knows an installed app with this AppUserModelID.
    pub fn app_installed(app_id: &str) -> bool {
        let name = wide(&apps_folder_arg(app_id));
        let mut pidl = std::ptr::null_mut();
        // SAFETY: `name` is NUL-terminated; a returned pidl is freed with CoTaskMemFree.
        let parsed = unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None) };
        if parsed.is_ok() && !pidl.is_null() {
            // SAFETY: frees the item id list allocated by SHParseDisplayName.
            unsafe { CoTaskMemFree(Some(pidl as *const _)) };
            true
        } else {
            false
        }
    }

    fn start_app(app_id: &str) -> Result<(), ShellError> {
        if !app_installed(app_id) {
            return Err(ShellError(
                "This app is no longer installed. Remove the shortcut and choose an installed app in Settings."
                    .into(),
            ));
        }
        explorer()
            .arg(apps_folder_arg(app_id))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
            .map_err(|e| ShellError(e.to_string()))
    }

    /// Opens a settings shortcut (C# `ShortcutService.Open`).
    pub fn launch(target: &str, app_id: Option<&str>) -> Result<(), ShellError> {
        let expand = |s: &str| expand_env(s, &|name| std::env::var(name).ok());
        let exists = |p: &str| std::path::Path::new(p).exists();
        match resolve_shortcut(target, app_id, &expand, &exists)? {
            Launch::AppsFolder(id) => start_app(&id),
            Launch::Open(t) => shell_open(&t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(target: &str, app: Option<&str>, exists: bool) -> Result<Launch, ShellError> {
        resolve_shortcut(target, app, &|s| s.to_string(), &|_| exists)
    }

    #[test]
    fn app_ids_need_exactly_one_bang_between_safe_parts() {
        assert!(valid_app_id(
            "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"
        ));
        for bad in [
            "",
            " ",
            "NoBang",
            "!App",
            "Pkg!",
            "A!B!C",
            "Pkg!A pp",
            "Pkg!A\\b",
            "P\u{e9}!A",
        ] {
            assert!(!valid_app_id(bad), "{bad:?}");
        }
        assert!(!valid_app_id(&format!("{}!A", "x".repeat(260))));
    }

    #[test]
    fn schemes_ignore_drive_letters_and_plain_words() {
        assert_eq!(url_scheme("https://x.y").as_deref(), Some("https"));
        assert_eq!(
            url_scheme("MS-Settings:display").as_deref(),
            Some("ms-settings")
        );
        assert_eq!(url_scheme(r"C:\Windows"), None);
        assert_eq!(url_scheme("notepad"), None);
        assert_eq!(url_scheme("1abc:x"), None);
        assert_eq!(url_scheme(""), None);
    }

    #[test]
    fn allowed_urls_pass_and_others_are_refused() {
        for url in [
            "https://example.com",
            "http://example.com/a?b=1",
            "ms-settings:network",
            "spotify:track:1",
            "mailto:a@b.c",
        ] {
            assert_eq!(
                resolve(url, None, false),
                Ok(Launch::Open(url.into())),
                "{url}"
            );
        }
        for url in ["javascript:alert(1)", "ftp://x", "vbscript:x", "ssh://h"] {
            assert_eq!(
                resolve(url, None, true),
                Err(ShellError(
                    "Use a file, folder, application, or http/https URL.".into()
                )),
                "{url}"
            );
        }
    }

    #[test]
    fn missing_rooted_paths_fail_but_program_names_pass() {
        let missing = Err(ShellError(
            "The shortcut file or folder could not be found.".into(),
        ));
        assert_eq!(resolve(r"C:\nope\a.exe", None, false), missing);
        assert_eq!(resolve(r"\\server\share\f", None, false), missing);
        assert_eq!(resolve(r"sub\a.exe", None, false), missing);
        assert_eq!(
            resolve(r"C:\there.txt", None, true),
            Ok(Launch::Open(r"C:\there.txt".into()))
        );
        assert_eq!(
            resolve("notepad", None, false),
            Ok(Launch::Open("notepad".into()))
        );
        assert_eq!(
            resolve("file:///C:/x.txt", None, false),
            Ok(Launch::Open("file:///C:/x.txt".into()))
        );
    }

    #[test]
    fn empty_targets_and_variables() {
        assert_eq!(
            resolve("   ", None, true),
            Err(ShellError("The shortcut destination is empty.".into()))
        );
        let r = resolve_shortcut(
            "%TOOLS%\\a.exe",
            None,
            &|s| s.replace("%TOOLS%", r"D:\tools"),
            &|p| p == r"D:\tools\a.exe",
        );
        assert_eq!(r, Ok(Launch::Open(r"D:\tools\a.exe".into())));
    }

    #[test]
    fn installed_apps_validate_the_id_first() {
        assert_eq!(
            resolve("ignored", Some("Pkg_abc!App"), false),
            Ok(Launch::AppsFolder("Pkg_abc!App".into()))
        );
        assert!(resolve("x", Some("bad"), true).is_err());
        assert_eq!(apps_folder_arg("Pkg!App"), "shell:AppsFolder\\Pkg!App");
        assert_eq!(
            reveal_args(r"C:\a b\c.txt"),
            ["/select,".to_string(), r"C:\a b\c.txt".to_string()]
        );
    }
}
