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
    tauri_plugin::Builder::new(COMMANDS)
        .global_api_script_path("./js/bridge.js")
        .build();
}
