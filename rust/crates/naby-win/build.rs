//! Embeds the application icon (resource id 1), the version resource (FileVersion and
//! ProductVersion = the workspace version, read by the updater's setup inspection) and the
//! manifest (PerMonitorV2 DPI, Common Controls 6, Windows 10 compatibility) into `nabyscape.exe`. Nothing happens for non-Windows
//! targets, so the Linux test and clippy runs stay toolchain-free.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/nabyscape.manifest");
    println!("cargo:rerun-if-changed=../../../src/NabyScape/Assets/island.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("../../../src/NabyScape/Assets/island.ico");
    let manifest = manifest_dir.join("resources/nabyscape.manifest");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let rc_path = out.join("nabyscape.rc");
    let quote = |p: &std::path::Path| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|e| panic!("missing resource {}: {e}", p.display()))
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('\\', "/")
    };
    let rc = format!(
        "1 ICON \"{}\"\n1 24 \"{}\"\n{}",
        quote(&icon),
        quote(&manifest),
        version_resource(&std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION"))
    );
    std::fs::write(&rc_path, rc).expect("write rc");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("compile Windows resources");
}

/// The `VERSIONINFO` resource for `version` (`major.minor.patch`). Numeric constants instead of
/// `VS_*` names so no Windows header is needed by `windres`.
fn version_resource(version: &str) -> String {
    let mut parts = version
        .split(['.', '-', '+'])
        .map(|p| p.parse::<u16>().unwrap_or(0));
    let (major, minor, patch) = (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    );
    let numeric = format!("{major},{minor},{patch},0");
    let text = format!("{major}.{minor}.{patch}.0");
    format!(
        r#"1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3f
FILEFLAGS 0
FILEOS 0x40004
FILETYPE 0x1
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "NabyScape"
      VALUE "FileDescription", "NabyScape"
      VALUE "FileVersion", "{text}"
      VALUE "InternalName", "NabyScape"
      VALUE "LegalCopyright", "Copyright NabyScape contributors. MIT License."
      VALUE "OriginalFilename", "NabyScape.exe"
      VALUE "ProductName", "NabyScape"
      VALUE "ProductVersion", "{major}.{minor}.{patch}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}
