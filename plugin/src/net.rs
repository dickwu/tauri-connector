//! Port selection shared by the bridge, WebSocket and MCP listeners.

use std::net::TcpListener;

/// Bind the first free port in the inclusive range `start..=end` and keep the
/// listener, so no other process can take the port between selection and use.
///
/// The returned listener is non-blocking, ready for
/// `tokio::net::TcpListener::from_std`. Listeners bind through [`bind_plan`].
#[cfg(test)]
pub(crate) fn bind_in_range(
    addr: &str,
    start: u16,
    end: u16,
) -> Result<(TcpListener, u16), String> {
    bind_first(addr, start, end, start..=end)
}

fn bind_first(
    addr: &str,
    start: u16,
    end: u16,
    ports: impl Iterator<Item = u16>,
) -> Result<(TcpListener, u16), String> {
    if start > end {
        return Err(format!("Invalid port range {start}-{end}"));
    }
    let mut last_error = None;
    for port in ports {
        match TcpListener::bind((addr, port)) {
            Ok(listener) => {
                listener
                    .set_nonblocking(true)
                    .map_err(|e| format!("Failed to configure listener on {addr}:{port}: {e}"))?;
                let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
                return Ok((listener, bound));
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(match last_error {
        Some(e) => format!("No available port on {addr} in range {start}-{end} (last error: {e})"),
        None => format!("No available port on {addr} in range {start}-{end}"),
    })
}

/// Bind a listener's [`PortPlan`]: its preferred (remembered) port, else the
/// first free port of a range, or exactly the configured port (a busy exact
/// port is an error, never a fallback).
pub(crate) fn bind_plan(
    addr: &str,
    plan: &connector_client::ports::PortPlan,
) -> Result<(TcpListener, u16), String> {
    bind_first(addr, plan.start, plan.end, plan.candidates())
        .map_err(|error| plan.bind_error(&error))
}

/// The WebSocket and MCP ports an app bound last time (0.17.8), kept in its
/// app data dir as `.tauri-connector/ports.json`. Before this, two apps on
/// the default ranges swapped 9555/9556 and 9557/9558 whenever they started
/// in the other order (gfit). Each app now tries its own last ports first,
/// so the pair settles after the first run with both apps, and a single app
/// keeps 9555/9556.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RememberedPorts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<u16>,
}

pub(crate) const REMEMBERED_PORTS_FILE: &str = "ports.json";

impl RememberedPorts {
    pub(crate) fn load(dir: &std::path::Path) -> Self {
        std::fs::read_to_string(dir.join(REMEMBERED_PORTS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Record the ports bound from a range; exact ports are not remembered
    /// (they win anyway) and keep the earlier entry.
    pub(crate) fn update(
        self,
        ws: Option<u16>,
        ws_plan: &connector_client::ports::PortPlan,
        mcp: Option<u16>,
        mcp_plan: &connector_client::ports::PortPlan,
    ) -> Self {
        let pick =
            |bound: Option<u16>, plan: &connector_client::ports::PortPlan, old: Option<u16>| {
                if plan.exact.is_some() {
                    old
                } else {
                    bound.or(old)
                }
            };
        Self {
            ws: pick(ws, ws_plan, self.ws),
            mcp: pick(mcp, mcp_plan, self.mcp),
        }
    }

    pub(crate) fn save(self, dir: &std::path::Path) {
        if let Ok(text) = serde_json::to_string(&self) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join(REMEMBERED_PORTS_FILE), text);
        }
    }
}

/// `TAURI_CONNECTOR_REMEMBER_PORTS=0` (or `false`/`off`) turns remembered
/// ports off: every start scans from the range start, as before 0.17.8.
pub(crate) fn remember_ports_enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

#[cfg(test)]
mod tests {
    use super::{bind_in_range, bind_plan};

    #[test]
    fn a_busy_exact_port_fails_without_falling_back() {
        use connector_client::ports::{self, WS_PORT_VAR};
        let (held, port) = bind_in_range("127.0.0.1", 0, 0).unwrap();
        let text = port.to_string();
        let (plan, _) = ports::resolve_port(
            |name| (name == WS_PORT_VAR).then(|| text.clone()),
            WS_PORT_VAR,
            ports::WS_RANGE_VAR,
            None,
            None,
            ports::DEFAULT_WS_RANGE,
        )
        .unwrap();
        let error = bind_plan("127.0.0.1", &plan).unwrap_err();
        assert!(
            error.contains(WS_PORT_VAR) && error.contains("never falls back"),
            "{error}"
        );
        drop(held);
        // Once free, exactly that port is bound.
        let (_listener, bound) = bind_plan("127.0.0.1", &plan).unwrap();
        assert_eq!(bound, port);
    }

    #[test]
    fn range_end_is_inclusive_and_listener_is_held() {
        // Take an ephemeral port, then ask for the single-port range it forms.
        let (probe, actual) = bind_in_range("127.0.0.1", 0, 0).unwrap();
        assert_ne!(actual, 0);
        drop(probe);

        let (held, chosen) = bind_in_range("127.0.0.1", actual, actual).unwrap();
        assert_eq!(chosen, actual);
        // While held, the same single-port range must report exhaustion.
        let err = bind_in_range("127.0.0.1", actual, actual).unwrap_err();
        assert!(err.contains(&format!("{actual}-{actual}")), "{err}");
        drop(held);
    }

    #[test]
    fn two_apps_keep_their_ports_whichever_starts_first() {
        // gfit: two apps on the default range swapped ports on restart.
        use super::{RememberedPorts, remember_ports_enabled};
        use connector_client::ports::{self, PortPlan};
        // A free 4-port range for this test.
        let (probe, base) = bind_in_range("127.0.0.1", 0, 0).unwrap();
        drop(probe);
        let base = base.min(u16::MAX - 4);
        let range = |preferred: Option<u16>| {
            let (plan, _) = ports::resolve_port(
                |_| None,
                ports::WS_PORT_VAR,
                ports::WS_RANGE_VAR,
                None,
                Some((base, base + 3)),
                ports::DEFAULT_WS_RANGE,
            )
            .unwrap();
            plan.with_preferred(preferred)
        };
        let dir_a = std::env::temp_dir().join(format!("tc-ports-a-{}", std::process::id()));
        let dir_b = std::env::temp_dir().join(format!("tc-ports-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
        // Start the app whose directory is `dir`, return its listener.
        let start = |dir: &std::path::Path| {
            let memory = RememberedPorts::load(dir);
            let plan = range(memory.ws);
            let (listener, port) = bind_plan("127.0.0.1", &plan).unwrap();
            memory.update(Some(port), &plan, None, &plan).save(dir);
            (listener, port)
        };
        // A single app keeps the range start, run after run.
        let (a, port_a) = start(&dir_a);
        assert_eq!(port_a, base);
        drop(a);
        let (a, again) = start(&dir_a);
        assert_eq!(again, base);
        // B starts while A runs: next port, remembered.
        let (b, port_b) = start(&dir_b);
        assert_eq!(port_b, base + 1);
        drop((a, b));
        // Restart in the other order: each app gets its own port back.
        let (b, b_again) = start(&dir_b);
        let (a, a_again) = start(&dir_a);
        assert_eq!((a_again, b_again), (base, base + 1));
        drop((a, b));
        // An exact port wins and is not remembered.
        let exact = ports::resolve_port(
            |name| (name == ports::WS_PORT_VAR).then(|| (base + 2).to_string()),
            ports::WS_PORT_VAR,
            ports::WS_RANGE_VAR,
            None,
            None,
            ports::DEFAULT_WS_RANGE,
        )
        .unwrap()
        .0
        .with_preferred(Some(base));
        assert_eq!(exact.preferred, None);
        assert_eq!(exact.candidates().collect::<Vec<_>>(), vec![base + 2]);
        let kept = RememberedPorts::load(&dir_a).update(Some(base + 2), &exact, None, &exact);
        assert_eq!(kept.ws, Some(base));
        // A remembered port outside the range is ignored.
        let plan: PortPlan = range(Some(base + 50));
        assert_eq!(plan.preferred, None);
        assert!(remember_ports_enabled(None) && !remember_ports_enabled(Some("0")));
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn reversed_range_is_rejected() {
        assert!(bind_in_range("127.0.0.1", 10, 9).is_err());
    }
}
