//! Discovery helpers for locating running tauri-connector instances.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::identity::{workspace_matches, AppIdentity};
use crate::project::{cwd_project_root, is_other_project, ProjectFacts, PROJECT_ENV};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

const DEFAULT_HOST: &str = "127.0.0.1";

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
    /// Project the app belongs to (plugins 0.17.5+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_root: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cargo_package: Option<String>,
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
    /// Allow attaching to a port-scanned app that does not match the app a
    /// stale `.connector.json` names (`--any-app`, `TAURI_CONNECTOR_ANY_APP=1`).
    pub allow_unrelated_app: bool,
    /// Only connect to an app of this project (`--project`,
    /// `TAURI_CONNECTOR_PROJECT`); see [`crate::project::ProjectFacts::matches`].
    pub project: Option<String>,
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
            allow_unrelated_app: false,
            project: None,
        }
    }
}

/// Environment opt-in for attaching to an unrelated app (see
/// [`ConnectionOptions::allow_unrelated_app`]).
pub const ANY_APP_ENV: &str = "TAURI_CONNECTOR_ANY_APP";

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes"))
}

/// A `.connector.json` near the working directory whose app is gone or no
/// longer answers as the process it recorded.
#[derive(Debug, Clone)]
pub struct StaleRecord {
    pub instance: ConnectorInstance,
    pub reason: String,
}

fn describe_record(instance: &ConnectorInstance) -> String {
    let name = instance.app_name.as_deref().unwrap_or("unnamed app");
    match instance.app_id.as_deref() {
        Some(id) => format!("{name} ({id})"),
        None => name.to_string(),
    }
}

