//! `tauri-connector doctor` — diagnose the current project's setup.
//!
//! Walks the working directory, inspects Tauri plugin files, the frontend
//! `package.json`, `.mcp.json`, and the runtime `.connector.json` PID file,
//! then probes live connectivity to the WebSocket and MCP ports. Anything
//! missing is reported with a concrete fix instruction. Exits non-zero when
//! one or more required checks fail so it is CI-friendly.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use connector_client::ConnectorClient;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const SYM_OK: &str = "✓";
const SYM_FAIL: &str = "✗";
const SYM_WARN: &str = "!";

const DEFAULT_WS_PORT: u16 = 9555;
const DEFAULT_MCP_PORT: u16 = 9556;
const PROBE_HOST: &str = "127.0.0.1";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Fail,
    Warn,
}

impl Status {
    fn symbol(self) -> &'static str {
        match self {
            Status::Ok => SYM_OK,
            Status::Fail => SYM_FAIL,
            Status::Warn => SYM_WARN,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Fail => "fail",
            Status::Warn => "warn",
        }
    }
}

struct Check {
    label: String,
    status: Status,
    detail: Option<String>,
    fix: Option<String>,
}

impl Check {
    fn ok(label: impl Into<String>) -> Self {
        Self { label: label.into(), status: Status::Ok, detail: None, fix: None }
    }
    fn fail(label: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { label: label.into(), status: Status::Fail, detail: None, fix: Some(fix.into()) }
    }
    fn warn(label: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { label: label.into(), status: Status::Warn, detail: None, fix: Some(fix.into()) }
    }
    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

struct Section {
    name: &'static str,
    checks: Vec<Check>,
}

/// Options for the doctor command.
pub struct Options {
    /// Emit machine-readable JSON instead of the text checklist.
    pub json: bool,
    /// Skip live WebSocket/MCP probes (offline/CI mode).
    pub no_runtime: bool,
}

/// Run the doctor. Returns `Err` when one or more FAIL checks were reported so
/// the process exits non-zero; warnings and skips do not fail the run.
pub async fn run(opts: Options) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| format!("Cannot read cwd: {e}"))?;
    let project_root = find_project_root(&cwd);

    let mut sections = Vec::new();

    sections.push(check_environment(&cwd, project_root.as_ref()));

    if let Some(root) = project_root.as_ref() {
        sections.push(check_plugin_setup(root));
    }

    if !opts.no_runtime {
        let pid_info = project_root
            .as_ref()
            .and_then(|r| find_pid_file(r))
            .and_then(|p| read_pid_file(&p).map(|info| (p, info)));
        sections.push(check_runtime(project_root.as_ref(), pid_info).await);
    }

    if let Some(root) = project_root.as_ref() {
        sections.push(check_integration(root));
    }

    if opts.json {
        print_json(&sections);
    } else {
        print_text(&sections);
    }

    if any_fail(&sections) {
        Err("doctor found problems — see the Fix lines above".to_string())
    } else {
        Ok(())
    }
}

// ----- project detection ---------------------------------------------------

/// Walk up from `start` looking for a Tauri project root (a directory
/// containing `src-tauri/tauri.conf.json`). Returns `None` for non-Tauri dirs.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur: PathBuf = start.to_path_buf();
    for _ in 0..5 {
        if cur.join("src-tauri").join("tauri.conf.json").is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            break;
        }
    }
    // Some projects keep tauri.conf.json at the root (rare, older layout).
    if start.join("tauri.conf.json").is_file() {
        return Some(start.to_path_buf());
    }
    None
}

// ----- section: environment ------------------------------------------------

fn check_environment(cwd: &Path, project: Option<&PathBuf>) -> Section {
    let mut checks = Vec::new();

    checks.push(Check::ok(format!("CLI version {CURRENT_VERSION}")));
    checks.push(Check::ok(format!("Working directory {}", cwd.display())));

    match project {
        Some(root) => checks.push(
            Check::ok("Tauri v2 project detected").with_detail(format!(
                "{}",
                root.join("src-tauri").join("tauri.conf.json").display()
            )),
        ),
        None => checks.push(
            Check::warn(
                "No Tauri project found near the working directory",
                "run `tauri-connector doctor` from the directory that contains `src-tauri/`",
            )
            .with_detail(
                "setup/runtime checks skipped — open a Tauri v2 project first".to_string(),
            ),
        ),
    }

    Section { name: "Environment", checks }
}

