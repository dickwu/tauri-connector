//! Discovery helpers for locating running tauri-connector instances.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_SCAN_RANGE: std::ops::RangeInclusive<u16> = 9555..=9655;

/// Connector instance metadata written by the plugin into `.connector.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectorInstance {
    pub pid: u32,
    pub ws_port: u16,
    pub mcp_port: Option<u16>,
    pub bridge_port: Option<u16>,
    pub app_name: Option<String>,
    pub app_id: Option<String>,
    pub log_dir: Option<PathBuf>,
    pub exe: Option<PathBuf>,
    pub started_at: Option<u64>,
    #[serde(skip_deserializing)]
    pub pid_file: PathBuf,
}

impl ConnectorInstance {
    /// Snapshot directory used by the plugin.
    pub fn snapshots_dir(&self) -> PathBuf {
        self.log_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join(format!("tauri-connector-{}", self.pid)))
            .join("snapshots")
    }
}

/// Discovery inputs shared by CLI and standalone MCP server.
#[derive(Debug, Clone)]
pub struct ConnectionOptions {
    pub cwd: PathBuf,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub app_id: Option<String>,
    pub pid_file: Option<PathBuf>,
}

impl ConnectionOptions {
    pub fn from_current_dir() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            host: None,
            port: None,
            app_id: None,
            pid_file: None,
        }
    }
}

/// How the active connection was resolved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionSource {
    Explicit,
    Env,
    PidFile,
    PortScan,
}

/// Resolved WebSocket endpoint plus optional instance metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedConnection {
    pub host: String,
    pub port: u16,
    pub source: ConnectionSource,
    pub instance: Option<ConnectorInstance>,
}

/// Status for one discovered PID file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub instance: ConnectorInstance,
    pub pid_alive: bool,
    pub ws_reachable: bool,
    pub stale: bool,
    pub error: Option<String>,
}

/// Resolve an endpoint using explicit inputs, environment, PID files, then scan.
pub async fn resolve_connection(opts: ConnectionOptions) -> Result<ResolvedConnection, String> {
    let env_host = std::env::var("TAURI_CONNECTOR_HOST").ok();
    let env_port = std::env::var("TAURI_CONNECTOR_PORT")
        .ok()
        .map(|p| {
            p.parse::<u16>()
                .map_err(|_| format!("Invalid TAURI_CONNECTOR_PORT={p}"))
        })
        .transpose()?;

    if opts.port.is_some() || opts.host.is_some() {
        let host = opts
            .host
            .or(env_host)
            .unwrap_or_else(|| DEFAULT_HOST.to_string());
        let port = opts.port.or(env_port).unwrap_or(9555);
        return Ok(ResolvedConnection {
            host,
            port,
            source: ConnectionSource::Explicit,
            instance: None,
        });
    }

    if let Some(port) = env_port {
        return Ok(ResolvedConnection {
            host: env_host.unwrap_or_else(|| DEFAULT_HOST.to_string()),
            port,
            source: ConnectionSource::Env,
            instance: None,
        });
    }

    let host = env_host.unwrap_or_else(|| DEFAULT_HOST.to_string());
    let app_id = opts
        .app_id
        .or_else(|| std::env::var("TAURI_CONNECTOR_APP_ID").ok());

    let instances = discover_instances(&opts.cwd, app_id.as_deref(), opts.pid_file.as_deref());
    let mut live = Vec::new();
    let mut stale = Vec::new();
    for instance in instances {
        if !pid_is_alive(instance.pid) {
            stale.push(format!(
                "{} (pid {} is not running)",
                instance.pid_file.display(),
                instance.pid
            ));
            continue;
        }
        match ping_ws(&host, instance.ws_port, 1_500).await {
            Ok(()) => live.push(instance),
            Err(e) => stale.push(format!(
                "{} (ws_port {} not reachable: {e})",
                instance.pid_file.display(),
                instance.ws_port
            )),
        }
    }

    if !live.is_empty() {
        live.sort_by_key(|i| std::cmp::Reverse(i.started_at.unwrap_or(0)));
        let instance = live.remove(0);
        return Ok(ResolvedConnection {
            host,
            port: instance.ws_port,
            source: ConnectionSource::PidFile,
            instance: Some(instance),
        });
    }

    for port in DEFAULT_SCAN_RANGE {
        if ping_ws(&host, port, 250).await.is_ok() {
            return Ok(ResolvedConnection {
                host,
                port,
                source: ConnectionSource::PortScan,
                instance: None,
            });
        }
    }

    let stale_hint = if stale.is_empty() {
        String::new()
    } else {
        format!("\nStale connector files:\n- {}", stale.join("\n- "))
    };
    Err(format!(
        "No running tauri-connector instance found. Start the Tauri app, pass --host/--port, set TAURI_CONNECTOR_PORT, or remove stale .connector.json files.{stale_hint}"
    ))
}

