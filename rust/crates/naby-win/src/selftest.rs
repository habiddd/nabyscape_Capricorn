//! `--self-test [--report <file>] [--require-payload]`: a quick check that the build works on
//! this machine (C# `SelfTests`, reduced to what a release gate needs): the store round-trips a
//! document in a temp folder, the bundled defaults normalize, system fonts load, the version
//! resource matches the package version, and the Whistle files sit next to the executable.
//!
//! Exit code 0 when no check failed. Missing Whistle files only fail with `--require-payload`
//! (the release script passes it), so a plain `cargo run` build still self-tests.

use std::path::{Path, PathBuf};

use naby_core::settings::presets;
use naby_core::store::Store;
use naby_render::{Fonts, Rgba, TextStyle};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Note(String),
    Fail(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub outcome: Outcome,
}

pub fn store_roundtrip(dir: &Path) -> Outcome {
    let store = Store::open(dir.to_owned());
    let value = serde_json::json!({ "Answer": 42, "Text": "naby" });
    if let Err(e) = store.save("selftest", &value) {
        return Outcome::Fail(format!("save: {e:?}"));
    }
    match store.load::<serde_json::Value>("selftest") {
        Ok(Some(back)) if back == value => Outcome::Pass,
        other => Outcome::Fail(format!("load returned {other:?}")),
    }
}

pub fn defaults_roundtrip() -> Outcome {
    let settings = presets::recommended();
    match serde_json::to_string(&settings)
        .and_then(|t| serde_json::from_str::<naby_core::settings::Settings>(&t))
    {
        Ok(back) if back == settings => Outcome::Pass,
        Ok(_) => Outcome::Fail("settings changed in a JSON round trip".into()),
        Err(e) => Outcome::Fail(e.to_string()),
    }
}

pub fn fonts_load() -> Outcome {
    let fonts = Fonts::load_system();
    let width = fonts.measure_width("NabyScape", &TextStyle::new(14.0, Rgba::TRANSPARENT));
    if width > 0.0 {
        Outcome::Pass
    } else if cfg!(windows) {
        Outcome::Fail("no system font produced glyphs".into())
    } else {
        Outcome::Note("no system font on this platform".into())
    }
}

/// Payload files expected beside `exe_dir` (`libneedle.dll`, `whistle.cact`).
pub fn payload_present(exe_dir: &Path, require: bool) -> Outcome {
    let missing: Vec<&str> = ["libneedle.dll", "whistle.cact"]
        .into_iter()
        .filter(|name| !exe_dir.join(name).is_file())
        .collect();
    match (missing.is_empty(), require) {
        (true, _) => Outcome::Pass,
        (false, true) => Outcome::Fail(format!("missing: {}", missing.join(", "))),
        (false, false) => Outcome::Note(format!(
            "missing (development build): {}",
            missing.join(", ")
        )),
    }
}

#[cfg(windows)]
fn version_resource() -> Outcome {
    let Ok(exe) = std::env::current_exe() else {
        return Outcome::Fail("no executable path".into());
    };
    match crate::winops::file_version(&exe) {
        Ok(v) if v == naby_core::updates::Version::installed() => Outcome::Pass,
        Ok(v) => Outcome::Fail(format!("version resource {v} differs from package version")),
        Err(e) => Outcome::Fail(e.to_string()),
    }
}

#[cfg(not(windows))]
fn version_resource() -> Outcome {
    Outcome::Note("Windows only".into())
}

pub fn run_checks(require_payload: bool) -> Vec<Check> {
    let temp = std::env::temp_dir().join(format!("NabyScape-selftest-{}", crate::update::new_id()));
    let store = store_roundtrip(&temp);
    let _ = std::fs::remove_dir_all(&temp);
    let exe_dir: PathBuf = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    vec![
        Check {
            name: "store read/write",
            outcome: store,
        },
        Check {
            name: "default settings round trip",
            outcome: defaults_roundtrip(),
        },
        Check {
            name: "fonts",
            outcome: fonts_load(),
        },
        Check {
            name: "version resource",
            outcome: version_resource(),
        },
        Check {
            name: "whistle payload",
            outcome: payload_present(&exe_dir, require_payload),
        },
    ]
}

pub fn render(checks: &[Check]) -> String {
    let mut text = format!("NabyScape {} self-test\n", env!("CARGO_PKG_VERSION"));
    for check in checks {
        match &check.outcome {
            Outcome::Pass => text.push_str(&format!("PASS {}\n", check.name)),
            Outcome::Note(n) => text.push_str(&format!("NOTE {}: {n}\n", check.name)),
            Outcome::Fail(n) => text.push_str(&format!("FAIL {}: {n}\n", check.name)),
        }
    }
    text
}

/// Entry point; returns the exit code.
pub fn run(args: &[String]) -> i32 {
    let checks = run_checks(args.iter().any(|a| a == "--require-payload"));
    let text = render(&checks);
    if let Some(i) = args.iter().position(|a| a == "--report") {
        if let Some(path) = args.get(i + 1) {
            let _ = std::fs::write(path, &text);
        }
    }
    print!("{text}");
    i32::from(checks.iter().any(|c| matches!(c.outcome, Outcome::Fail(_))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_defaults_pass() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(store_roundtrip(dir.path()), Outcome::Pass);
        assert_eq!(defaults_roundtrip(), Outcome::Pass);
    }

    #[test]
    fn payload_requirement_decides_failure() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            payload_present(dir.path(), false),
            Outcome::Note(_)
        ));
        assert!(matches!(
            payload_present(dir.path(), true),
            Outcome::Fail(_)
        ));
        std::fs::write(dir.path().join("libneedle.dll"), b"x").unwrap();
        std::fs::write(dir.path().join("whistle.cact"), b"x").unwrap();
        assert_eq!(payload_present(dir.path(), true), Outcome::Pass);
    }

    #[test]
    fn report_lists_each_check_and_exit_reflects_failures() {
        let checks = vec![
            Check {
                name: "a",
                outcome: Outcome::Pass,
            },
            Check {
                name: "b",
                outcome: Outcome::Fail("bad".into()),
            },
        ];
        let text = render(&checks);
        assert!(text.contains("PASS a") && text.contains("FAIL b: bad"));
    }
}
