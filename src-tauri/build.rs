use std::env;
use std::path::Path;

fn main() {
    embed_windows_manifest();

    // Tauri's own manifest is only linked into bins, so lib tests and examples would fail to
    // start with STATUS_ENTRYPOINT_NOT_FOUND (tauri#13419). We embed ours into every target.
    // Listing the commands makes each one need an explicit `allow-*` in the capabilities.
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
        .app_manifest(tauri_build::AppManifest::new().commands(&[
            "get_status",
            "get_pairing_info",
            "pairing_qr",
            "repair",
            "get_settings",
            "set_settings",
            "get_autostart",
            "set_autostart",
            "list_recent",
            "reveal_code",
            "copy_code",
            "send_test_code",
            "detect_app",
        ]));
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri-build failed: {error:#}");
    }
}

fn embed_windows_manifest() {
    let target_os = env::var("CARGO_CFG_TARGET_OS");
    let target_env = env::var("CARGO_CFG_TARGET_ENV");
    if target_os.as_deref() != Ok("windows") || target_env.as_deref() != Ok("msvc") {
        return;
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}