/// Return statuses for every PID file candidate.
pub async fn instance_statuses(
    cwd: &Path,
    app_id: Option<&str>,
    pid_file: Option<&Path>,
    host: Option<&str>,
) -> Vec<InstanceStatus> {
    let host = host.unwrap_or(DEFAULT_HOST);
    let instances = discover_instances(cwd, app_id, pid_file);
    let mut statuses = Vec::with_capacity(instances.len());
    for instance in instances {
        let pid_alive = pid_is_alive(instance.pid);
        let (ws_reachable, error) = if pid_alive {
            match ping_ws(host, instance.ws_port, 1_000).await {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e)),
            }
        } else {
            (false, Some("process is not running".to_string()))
        };
        statuses.push(InstanceStatus {
            instance,
            pid_alive,
            ws_reachable,
            stale: !pid_alive || !ws_reachable,
            error,
        });
    }
    statuses.sort_by_key(|s| std::cmp::Reverse(s.instance.started_at.unwrap_or(0)));
    statuses
}

/// Read all matching `.connector.json` files near `cwd`.
pub fn discover_instances(
    cwd: &Path,
    app_id: Option<&str>,
    pid_file: Option<&Path>,
) -> Vec<ConnectorInstance> {
    let mut paths = Vec::new();
    if let Some(p) = pid_file {
        paths.push(p.to_path_buf());
    }
    if let Ok(p) = std::env::var("TAURI_CONNECTOR_PID_FILE") {
        paths.push(PathBuf::from(p));
    }
    paths.extend(pid_file_candidates(cwd));

    let mut seen = HashSet::new();
    let mut instances = Vec::new();
    for path in paths {
        let key = path.canonicalize().unwrap_or(path.clone());
        if !seen.insert(key) {
            continue;
        }
        let Some(instance) = read_instance_file(&path) else {
            continue;
        };
        if app_id.is_some_and(|id| instance.app_id.as_deref() != Some(id)) {
            continue;
        }
        instances.push(instance);
    }
    instances
}

/// Build the candidate list documented by the CLI/playbook.
pub fn pid_file_candidates(cwd: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for root in cwd.ancestors().take(8) {
        candidates.extend([
            root.join("src-tauri/target/.connector.json"),
            root.join("src-tauri/target/debug/.connector.json"),
            root.join("src-tauri/target/release/.connector.json"),
            root.join("target/.connector.json"),
            root.join("target/debug/.connector.json"),
            root.join("target/release/.connector.json"),
        ]);
    }
    candidates
}

fn read_instance_file(path: &Path) -> Option<ConnectorInstance> {
    #[derive(Deserialize)]
    struct RawInstance {
        pid: u32,
        ws_port: u16,
        #[serde(default)]
        mcp_port: Option<u16>,
        #[serde(default)]
        bridge_port: Option<u16>,
        #[serde(default)]
        app_name: Option<String>,
        #[serde(default)]
        app_id: Option<String>,
        #[serde(default)]
        log_dir: Option<PathBuf>,
        #[serde(default)]
        exe: Option<PathBuf>,
        #[serde(default)]
        started_at: Option<u64>,
    }

    let raw: RawInstance = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(ConnectorInstance {
        pid: raw.pid,
        ws_port: raw.ws_port,
        mcp_port: raw.mcp_port,
        bridge_port: raw.bridge_port,
        app_name: raw.app_name,
        app_id: raw.app_id,
        log_dir: raw.log_dir,
        exe: raw.exe,
        started_at: raw.started_at,
        pid_file: path.to_path_buf(),
    })
}

/// Ping a connector WebSocket endpoint.
pub async fn ping_ws(host: &str, port: u16, timeout_ms: u64) -> Result<(), String> {
    let url = format!("ws://{host}:{port}");
    let connect = tokio_tungstenite::connect_async(&url);
    let (mut ws, _) = tokio::time::timeout(Duration::from_millis(timeout_ms), connect)
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|e| format!("connect failed: {e}"))?;

    let payload = json!({ "id": "discovery-ping", "type": "ping" }).to_string();
    ws.send(Message::Text(payload.into()))
        .await
        .map_err(|e| format!("ping send failed: {e}"))?;

    let next = tokio::time::timeout(Duration::from_millis(timeout_ms), ws.next())
        .await
        .map_err(|_| "ping timed out".to_string())?;
    let Some(Ok(Message::Text(text))) = next else {
        return Err("ping returned no text response".to_string());
    };
    let value: serde_json::Value =
        serde_json::from_str(text.as_ref()).map_err(|e| format!("invalid ping JSON: {e}"))?;
    if value.get("result").and_then(|v| v.as_str()) == Some("pong") {
        Ok(())
    } else {
        Err("ping did not return pong".to_string())
    }
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_include_documented_locations() {
        let cwd = Path::new("/tmp/example/app");
        let candidates = pid_file_candidates(cwd);
        assert!(candidates
            .iter()
            .any(|p| p.ends_with("src-tauri/target/.connector.json")));
        assert!(candidates
            .iter()
            .any(|p| p.ends_with("target/debug/.connector.json")));
    }

    #[test]
    fn instance_snapshot_dir_prefers_log_dir() {
        let instance = ConnectorInstance {
            pid: 42,
            ws_port: 9555,
            mcp_port: None,
            bridge_port: None,
            app_name: None,
            app_id: None,
            log_dir: Some(PathBuf::from("/tmp/logs")),
            exe: None,
            started_at: None,
            pid_file: PathBuf::from("/tmp/.connector.json"),
        };
        assert_eq!(
            instance.snapshots_dir(),
            PathBuf::from("/tmp/logs/snapshots")
        );
    }
}