// ----- section: plugin setup -----------------------------------------------

fn check_plugin_setup(root: &Path) -> Section {
    let checks = vec![
        check_cargo_dependency(root),
        check_plugin_registration(root),
        check_capabilities(root),
        check_with_global_tauri(root),
        check_snapdom(root),
        check_mcp_json(root),
    ];
    Section { name: "Plugin Setup", checks }
}

/// `src-tauri/Cargo.toml` must depend on `tauri-plugin-connector`.
fn check_cargo_dependency(root: &Path) -> Check {
    let path = root.join("src-tauri").join("Cargo.toml");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::fail(
                format!("Cargo dependency missing ({display} unreadable)"),
                "ensure src-tauri/Cargo.toml exists and is readable",
            )
        }
    };

    if let Some(version) = extract_cargo_version(&text, "tauri-plugin-connector") {
        Check::ok(format!(
            "Cargo dependency: tauri-plugin-connector = \"{version}\""
        ))
        .with_detail(display)
    } else {
        Check::fail(
            "Cargo dependency `tauri-plugin-connector` is missing",
            format!("add `tauri-plugin-connector = \"0.8\"` under [dependencies] in {display}"),
        )
    }
}

/// Parse a Cargo.toml entry for `name`. Accepts both `name = "x.y"` and
/// `name = { version = "x.y", ... }` forms. Returns the version string if found.
fn extract_cargo_version(text: &str, name: &str) -> Option<String> {
    let mut in_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            // Accept any `dependencies`-flavored table: [dependencies],
            // [dev-dependencies], [build-dependencies], [target.*.dependencies].
            in_section = line.contains("dependencies");
            continue;
        }
        if !in_section {
            continue;
        }

        // Match "name = ..." (tolerate spaces).
        let Some(rest) = line.strip_prefix(name) else { continue };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else { continue };
        let rest = rest.trim_start();

        // Inline table: { version = "...", ... }
        if rest.starts_with('{') {
            if let Some(v) = extract_quoted_field(rest, "version") {
                return Some(v);
            }
            // Version could be implied by path/git; treat as "present (unversioned)".
            return Some("path|git".to_string());
        }

        // Plain string: "x.y"
        if let Some(inner) = rest.strip_prefix('"') {
            if let Some(end) = inner.find('"') {
                return Some(inner[..end].to_string());
            }
        }
    }
    None
}

/// Return the value of `key = "..."` inside an inline table string.
fn extract_quoted_field(inline: &str, key: &str) -> Option<String> {
    let needle = format!("{key} =");
    let idx = inline.find(&needle)?;
    let tail = &inline[idx + needle.len()..];
    let tail = tail.trim_start();
    let tail = tail.strip_prefix('"')?;
    let end = tail.find('"')?;
    Some(tail[..end].to_string())
}

/// Plugin must be registered via `tauri_plugin_connector::init()` or
/// `ConnectorBuilder::new()...build()` in lib.rs or main.rs.
fn check_plugin_registration(root: &Path) -> Check {
    let candidates = [
        root.join("src-tauri").join("src").join("lib.rs"),
        root.join("src-tauri").join("src").join("main.rs"),
    ];

    for path in &candidates {
        let Ok(text) = fs::read_to_string(path) else { continue };
        let mentions_init = text.contains("tauri_plugin_connector::init")
            || text.contains("tauri_plugin_connector::ConnectorBuilder")
            || (text.contains("tauri_plugin_connector") && text.contains("ConnectorBuilder"));
        if mentions_init {
            return Check::ok(format!("Plugin registered in {}", rel(root, path)));
        }
    }

    Check::fail(
        "Plugin not registered",
        "add `#[cfg(debug_assertions)] { builder = builder.plugin(tauri_plugin_connector::init()); }` \
         in src-tauri/src/lib.rs before `.invoke_handler(...)`",
    )
}

