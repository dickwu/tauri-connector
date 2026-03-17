const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .global_api_script_path("./js/bridge.js")
        .build();
}
