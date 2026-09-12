// Explicit native callback permissions; no wildcard or business command grant.
const COMMANDS: &[&str] = &[
    "push_dom",
    "push_logs",
    "set_pointed_element",
    "push_ipc_event",
    "push_capture_events",
    "capture_self_test",
    "push_event",
    "push_runtime",
];

fn main() {
    // Tauri enables Common Controls v6, but tauri-plugin's builder does not
    // embed an application manifest into this crate's Windows test harness.
    // Without activation, the loader selects comctl32 v5 and can fail before
    // any test runs (STATUS_ENTRYPOINT_NOT_FOUND). Match Tauri's own test fix:
    // https://github.com/tauri-apps/tauri/issues/11028
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets the package directory"),
        )
        .join("windows-test-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        // These arguments apply to this package's link targets, including its
        // lib-test executable; they are not propagated as consumer rustflags.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
    }

    tauri_plugin::Builder::new(COMMANDS)
        .global_api_script_path("./js/bridge.js")
        .build();
}