/// At least one JSON file under `src-tauri/capabilities/` must list
/// `"connector:default"` in its `permissions` array.
fn check_capabilities(root: &Path) -> Check {
    let dir = root.join("src-tauri").join("capabilities");
    if !dir.is_dir() {
        return Check::fail(
            "Capabilities directory missing",
            "create src-tauri/capabilities/default.json and add `\"connector:default\"` to its permissions",
        );
    }

    let Ok(entries) = fs::read_dir(&dir) else {
        return Check::fail(
            "Cannot read src-tauri/capabilities/",
            "check filesystem permissions for the capabilities directory",
        );
    };

    let mut found_in: Option<PathBuf> = None;
    let mut checked_any = false;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_json = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("json"))
            .unwrap_or(false);
        if !is_json {
            continue;
        }
        checked_any = true;
        let Ok(text) = fs::read_to_string(&path) else { continue };
        let Ok(value) = serde_json::from_str::<Value>(&text) else { continue };
        if permissions_contain(&value, "connector:default") {
            found_in = Some(path);
            break;
        }
    }

    match found_in {
        Some(p) => Check::ok(format!("Permission \"connector:default\" in {}", rel(root, &p))),
        None if !checked_any => Check::fail(
            "No capability JSON files in src-tauri/capabilities/",
            "create default.json with `{ \"permissions\": [\"connector:default\"] }`",
        ),
        None => Check::fail(
            "Permission `connector:default` missing",
            format!(
                "add \"connector:default\" to the `permissions` array in a file under {}",
                rel(root, &dir)
            ),
        ),
    }
}

/// Walk a Tauri capability JSON and check whether `needle` appears in any
/// `permissions` array (supports both string and `{identifier: "..."}` items).
fn permissions_contain(value: &Value, needle: &str) -> bool {
    let Some(arr) = value.get("permissions").and_then(|v| v.as_array()) else {
        return false;
    };
    arr.iter().any(|p| match p {
        Value::String(s) => s == needle,
        Value::Object(obj) => obj
            .get("identifier")
            .and_then(|v| v.as_str())
            .map(|s| s == needle)
            .unwrap_or(false),
        _ => false,
    })
}

/// `src-tauri/tauri.conf.json` must have `app.withGlobalTauri: true`.
fn check_with_global_tauri(root: &Path) -> Check {
    let path = root.join("src-tauri").join("tauri.conf.json");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::fail(
                "tauri.conf.json unreadable",
                format!("make sure {display} exists and is valid JSON"),
            )
        }
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Check::fail(
            "tauri.conf.json is not valid JSON",
            format!("fix the JSON in {display}"),
        );
    };

    let with_global = value
        .pointer("/app/withGlobalTauri")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if with_global {
        Check::ok("app.withGlobalTauri: true").with_detail(display)
    } else {
        Check::fail(
            "app.withGlobalTauri is not true",
            format!("set `\"withGlobalTauri\": true` under `\"app\"` in {display} (required for the eval+event fallback)"),
        )
    }
}

/// Look for `@zumer/snapdom` in the nearest `package.json`.
fn check_snapdom(root: &Path) -> Check {
    let path = root.join("package.json");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::warn(
                "package.json not found at project root",
                "if your frontend lives in a subdirectory, install @zumer/snapdom there (`npm install @zumer/snapdom`)",
            )
        }
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Check::warn(
            "package.json is not valid JSON",
            format!("fix the JSON in {display}"),
        );
    };

    let has = package_has_dep(&value, "@zumer/snapdom");
    if has {
        Check::ok("Frontend dependency: @zumer/snapdom").with_detail(display)
    } else {
        Check::fail(
            "Frontend dependency `@zumer/snapdom` is missing",
            "run `npm install @zumer/snapdom` (or `bun add @zumer/snapdom`) — needed for the screenshot fallback",
        )
    }
}

fn package_has_dep(value: &Value, name: &str) -> bool {
    const KEYS: &[&str] = &[
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ];
    KEYS.iter().any(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_object())
            .map(|obj| obj.contains_key(name))
            .unwrap_or(false)
    })
}

