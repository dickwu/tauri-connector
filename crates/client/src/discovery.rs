//! Discovery helpers for locating running tauri-connector instances.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::identity::{workspace_matches, AppIdentity};
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
    pub app_instance_id: Option<String>,
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
    pub app_instance_id: Option<String>,
    pub pid_file: Option<PathBuf>,
}

impl ConnectionOptions {
    pub fn from_current_dir() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            host: None,
            port: None,
            app_id: None,
            app_instance_id: None,
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
    pub identity: Option<AppIdentity>,
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

/// Resolve every candidate against its live identity. Explicit constraints never fall back.
pub async fn resolve_connection(opts: ConnectionOptions) -> Result<ResolvedConnection, String> {
    let env_host = std::env::var("TAURI_CONNECTOR_HOST").ok();
    let env_port = std::env::var("TAURI_CONNECTOR_PORT")
        .ok()
        .map(|p| {
            p.parse::<u16>()
                .map_err(|_| "invalid_arguments: invalid TAURI_CONNECTOR_PORT".to_string())
        })
        .transpose()?;
    let host = opts
        .host
        .clone()
        .or(env_host)
        .unwrap_or_else(|| DEFAULT_HOST.into());
    let app_id = opts
        .app_id
        .clone()
        .or_else(|| std::env::var("TAURI_CONNECTOR_APP_ID").ok());
    let expected_instance = opts
        .app_instance_id
        .clone()
        .or_else(|| std::env::var("TAURI_CONNECTOR_APP_INSTANCE_ID").ok());
    for identifier in [app_id.as_deref(), expected_instance.as_deref()]
        .into_iter()
        .flatten()
    {
        if identifier.is_empty()
            || identifier.len() > 256
            || identifier.chars().any(char::is_control)
        {
            return Err("invalid_arguments: malformed application identifier".into());
        }
    }
    let pid_file = opts.pid_file.clone().or_else(|| {
        std::env::var("TAURI_CONNECTOR_PID_FILE")
            .ok()
            .map(PathBuf::from)
    });
    let explicit_endpoint = opts.port.is_some() || opts.host.is_some() || env_port.is_some();
    if explicit_endpoint {
        let port = opts.port.or(env_port).unwrap_or(9555);
        if port == 0 {
            return Err("invalid_arguments: port must be nonzero".into());
        }
        let identity = endpoint_identity(&host, port, 1500).await;
        let identity = match identity {
            Ok(identity) => Some(identity),
            Err(error) if app_id.is_some() || expected_instance.is_some() || pid_file.is_some() => {
                return Err(error)
            }
            Err(_) => {
                ping_ws(&host, port, 1500).await?;
                None
            }
        };
        if let Some(ref identity) = identity {
            validate_identity(
                identity,
                app_id.as_deref(),
                expected_instance.as_deref(),
                None,
            )?;
            if let Some(path) = pid_file.as_deref() {
                let hint = read_instance_file(path)
                    .ok_or("app_not_found: explicit PID file unavailable")?;
                validate_identity(
                    identity,
                    app_id.as_deref(),
                    expected_instance.as_deref(),
                    Some(&hint),
                )?;
            }
        }
        return Ok(ResolvedConnection {
            host,
            port,
            source: if opts.host.is_some() || opts.port.is_some() {
                ConnectionSource::Explicit
            } else {
                ConnectionSource::Env
            },
            instance: None,
            identity,
        });
    }
    let hints = discover_instances(&opts.cwd, app_id.as_deref(), pid_file.as_deref());
    let mut candidates = Vec::new();
    let mut endpoints = HashSet::new();
    for hint in hints {
        if !pid_is_alive(hint.pid) || !endpoints.insert((host.clone(), hint.ws_port)) {
            continue;
        }
        if let Ok(identity) = endpoint_identity(&host, hint.ws_port, 1500).await {
            if validate_identity(
                &identity,
                app_id.as_deref(),
                expected_instance.as_deref(),
                Some(&hint),
            )
            .is_ok()
                && (app_id.is_some()
                    || expected_instance.is_some()
                    || pid_file.is_some()
                    || identity
                        .workspace_path
                        .as_deref()
                        .is_some_and(|path| workspace_matches(&opts.cwd, path)))
            {
                candidates.push(ResolvedConnection {
                    host: host.clone(),
                    port: hint.ws_port,
                    source: ConnectionSource::PidFile,
                    instance: Some(hint),
                    identity: Some(identity),
                });
            }
        }
    }
    if pid_file.is_none() {
        use futures_util::stream;
        let results = stream::iter(
            DEFAULT_SCAN_RANGE.filter(|port| !endpoints.contains(&(host.clone(), *port))),
        )
        .map(|port| {
            let host = host.clone();
            async move { (port, endpoint_identity(&host, port, 250).await) }
        })
        .buffer_unordered(12)
        .collect::<Vec<_>>()
        .await;
        for (port, result) in results {
            if let Ok(identity) = result {
                if validate_identity(
                    &identity,
                    app_id.as_deref(),
                    expected_instance.as_deref(),
                    None,
                )
                .is_ok()
                    && (app_id.is_some()
                        || expected_instance.is_some()
                        || identity
                            .workspace_path
                            .as_deref()
                            .is_some_and(|path| workspace_matches(&opts.cwd, path)))
                {
                    candidates.push(ResolvedConnection {
                        host: host.clone(),
                        port,
                        source: ConnectionSource::PortScan,
                        instance: None,
                        identity: Some(identity),
                    });
                }
            }
        }
    }
    select_unique(candidates)
}

pub fn select_unique(
    mut candidates: Vec<ResolvedConnection>,
) -> Result<ResolvedConnection, String> {
    match candidates.len() {
        0=>Err("app_not_found: no endpoint satisfies the requested identity; no default-instance fallback was attempted".into()),
        1=>Ok(candidates.remove(0)),
        _=>{
            let summaries=candidates.iter().map(|candidate|json!({"host":candidate.host,"port":candidate.port,"appId":candidate.identity.as_ref().map(|identity|&identity.app_id),"appInstanceId":candidate.identity.as_ref().map(|identity|&identity.app_instance_id)})).collect::<Vec<_>>();
            Err(format!("ambiguous_app: {} verified candidates {}; specify --app-instance-id or --host/--port",candidates.len(),json!(summaries)))
        },
    }
}

pub fn validate_identity(
    identity: &AppIdentity,
    app_id: Option<&str>,
    instance_id: Option<&str>,
    hint: Option<&ConnectorInstance>,
) -> Result<(), String> {
    if app_id.is_some_and(|id| identity.app_id != id)
        || instance_id.is_some_and(|id| identity.app_instance_id != id)
    {
        return Err("app_identity_mismatch: endpoint does not match requested app".into());
    }
    if let Some(hint) = hint {
        if hint.pid != identity.pid
            || hint
                .started_at
                .is_some_and(|start| start != identity.started_at)
            || hint
                .app_id
                .as_deref()
                .is_some_and(|id| id != identity.app_id)
            || hint
                .app_instance_id
                .as_deref()
                .is_some_and(|id| id != identity.app_instance_id)
        {
            return Err("app_identity_mismatch: stale PID file or reused endpoint/process".into());
        }
    }
    Ok(())
}

pub async fn endpoint_identity(
    host: &str,
    port: u16,
    timeout_ms: u64,
) -> Result<AppIdentity, String> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let mut client = crate::ConnectorClient::new();
        client.connect(host, port).await?;
        let mut args = json!({});
        if let Ok(token) = std::env::var("TAURI_CONNECTOR_WORKFLOW_TOKEN") {
            args["authToken"] = json!(token);
        }
        AppIdentity::parse(
            client
                .send_with_timeout(
                    json!({"type":"inspection","operation":"app_identity","args":args}),
                    timeout_ms,
                )
                .await?,
        )
    })
    .await
    .map_err(|_| "identity_unavailable: handshake timed out".to_string())?
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
            match endpoint_identity(host, instance.ws_port, 1_000)
                .await
                .and_then(|identity| validate_identity(&identity, app_id, None, Some(&instance)))
            {
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
    let paths = if let Some(p) = pid_file {
        vec![p.to_path_buf()]
    } else if let Ok(p) = std::env::var("TAURI_CONNECTOR_PID_FILE") {
        vec![PathBuf::from(p)]
    } else {
        pid_file_candidates(cwd)
    };

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
        #[serde(default, alias = "appInstanceId")]
        app_instance_id: Option<String>,
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
        app_instance_id: raw.app_instance_id,
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
            app_instance_id: None,
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