/// Decide what a stale record allows when nothing in this workspace matched:
/// only an app with the record's identity, or anything with the opt-in.
/// `Ok(None)` means "no stale record applies; continue as before".
pub fn stale_record_gate(
    stale: &[StaleRecord],
    others: &[ResolvedConnection],
    allow_unrelated_app: bool,
) -> Result<Option<Vec<ResolvedConnection>>, String> {
    if stale.is_empty() || allow_unrelated_app {
        return Ok(None);
    }
    let expected: Vec<&str> = stale
        .iter()
        .filter_map(|record| record.instance.app_id.as_deref())
        .collect();
    let (same_app, unrelated): (Vec<_>, Vec<_>) = others.iter().cloned().partition(|candidate| {
        candidate
            .identity
            .as_ref()
            .is_some_and(|identity| expected.contains(&identity.app_id.as_str()))
    });
    if !same_app.is_empty() {
        return Ok(Some(same_app));
    }
    let records = stale
        .iter()
        .map(|record| {
            format!(
                "{} records {} pid {} on port {}, but {}",
                record.instance.pid_file.display(),
                describe_record(&record.instance),
                record.instance.pid,
                record.instance.ws_port,
                record.reason
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let found = if unrelated.is_empty() {
        "no running app with that identity was found".to_string()
    } else {
        let apps = unrelated
            .iter()
            .map(|candidate| {
                let identity = candidate.identity.as_ref();
                let id = identity.map(|i| i.app_id.as_str()).unwrap_or("?");
                let app = match identity.and_then(|identity| identity.app_name.as_deref()) {
                    Some(name) => format!("{name} ({id})"),
                    None => id.to_string(),
                };
                format!(
                    "{app} pid {} on port {}",
                    identity.map(|i| i.pid).unwrap_or(0),
                    candidate.port
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("a port scan found a different app instead and did not attach to it: {apps}")
    };
    let choose = unrelated
        .first()
        .map(|candidate| {
            format!(
                "; to use the other app pass --app-id {} or --port {}",
                candidate
                    .identity
                    .as_ref()
                    .map(|i| i.app_id.as_str())
                    .unwrap_or("?"),
                candidate.port
            )
        })
        .unwrap_or_default();
    Err(format!(
        "stale_pid_file: {records}; {found}. Start the app again (it rewrites the record) or delete the stale file{choose}, or set {ANY_APP_ENV}=1 (CLI: --any-app) to attach to any single running app"
    ))
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
    /// The endpoint answered but refused the identity handshake because the
    /// host requires a workflow token the client did not supply. Such an
    /// instance is alive, not stale.
    #[serde(default)]
    pub unauthorized: bool,
    /// The endpoint answers but its plugin predates the identity handshake
    /// (before 0.16.0). It is alive and reachable with `--port`, not stale.
    #[serde(default)]
    pub legacy_plugin: bool,
    pub stale: bool,
    pub error: Option<String>,
    /// What the endpoint reported about itself, when it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<AppIdentity>,
    /// Project name, or `unknown project` for plugins before 0.17.5.
    #[serde(default)]
    pub project: String,
}

/// `plugin=` label for status output; older plugins do not report a version.
pub fn plugin_version_label(identity: Option<&AppIdentity>) -> String {
    match identity.and_then(|identity| identity.plugin_version.as_deref()) {
        Some(version) => version.to_string(),
        None if identity.is_some() => "<0.17.5 (unreported)".to_string(),
        None => "?".to_string(),
    }
}

impl InstanceStatus {
    /// Human label: `ready`, `unauthorized`, `legacy_plugin`, or `stale`.
    pub fn state(&self) -> &'static str {
        if self.stale {
            "stale"
        } else if self.unauthorized {
            "unauthorized"
        } else if self.legacy_plugin {
            "legacy_plugin"
        } else {
            "ready"
        }
    }
}

/// The instance a connection found by port scan (no PID file), as a status
/// row, unless a PID-file row already covers that port. Lets `status` list
/// every app it can connect to, even from an unrelated working directory.
/// Its `instance.pid_file` is empty.
pub fn port_scan_status(
    resolved: &ResolvedConnection,
    statuses: &[InstanceStatus],
) -> Option<InstanceStatus> {
    if resolved.instance.is_some() || statuses.iter().any(|s| s.instance.ws_port == resolved.port) {
        return None;
    }
    Some(scanned_status(resolved.port, resolved.identity.as_ref()?))
}

/// Status rows for every verified app answering on the default port range
/// that no row in `statuses` already covers, so `status` lists every app
/// (with its project) even when it cannot pick one.
pub async fn scan_statuses(host: &str, statuses: &[InstanceStatus]) -> Vec<InstanceStatus> {
    use futures_util::stream;
    let mut rows = stream::iter(
        crate::ports::ws_scan_range()
            .filter(|port| !statuses.iter().any(|s| s.instance.ws_port == *port)),
    )
    .map(|port| async move { (port, endpoint_identity(host, port, 250).await) })
    .buffer_unordered(12)
    .filter_map(|(port, result)| async move {
        result.ok().map(|identity| scanned_status(port, &identity))
    })
    .collect::<Vec<_>>()
    .await;
    rows.sort_by_key(|row| row.instance.ws_port);
    rows
}

fn scanned_status(port: u16, identity: &AppIdentity) -> InstanceStatus {
    InstanceStatus {
        instance: ConnectorInstance {
            pid: identity.pid,
            ws_port: port,
            mcp_port: None,
            bridge_port: None,
            app_name: identity.app_name.clone(),
            app_id: Some(identity.app_id.clone()),
            app_instance_id: Some(identity.app_instance_id.clone()),
            log_dir: None,
            exe: None,
            started_at: Some(identity.started_at),
            pid_file: PathBuf::new(),
            project_name: identity.project_name.clone(),
            project_root: identity.project_root.clone(),
            cargo_package: identity.cargo_package.clone(),
        },
        pid_alive: true,
        ws_reachable: true,
        unauthorized: false,
        legacy_plugin: false,
        stale: false,
        error: None,
        project: ProjectFacts::of(Some(identity), None).label(),
        identity: Some(identity.clone()),
    }
}

/// Whether an identity error means the host refused our credentials (the
/// endpoint is alive) rather than being unreachable.
pub fn is_unauthorized_error(error: &str) -> bool {
    error.contains("\"code\":\"unauthorized\"") || error.starts_with("unauthorized")
}

/// Minimum plugin release that answers the identity handshake used by
/// `--app-id`, `--app-instance-id` and `--pid-file` routing.
pub const IDENTITY_PLUGIN_VERSION: &str = "0.16.0";

fn legacy_plugin_error(ports: &[u16]) -> String {
    let port = ports[0];
    format!(
        "capability_unavailable: running app on port(s) {ports:?} uses a tauri-plugin-connector older than {IDENTITY_PLUGIN_VERSION} that cannot report its app identity, so --app-id/--app-instance-id/--pid-file cannot verify it; upgrade the host app's tauri-plugin-connector (0.17.2 or later recommended), or pass --port {port} to connect without identity verification"
    )
}

/// True when the endpoint answers `bridge_status` but advertises no inspection
/// protocol, i.e. a plugin that predates the identity handshake. Such plugins
/// reply to unknown commands with id `unknown`, so the identity request itself
/// only times out.
pub async fn is_legacy_plugin(host: &str, port: u16, timeout_ms: u64) -> bool {
    let probe = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let mut client = crate::ConnectorClient::new();
        client.connect(host, port).await?;
        client
            .send_with_timeout(json!({"type":"bridge_status"}), timeout_ms)
            .await
    })
    .await;
    matches!(probe, Ok(Ok(status)) if status.is_object() && status.get("inspectionProtocolVersion").is_none())
}

const UNAUTHORIZED_HINT: &str = "the app requires a workflow token; set TAURI_CONNECTOR_WORKFLOW_TOKEN to the host's token, or pass --port to connect without identity verification";

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
    let project = opts
        .project
        .clone()
        .or_else(|| std::env::var(PROJECT_ENV).ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    for identifier in [
        app_id.as_deref(),
        expected_instance.as_deref(),
        project.as_deref(),
    ]
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
            Err(error)
                if app_id.is_some()
                    || expected_instance.is_some()
                    || pid_file.is_some()
                    || project.is_some() =>
            {
                if !is_unauthorized_error(&error) && is_legacy_plugin(&host, port, 1500).await {
                    return Err(legacy_plugin_error(&[port]));
                }
                return Err(error);
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
            if let Some(selector) = project.as_deref() {
                let facts = ProjectFacts::of(Some(identity), None);
                if !facts.matches(selector) {
                    return Err(format!(
                        "project_mismatch: port {port} serves {}, not project '{selector}'",
                        describe_project(&facts)
                    ));
                }
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
    let constrained =
        app_id.is_some() || expected_instance.is_some() || pid_file.is_some() || project.is_some();
    let wrong_project = |identity: &AppIdentity, hint: Option<&ConnectorInstance>| {
        project
            .as_deref()
            .is_some_and(|selector| !ProjectFacts::of(Some(identity), hint).matches(selector))
    };
    // Apps whose project another selector ruled out, for the error message.
    let mut other_projects = Vec::new();
    let hints = discover_instances(&opts.cwd, app_id.as_deref(), pid_file.as_deref());
    // Tier 1: endpoints that match an explicit constraint or this workspace.
    let mut candidates = Vec::new();
    // Tier 2: a live PID file found near the working directory whose endpoint
    // proves it is that exact process. `status` lists these as ready, so every
    // command must be able to use them too (e.g. from the repo root while the
    // app's working directory is `src-tauri`).
    let mut nearby = Vec::new();
    // Tier 3: any other verified endpoint; used only when it is the only one.
    let mut others = Vec::new();
    let mut unauthorized_ports = Vec::new();
    let mut legacy_ports = Vec::new();
    // PID files near the working directory whose app is dead or replaced.
    let mut stale = Vec::new();
    let mut endpoints = HashSet::new();
    for hint in hints {
        if !pid_is_alive(hint.pid) {
            stale.push(StaleRecord {
                reason: format!("process {} is not running", hint.pid),
                instance: hint,
            });
            continue;
        }
        if !endpoints.insert((host.clone(), hint.ws_port)) {
            continue;
        }
        let identity = endpoint_identity(&host, hint.ws_port, 1500).await;
        if let Err(error) = &identity {
            if is_unauthorized_error(error) {
                unauthorized_ports.push(hint.ws_port);
            } else if is_legacy_plugin(&host, hint.ws_port, 1500).await {
                legacy_ports.push(hint.ws_port);
            } else {
                stale.push(StaleRecord {
                    reason: format!("its endpoint does not answer ({error})"),
                    instance: hint.clone(),
                });
            }
        }
        if let Ok(identity) = identity {
            if let Err(error) = validate_identity(
                &identity,
                app_id.as_deref(),
                expected_instance.as_deref(),
                Some(&hint),
            ) {
                if error.contains("stale PID file") {
                    stale.push(StaleRecord {
                        reason: format!(
                            "port {} now answers as {} pid {}",
                            hint.ws_port, identity.app_id, identity.pid
                        ),
                        instance: hint,
                    });
                }
            } else if wrong_project(&identity, Some(&hint)) {
                other_projects.push(ResolvedConnection {
                    host: host.clone(),
                    port: hint.ws_port,
                    source: ConnectionSource::PidFile,
                    instance: Some(hint),
                    identity: Some(identity),
                });
            } else {
                let in_workspace = in_workspace(&opts.cwd, &identity);
                let candidate = ResolvedConnection {
                    host: host.clone(),
                    port: hint.ws_port,
                    source: ConnectionSource::PidFile,
                    instance: Some(hint),
                    identity: Some(identity),
                };
                if constrained || in_workspace {
                    candidates.push(candidate);
                } else {
                    nearby.push(candidate);
                }
            }
        }
    }
    if pid_file.is_none() {
        use futures_util::stream;
        let results = stream::iter(
            crate::ports::ws_scan_range()
                .filter(|port| !endpoints.contains(&(host.clone(), *port))),
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
                {
                    let in_workspace = in_workspace(&opts.cwd, &identity);
                    let wrong_project = wrong_project(&identity, None);
                    let candidate = ResolvedConnection {
                        host: host.clone(),
                        port,
                        source: ConnectionSource::PortScan,
                        instance: None,
                        identity: Some(identity),
                    };
                    if wrong_project {
                        other_projects.push(candidate);
                    } else if constrained || in_workspace {
                        candidates.push(candidate);
                    } else {
                        others.push(candidate);
                    }
                }
            }
        }
    }
    if candidates.is_empty() {
        candidates = nearby;
    }
    // R2: a dead or replaced app named by a PID file must not silently hand
    // the session to whatever other app a port scan finds.
    let allow_unrelated_app = opts.allow_unrelated_app || env_flag(ANY_APP_ENV);
    if candidates.is_empty()
        && unauthorized_ports.is_empty()
        && legacy_ports.is_empty()
        && !constrained
    {
        if let Some(same_app) = stale_record_gate(&stale, &others, allow_unrelated_app)? {
            return select_unique(same_app);
        }
    }
    if candidates.is_empty() && others.len() == 1 && !constrained {
        // Exactly one verified app is running and nothing ties the working
        // directory to another one: select it, as `status` does. An app that
        // reports a different project than the working directory's is
        // refused: several apps share the default ports.
        if !allow_unrelated_app {
            project_gate(&opts.cwd, &others[0])?;
        }
        candidates = others;
    } else if candidates.is_empty() && others.len() > 1 {
        // Several unrelated apps: report them instead of a bare app_not_found.
        return select_unique(others);
    }
    if candidates.is_empty() && !unauthorized_ports.is_empty() {
        return Err(format!(
            "unauthorized: running app on port(s) {unauthorized_ports:?} refused identity; {UNAUTHORIZED_HINT}"
        ));
    }
    if candidates.is_empty() && !legacy_ports.is_empty() {
        return Err(legacy_plugin_error(&legacy_ports));
    }
    if candidates.is_empty() && !other_projects.is_empty() {
        let selector = project.as_deref().unwrap_or_default();
        let running = other_projects
            .iter()
            .map(|candidate| {
                format!(
                    "{} on port {}",
                    describe_project(&ProjectFacts::of(
                        candidate.identity.as_ref(),
                        candidate.instance.as_ref()
                    )),
                    candidate.port
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "project_mismatch: no running app belongs to project '{selector}'; running: {running}. Run `tauri-connector status` to list projects"
        ));
    }
    select_unique(candidates)
}

/// Whether the endpoint's workspace or project root contains the working
/// directory.
fn in_workspace(cwd: &Path, identity: &AppIdentity) -> bool {
    [
        identity.workspace_path.as_deref(),
        identity.project_root.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|path| workspace_matches(cwd, path))
}

/// `R2 (com.example.r2) [project R2 at /src/r2]`, or `unknown project`.
fn describe_project(facts: &ProjectFacts<'_>) -> String {
    let app = match (facts.app_name, facts.app_id) {
        (Some(name), Some(id)) => format!("{name} ({id})"),
        (None, Some(id)) => id.to_string(),
        (Some(name), None) => name.to_string(),
        (None, None) => "an app".to_string(),
    };
    match facts.root {
        Some(root) => format!("{app} [project {} at {}]", facts.label(), root.display()),
        None => format!("{app} [{}]", facts.label()),
    }
}

/// Refuse a fallback app that belongs to a different project than `cwd`.
/// Apps that do not report a project root (plugins before 0.17.5) pass.
pub fn project_gate(cwd: &Path, candidate: &ResolvedConnection) -> Result<(), String> {
    let facts = ProjectFacts::of(candidate.identity.as_ref(), candidate.instance.as_ref());
    let cwd_root = cwd_project_root(cwd);
    if !is_other_project(cwd_root.as_deref(), facts.root) {
        return Ok(());
    }
    let selector = facts.name.or(facts.app_id).unwrap_or_default();
    Err(format!(
        "project_mismatch: the only running app is {} on port {}, but this directory belongs to the project at {}. Start this project's app, or pass --project '{selector}' (or --port {}) to use the other one, or set {ANY_APP_ENV}=1 (CLI: --any-app)",
        describe_project(&facts),
        candidate.port,
        cwd_root.as_deref().unwrap_or(cwd).display(),
        candidate.port,
    ))
}

pub fn select_unique(
    mut candidates: Vec<ResolvedConnection>,
) -> Result<ResolvedConnection, String> {
    match candidates.len() {
        0=>Err("app_not_found: no endpoint satisfies the requested identity; no default-instance fallback was attempted".into()),
        1=>Ok(candidates.remove(0)),
        _=>{
            let summaries=candidates.iter().map(|candidate|json!({"host":candidate.host,"port":candidate.port,"appId":candidate.identity.as_ref().map(|identity|&identity.app_id),"appInstanceId":candidate.identity.as_ref().map(|identity|&identity.app_instance_id),"project":ProjectFacts::of(candidate.identity.as_ref(),candidate.instance.as_ref()).label()})).collect::<Vec<_>>();
            Err(format!("ambiguous_app: {} verified candidates {}; specify --project, --app-instance-id or --host/--port",candidates.len(),json!(summaries)))
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
        let mut legacy_plugin = false;
        let mut reported = None;
        let (ws_reachable, unauthorized, error) = if pid_alive {
            match endpoint_identity(host, instance.ws_port, 1_000).await {
                Ok(identity) => {
                    let checked = validate_identity(&identity, app_id, None, Some(&instance));
                    reported = Some(identity);
                    match checked {
                        Ok(()) => (true, false, None),
                        Err(e) => (false, false, Some(e)),
                    }
                }
                Err(e) if is_unauthorized_error(&e) => (
                    true,
                    true,
                    Some(format!("unauthorized: {UNAUTHORIZED_HINT}")),
                ),
                Err(_) if is_legacy_plugin(host, instance.ws_port, 1_000).await => {
                    legacy_plugin = true;
                    (true, false, Some(legacy_plugin_error(&[instance.ws_port])))
                }
                Err(e) => (false, false, Some(e)),
            }
        } else {
            (false, false, Some("process is not running".to_string()))
        };
        let project = ProjectFacts::of(reported.as_ref(), Some(&instance)).label();
        statuses.push(InstanceStatus {
            project,
            instance,
            pid_alive,
            ws_reachable,
            unauthorized,
            legacy_plugin,
            stale: !pid_alive || !ws_reachable,
            error,
            identity: reported,
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
        #[serde(default)]
        project_name: Option<String>,
        #[serde(default)]
        project_root: Option<PathBuf>,
        #[serde(default)]
        cargo_package: Option<String>,
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
        project_name: raw.project_name,
        project_root: raw.project_root,
        cargo_package: raw.cargo_package,
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
    fn a_port_scanned_connection_is_listed_as_an_instance() {
        let identity = AppIdentity {
            app_instance_id: "inst-1".into(),
            app_id: "com.example.strata".into(),
            pid: 4242,
            started_at: 7,
            workspace_id: None,
            workspace_path: None,
            inspection_protocol_version: 1,
            workflow_protocol_version: None,
            plugin_version: None,
            app_name: None,
            app_version: None,
            project_name: None,
            project_root: None,
            cargo_package: None,
        };
        let resolved = ResolvedConnection {
            host: "127.0.0.1".into(),
            port: 9561,
            source: ConnectionSource::PortScan,
            instance: None,
            identity: Some(identity),
        };
        let row = port_scan_status(&resolved, &[]).expect("listed");
        assert_eq!(row.state(), "ready");
        assert_eq!(row.instance.ws_port, 9561);
        assert_eq!(row.instance.pid, 4242);
        assert_eq!(row.instance.app_id.as_deref(), Some("com.example.strata"));
        assert!(row.instance.pid_file.as_os_str().is_empty());
        // Not duplicated when a PID-file row already names the port.
        assert!(port_scan_status(&resolved, &[row]).is_none());
    }

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
            project_name: None,
            project_root: None,
            cargo_package: None,
        };
        assert_eq!(
            instance.snapshots_dir(),
            PathBuf::from("/tmp/logs/snapshots")
        );
    }

    #[test]
    fn unauthorized_identity_is_alive_not_stale() {
        let wire =
            r#"{"code":"unauthorized","message":"Workflow credential is missing or invalid"}"#;
        assert!(is_unauthorized_error(wire));
        assert!(is_unauthorized_error("unauthorized: refused"));
        assert!(!is_unauthorized_error(
            "identity_unavailable: handshake timed out"
        ));
        let instance = ConnectorInstance {
            pid: 42,
            ws_port: 9555,
            mcp_port: None,
            bridge_port: None,
            app_name: None,
            app_id: None,
            app_instance_id: None,
            log_dir: None,
            exe: None,
            started_at: None,
            pid_file: PathBuf::from("/tmp/.connector.json"),
            project_name: None,
            project_root: None,
            cargo_package: None,
        };
        let status = InstanceStatus {
            instance,
            pid_alive: true,
            ws_reachable: true,
            unauthorized: true,
            legacy_plugin: false,
            stale: false,
            error: None,
            identity: None,
            project: "unknown project".into(),
        };
        assert_eq!(status.state(), "unauthorized");
        let old_plugin = InstanceStatus {
            unauthorized: false,
            legacy_plugin: true,
            ..status.clone()
        };
        assert_eq!(old_plugin.state(), "legacy_plugin");
        let legacy: InstanceStatus = serde_json::from_value(serde_json::json!({
            "instance": serde_json::to_value(&status.instance).unwrap(),
            "pid_alive": true, "ws_reachable": false, "stale": true, "error": null
        }))
        .unwrap();
        assert_eq!(legacy.state(), "stale");
    }
}

#[cfg(test)]
mod project_tests {
    use super::*;

    fn scanned(root: Option<PathBuf>, name: Option<&str>) -> ResolvedConnection {
        ResolvedConnection {
            host: "127.0.0.1".into(),
            port: 9555,
            source: ConnectionSource::PortScan,
            instance: None,
            identity: Some(AppIdentity {
                app_instance_id: "inst".into(),
                app_id: "com.example.gfit".into(),
                pid: 7,
                started_at: 1,
                workspace_id: None,
                workspace_path: None,
                inspection_protocol_version: 1,
                workflow_protocol_version: None,
                plugin_version: Some("0.17.5".into()),
                app_name: Some("GFIT Workout".into()),
                app_version: None,
                project_name: name.map(str::to_owned),
                project_root: root,
                cargo_package: None,
            }),
        }
    }

    #[test]
    fn the_only_running_app_is_refused_when_it_belongs_to_another_project() {
        let base = std::env::temp_dir().join(format!("tc-gate-{}", uuid::Uuid::new_v4()));
        let base = {
            std::fs::create_dir_all(&base).unwrap();
            base.canonicalize().unwrap()
        };
        let r2 = base.join("r2");
        let gfit = base.join("gfit");
        std::fs::create_dir_all(r2.join("src-tauri")).unwrap();
        std::fs::create_dir_all(gfit.join("src-tauri")).unwrap();

        let other = scanned(Some(gfit.clone()), Some("GFIT Workout"));
        let error = project_gate(&r2, &other).unwrap_err();
        assert!(error.starts_with("project_mismatch:"), "{error}");
        assert!(
            error.contains("GFIT Workout (com.example.gfit) [project GFIT Workout at"),
            "{error}"
        );
        assert!(error.contains("--project 'GFIT Workout'"), "{error}");
        // Same project (from src-tauri too), an older plugin, and a working
        // directory outside any Tauri project all keep the fallback.
        assert!(project_gate(&gfit.join("src-tauri"), &other).is_ok());
        assert!(project_gate(&r2, &scanned(None, None)).is_ok());
        assert!(project_gate(&base, &other).is_ok());

        let row = port_scan_status(&scanned(None, None), &[]).unwrap();
        assert_eq!(row.project, "unknown project");
        let row = port_scan_status(&other, &[]).unwrap();
        assert_eq!(row.project, "GFIT Workout");
        assert_eq!(row.instance.project_root.as_deref(), Some(gfit.as_path()));
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["project"], "GFIT Workout");
        assert_eq!(json["identity"]["projectName"], "GFIT Workout");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn pid_files_carry_the_project_and_older_ones_still_parse() {
        let dir = std::env::temp_dir().join(format!("tc-pid-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".connector.json");
        std::fs::write(
            &path,
            r#"{"pid":1,"ws_port":9555,"app_id":"com.r2","project_name":"R2","project_root":"/src/r2","cargo_package":"r2-app"}"#,
        )
        .unwrap();
        let instance = read_instance_file(&path).unwrap();
        assert_eq!(instance.project_name.as_deref(), Some("R2"));
        assert_eq!(instance.project_root.as_deref(), Some(Path::new("/src/r2")));
        assert_eq!(instance.cargo_package.as_deref(), Some("r2-app"));
        assert!(ProjectFacts::of(None, Some(&instance)).matches("r2-app"));
        std::fs::write(&path, r#"{"pid":1,"ws_port":9555}"#).unwrap();
        let old = read_instance_file(&path).unwrap();
        assert_eq!(
            ProjectFacts::of(None, Some(&old)).label(),
            "unknown project"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