/// `.mcp.json` at the project root should register `tauri-connector`.
fn check_mcp_json(root: &Path) -> Check {
    let path = root.join(".mcp.json");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::fail(
                ".mcp.json missing",
                format!(
                    "create {display} with:\n         {}",
                    indent_block(
                        r#"{"mcpServers":{"tauri-connector":{"url":"http://127.0.0.1:9556/sse"}}}"#,
                    )
                ),
            );
        }
    };

    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Check::fail(
            ".mcp.json is not valid JSON",
            format!("fix the JSON in {display}"),
        );
    };

    let entry = value.pointer("/mcpServers/tauri-connector");
    match entry {
        Some(Value::Object(obj)) => {
            let url = obj.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() {
                Check::warn(
                    ".mcp.json `tauri-connector` entry has no `url`",
                    "set `\"url\": \"http://127.0.0.1:9556/sse\"`",
                )
            } else {
                Check::ok(format!(".mcp.json registers tauri-connector ({url})"))
                    .with_detail(display)
            }
        }
        _ => Check::fail(
            ".mcp.json has no `tauri-connector` entry",
            format!(
                "add `\"tauri-connector\": {{ \"url\": \"http://127.0.0.1:9556/sse\" }}` to `mcpServers` in {display}"
            ),
        ),
    }
}

// ----- section: runtime ----------------------------------------------------

struct PidInfo {
    pid: Option<u64>,
    ws_port: Option<u16>,
    mcp_port: Option<u16>,
    app_name: Option<String>,
    app_id: Option<String>,
}

