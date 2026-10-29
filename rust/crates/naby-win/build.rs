//! Embeds the application icon (resource id 1) and the manifest (PerMonitorV2 DPI, Common
//! Controls 6, Windows 10 compatibility) into `nabyscape.exe`. Nothing happens for non-Windows
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
        "1 ICON \"{}\"\n1 24 \"{}\"\n",
        quote(&icon),
        quote(&manifest)
    );
    std::fs::write(&rc_path, rc).expect("write rc");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("compile Windows resources");
}
