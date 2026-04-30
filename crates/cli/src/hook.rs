//! Claude Code hook management — auto-detect running Tauri connector.
//!
//! `tauri-connector hook install` writes a lightweight UserPromptSubmit hook
//! that checks for a running connector (via the PID file) and tells Claude
//! which MCP tools are available.  Zero output when the app isn't running.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

/// Marker string used to identify our hook entry in settings.
const HOOK_MARKER: &str = "tauri-connector-detect";

const HOOK_SCRIPT: &str = r#"#!/bin/bash
# tauri-connector auto-detect hook for Claude Code
# Detects a running Tauri app with tauri-plugin-connector and signals
# available MCP tools.  Exits silently when no app is running.

# Quick guard: is this a Tauri project?
[ -d "src-tauri" ] || [ -f "tauri.conf.json" ] || exit 0

# Search for .connector.json PID file
PID_FILE=""
for p in \
  src-tauri/target/.connector.json \
  target/.connector.json \
  ../src-tauri/target/.connector.json \
  ../target/.connector.json; do
  [ -f "$p" ] && PID_FILE="$p" && break
done
[ -n "$PID_FILE" ] || exit 0

# Portable JSON field extractor (works on macOS BSD sed + GNU sed)
field() { grep "\"$1\"" "$PID_FILE" | head -1 | sed 's/.*: *//; s/[",]//g; s/ *$//'; }

# Verify the process is still alive
PID=$(field pid)
[ -n "$PID" ] && kill -0 "$PID" 2>/dev/null || exit 0

MCP_PORT=$(field mcp_port)
WS_PORT=$(field ws_port)
APP_NAME=$(field app_name)

if [ "$MCP_PORT" = "null" ] || [ -z "$MCP_PORT" ]; then
  echo "[tauri-connector] '${APP_NAME:-Tauri App}' running — WS :${WS_PORT:-?}"
else
  echo "[tauri-connector] '${APP_NAME:-Tauri App}' running — MCP :${MCP_PORT} WS :${WS_PORT:-?}"
fi
echo "Tools: DOM snapshot, screenshot, JS eval, IPC debug, element interaction, drag-and-drop"
echo "Use /tauri-connector skill for guided debugging workflows"
"#;

fn hooks_dir() -> PathBuf {
    PathBuf::from(".claude").join("hooks")
}

fn script_path() -> PathBuf {
    hooks_dir().join("tauri-connector-detect.sh")
}

fn settings_path() -> PathBuf {
    PathBuf::from(".claude").join("settings.local.json")
}

/// Install the auto-detect hook into the current project.
pub fn install() -> Result<(), String> {
    let dir = hooks_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;

    // Write hook script
    let script = script_path();
    fs::write(&script, HOOK_SCRIPT)
        .map_err(|e| format!("Failed to write {}: {e}", script.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to chmod {}: {e}", script.display()))?;
    }

    // Read or create settings.local.json
    let settings_file = settings_path();
    let mut settings: Value = if settings_file.exists() {
        let content = fs::read_to_string(&settings_file)
            .map_err(|e| format!("Failed to read {}: {e}", settings_file.display()))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Invalid JSON in {}: {e}", settings_file.display()))?
    } else {
        if let Some(parent) = settings_file.parent() {
            fs::create_dir_all(parent).ok();
        }
        json!({})
    };

    let hook_entry = json!({
        "matcher": "",
        "command": format!("bash {}", script.display())
    });

    let hooks = settings
        .as_object_mut()
        .ok_or("Settings is not a JSON object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));

    let arr = hooks
        .as_object_mut()
        .ok_or("hooks is not a JSON object")?
        .entry("UserPromptSubmit")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or("UserPromptSubmit is not an array")?;

    // Idempotent: skip if already installed
    let already = arr.iter().any(|e| {
        e.get("command")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.contains(HOOK_MARKER))
    });
    if already {
        println!("Hook already installed.");
        return Ok(());
    }

    arr.push(hook_entry);

    let pretty = serde_json::to_string_pretty(&settings)
        .map_err(|e| format!("Failed to serialize settings: {e}"))?;
    fs::write(&settings_file, format!("{pretty}\n"))
        .map_err(|e| format!("Failed to write {}: {e}", settings_file.display()))?;

    println!("Installed tauri-connector auto-detect hook:");
    println!("  Script:   {}", script.display());
    println!("  Settings: {}", settings_file.display());
    println!();
    println!("On every prompt, the hook checks for a running Tauri app and");
    println!("signals available MCP tools.  Zero output when the app is off.");

    Ok(())
}

/// Remove the auto-detect hook from the current project.
pub fn remove() -> Result<(), String> {
    let mut removed = false;

    // Remove script file
    let script = script_path();
    if script.exists() {
        fs::remove_file(&script)
            .map_err(|e| format!("Failed to remove {}: {e}", script.display()))?;
        println!("Removed {}", script.display());
        removed = true;
    }

    // Remove entry from settings
    let settings_file = settings_path();
    if settings_file.exists() {
        let content = fs::read_to_string(&settings_file)
            .map_err(|e| format!("Failed to read {}: {e}", settings_file.display()))?;
        let mut settings: Value = serde_json::from_str(&content)
            .map_err(|e| format!("Invalid JSON in {}: {e}", settings_file.display()))?;

        if let Some(arr) = settings
            .pointer_mut("/hooks/UserPromptSubmit")
            .and_then(|v| v.as_array_mut())
        {
            let before = arr.len();
            arr.retain(|e| {
                !e.get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.contains(HOOK_MARKER))
            });

            if arr.len() < before {
                let pretty = serde_json::to_string_pretty(&settings)
                    .map_err(|e| format!("Failed to serialize: {e}"))?;
                fs::write(&settings_file, format!("{pretty}\n"))
                    .map_err(|e| format!("Failed to write {}: {e}", settings_file.display()))?;
                println!("Removed hook entry from {}", settings_file.display());
                removed = true;
            }
        }
    }

    if !removed {
        println!("No tauri-connector hook found to remove.");
    }

    Ok(())
}