fn find_pid_file(root: &Path) -> Option<PathBuf> {
    // The plugin writes the PID file next to the dev binary, so it moves between
    // `target/` and `target/debug|release/`. Scan common spots relative to the project.
    let candidates = [
        root.join("src-tauri").join("target").join("debug").join(".connector.json"),
        root.join("src-tauri").join("target").join(".connector.json"),
        root.join("target").join("debug").join(".connector.json"),
        root.join("target").join(".connector.json"),
        root.join("src-tauri").join("target").join("release").join(".connector.json"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn read_pid_file(path: &Path) -> Option<PidInfo> {
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some(PidInfo {
        pid: value.get("pid").and_then(|v| v.as_u64()),
        ws_port: value
            .get("ws_port")
            .and_then(|v| v.as_u64())
            .and_then(|n| u16::try_from(n).ok()),
        mcp_port: value
            .get("mcp_port")
            .and_then(|v| v.as_u64())
            .and_then(|n| u16::try_from(n).ok()),
        app_name: value
            .get("app_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        app_id: value
            .get("app_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

async fn check_runtime(
    project: Option<&PathBuf>,
    pid: Option<(PathBuf, PidInfo)>,
) -> Section {
    let mut checks = Vec::new();

    // PID file presence
    let (ws_port, mcp_port) = match (&pid, project) {
        (Some((path, info)), Some(root)) => {
            let mut detail = format!("app: {}", info.app_name.clone().unwrap_or_else(|| "?".into()));
            if let Some(id) = &info.app_id {
                detail.push_str(&format!(" ({id})"));
            }
            if let Some(pid) = info.pid {
                detail.push_str(&format!(", pid {pid}"));
            }
            checks.push(
                Check::ok(format!("PID file: {}", rel(root, path))).with_detail(detail),
            );
            (
                info.ws_port.unwrap_or(DEFAULT_WS_PORT),
                info.mcp_port.unwrap_or(DEFAULT_MCP_PORT),
            )
        }
        (Some((path, info)), None) => {
            checks.push(
                Check::ok(format!("PID file: {}", path.display()))
                    .with_detail(format!("app: {}", info.app_name.clone().unwrap_or_else(|| "?".into()))),
            );
            (
                info.ws_port.unwrap_or(DEFAULT_WS_PORT),
                info.mcp_port.unwrap_or(DEFAULT_MCP_PORT),
            )
        }
        (None, _) => {
            checks.push(Check::warn(
                "PID file (.connector.json) not found",
                "start the Tauri app in dev mode (`bun run tauri dev`) — the plugin writes the PID file at startup",
            ));
            (DEFAULT_WS_PORT, DEFAULT_MCP_PORT)
        }
    };

    // WebSocket reachable
    match probe_ws(PROBE_HOST, ws_port).await {
        Ok(()) => checks.push(
            Check::ok(format!("WebSocket ws://{PROBE_HOST}:{ws_port} reachable")),
        ),
        Err(e) => checks.push(
            Check::fail(
                format!("WebSocket ws://{PROBE_HOST}:{ws_port} unreachable"),
                "start the Tauri app in dev mode so the plugin binds its WebSocket listener",
            )
            .with_detail(e),
        ),
    }

    // MCP reachable (TCP probe — avoids blocking on SSE semantics)
    match probe_tcp(PROBE_HOST, mcp_port).await {
        Ok(()) => checks.push(
            Check::ok(format!(
                "MCP server http://{PROBE_HOST}:{mcp_port}/sse reachable"
            )),
        ),
        Err(e) => checks.push(
            Check::fail(
                format!("MCP server http://{PROBE_HOST}:{mcp_port}/sse unreachable"),
                "confirm the embedded MCP server is enabled (it is on by default) and `.mcp.json` points at the same port",
            )
            .with_detail(e),
        ),
    }

    Section { name: "Runtime", checks }
}

async fn probe_ws(host: &str, port: u16) -> Result<(), String> {
    let mut client = ConnectorClient::new();
    let connect = client.connect(host, port);
    tokio::time::timeout(Duration::from_secs(2), connect)
        .await
        .map_err(|_| "connect timed out after 2s".to_string())??;

    let ping = client.send_with_timeout(json!({ "type": "ping" }), 2_000);
    let _ = ping.await.map_err(|e| format!("ping failed: {e}"))?;
    client.disconnect().await;
    Ok(())
}

async fn probe_tcp(host: &str, port: u16) -> Result<(), String> {
    let addr = (host, port);
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .map_err(|_| "connect timed out after 2s".to_string())?
    .map_err(|e| format!("tcp connect failed: {e}"))?;
    Ok(())
}

// ----- section: integration ------------------------------------------------

fn check_integration(root: &Path) -> Section {
    let mut checks = Vec::new();

    let hook_script = root.join(".claude").join("hooks").join("tauri-connector-detect.sh");
    let settings = root.join(".claude").join("settings.local.json");

    let script_exists = hook_script.is_file();
    let settings_has_hook = fs::read_to_string(&settings)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .as_ref()
        .map(settings_has_connector_hook)
        .unwrap_or(false);

    match (script_exists, settings_has_hook) {
        (true, true) => checks.push(
            Check::ok("Claude Code auto-detect hook installed")
                .with_detail(rel(root, &hook_script)),
        ),
        (true, false) => checks.push(Check::warn(
            "Hook script present but not wired into settings.local.json",
            "run `tauri-connector hook install` to finish wiring the hook",
        )),
        (false, true) => checks.push(Check::warn(
            "Hook entry in settings.local.json but script is missing",
            "run `tauri-connector hook install` to regenerate the script",
        )),
        (false, false) => checks.push(Check::warn(
            "Claude Code auto-detect hook not installed (optional)",
            "run `tauri-connector hook install` to enable per-prompt detection",
        )),
    }

    Section { name: "Integration", checks }
}

fn settings_has_connector_hook(settings: &Value) -> bool {
    let Some(arr) = settings.pointer("/hooks/UserPromptSubmit").and_then(|v| v.as_array()) else {
        return false;
    };
    arr.iter().any(|entry| {
        entry
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.contains("tauri-connector-detect"))
    })
}

// ----- rendering -----------------------------------------------------------

fn print_text(sections: &[Section]) {
    println!("tauri-connector doctor v{CURRENT_VERSION}");
    println!("{}", "=".repeat(29 + CURRENT_VERSION.len()));
    println!();

    for section in sections {
        println!("{}", section.name);
        for c in &section.checks {
            println!("  {} {}", c.status.symbol(), c.label);
            if let Some(detail) = &c.detail {
                for line in detail.lines() {
                    println!("      {line}");
                }
            }
            if let Some(fix) = &c.fix {
                for (i, line) in fix.lines().enumerate() {
                    if i == 0 {
                        println!("      Fix: {line}");
                    } else {
                        println!("           {line}");
                    }
                }
            }
        }
        println!();
    }

    let (ok, fail, warn) = tally(sections);
    let summary = format!("Summary: {ok} ok · {warn} warn · {fail} fail");
    println!("{summary}");
    if fail > 0 {
        println!("Run the `Fix` commands above and re-run `tauri-connector doctor`.");
    } else if warn > 0 {
        println!("Warnings are non-blocking; address them when convenient.");
    } else {
        println!("All checks passed — you're good to go.");
    }
}

fn print_json(sections: &[Section]) {
    let (ok, fail, warn) = tally(sections);
    let payload = json!({
        "cli_version": CURRENT_VERSION,
        "summary": { "ok": ok, "fail": fail, "warn": warn, "passed": fail == 0 },
        "sections": sections.iter().map(|s| json!({
            "name": s.name,
            "checks": s.checks.iter().map(|c| json!({
                "label": c.label,
                "status": c.status.as_str(),
                "detail": c.detail,
                "fix": c.fix,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("Failed to serialize doctor JSON: {e}"),
    }
}

fn tally(sections: &[Section]) -> (usize, usize, usize) {
    let mut ok = 0;
    let mut fail = 0;
    let mut warn = 0;
    for s in sections {
        for c in &s.checks {
            match c.status {
                Status::Ok => ok += 1,
                Status::Fail => fail += 1,
                Status::Warn => warn += 1,
            }
        }
    }
    (ok, fail, warn)
}

fn any_fail(sections: &[Section]) -> bool {
    sections.iter().any(|s| s.checks.iter().any(|c| c.status == Status::Fail))
}

// ----- small helpers -------------------------------------------------------

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn indent_block(s: &str) -> String {
    s.lines().collect::<Vec<_>>().join("\n         ")
}

// ----- tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_version_plain_string() {
        let toml = r#"
[dependencies]
serde = "1"
tauri-plugin-connector = "0.8"
reqwest = "0.12"
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), Some("0.8".into()));
    }

    #[test]
    fn cargo_version_inline_table() {
        let toml = r#"
[dependencies]
tauri-plugin-connector = { version = "0.8.0", features = ["foo"] }
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), Some("0.8.0".into()));
    }

    #[test]
    fn cargo_version_absent() {
        let toml = r#"
[dependencies]
serde = "1"
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), None);
    }

    #[test]
    fn cargo_version_commented_out_is_absent() {
        let toml = r#"
[dependencies]
# tauri-plugin-connector = "0.8"
serde = "1"
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), None);
    }

    #[test]
    fn cargo_version_only_inside_dependencies_tables() {
        // Appearances outside a `dependencies`-flavored section should be ignored.
        let toml = r#"
[package]
tauri-plugin-connector = "shouldnotmatch"

[features]
default = []
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), None);
    }

    #[test]
    fn cargo_version_dev_dependencies() {
        let toml = r#"
[dev-dependencies]
tauri-plugin-connector = "0.8"
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), Some("0.8".into()));
    }

    #[test]
    fn permissions_string_form() {
        let caps: Value = serde_json::from_str(r#"{ "permissions": ["connector:default", "fs:default"] }"#).unwrap();
        assert!(permissions_contain(&caps, "connector:default"));
        assert!(!permissions_contain(&caps, "missing:perm"));
    }

    #[test]
    fn permissions_object_form() {
        let caps: Value = serde_json::from_str(
            r#"{ "permissions": [{ "identifier": "connector:default" }, "fs:default"] }"#,
        )
        .unwrap();
        assert!(permissions_contain(&caps, "connector:default"));
    }

    #[test]
    fn package_deps_found_across_sections() {
        let pkg: Value = serde_json::from_str(
            r#"{ "devDependencies": { "@zumer/snapdom": "^1.0.0" } }"#,
        )
        .unwrap();
        assert!(package_has_dep(&pkg, "@zumer/snapdom"));
        assert!(!package_has_dep(&pkg, "missing-pkg"));
    }

    #[test]
    fn settings_detects_connector_hook() {
        let v: Value = serde_json::from_str(
            r#"{ "hooks": { "UserPromptSubmit": [
                { "command": "bash .claude/hooks/tauri-connector-detect.sh" }
            ] } }"#,
        )
        .unwrap();
        assert!(settings_has_connector_hook(&v));
    }

    #[test]
    fn settings_ignores_unrelated_hooks() {
        let v: Value = serde_json::from_str(
            r#"{ "hooks": { "UserPromptSubmit": [
                { "command": "bash .claude/hooks/other.sh" }
            ] } }"#,
        )
        .unwrap();
        assert!(!settings_has_connector_hook(&v));
    }
}
