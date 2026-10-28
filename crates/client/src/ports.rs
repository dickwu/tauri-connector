//! Port selection shared by the plugin's listeners and the CLI's discovery
//! scan, with environment overrides:
//!
//! | Listener  | Default    | Exact port variable (0.17.7)  | Range variable                      |
//! |-----------|------------|-------------------------------|-------------------------------------|
//! | WebSocket | 9555-9655  | `TAURI_CONNECTOR_PORT`        | `TAURI_CONNECTOR_PORT_RANGE`        |
//! | MCP HTTP  | 9556-9656  | `TAURI_CONNECTOR_MCP_PORT`    | `TAURI_CONNECTOR_MCP_PORT_RANGE`    |
//! | Bridge    | 9300-9400  | `TAURI_CONNECTOR_BRIDGE_PORT` | `TAURI_CONNECTOR_BRIDGE_PORT_RANGE` |
//!
//! Precedence per listener: `ConnectorBuilder::port`/`mcp_port`/
//! `bridge_port` (exact), then the exact variable, then the range variable,
//! then `ConnectorBuilder::port_range`/`mcp_port_range`, then the default.
//! An exact port binds that port only: when it is busy the listener fails
//! to start and says so, with no fallback to a range. A range is
//! `START-END` (inclusive) or a single `PORT`, and takes the first free port.
//! (0.17.6 read `TAURI_CONNECTOR_PORT`/`_MCP_PORT` as `PORT..=PORT+100`.)

pub const DEFAULT_WS_RANGE: (u16, u16) = (9555, 9655);
pub const DEFAULT_MCP_RANGE: (u16, u16) = (9556, 9656);
pub const DEFAULT_BRIDGE_RANGE: (u16, u16) = (9300, 9400);

pub const WS_RANGE_VAR: &str = "TAURI_CONNECTOR_PORT_RANGE";
pub const WS_PORT_VAR: &str = "TAURI_CONNECTOR_PORT";
pub const MCP_RANGE_VAR: &str = "TAURI_CONNECTOR_MCP_PORT_RANGE";
pub const MCP_PORT_VAR: &str = "TAURI_CONNECTOR_MCP_PORT";
pub const BRIDGE_RANGE_VAR: &str = "TAURI_CONNECTOR_BRIDGE_PORT_RANGE";
pub const BRIDGE_PORT_VAR: &str = "TAURI_CONNECTOR_BRIDGE_PORT";

/// Ports scanned above a start port by [`range_from`]'s `start_var`.
const START_PORT_SPAN: u16 = 100;

/// How one listener picks its port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortPlan {
    pub start: u16,
    pub end: u16,
    /// Where an exact port came from (a variable name or
    /// `ConnectorBuilder`); `None` for a range.
    pub exact: Option<String>,
}

impl PortPlan {
    fn range((start, end): (u16, u16)) -> Self {
        Self {
            start,
            end,
            exact: None,
        }
    }

    fn exact(port: u16, source: &str) -> Self {
        Self {
            start: port,
            end: port,
            exact: Some(source.to_string()),
        }
    }

    /// `9700 (exact, TAURI_CONNECTOR_PORT)` or `9555-9655`.
    pub fn describe(&self) -> String {
        match &self.exact {
            Some(source) => format!("{} (exact, {source})", self.start),
            None => format!("{}-{}", self.start, self.end),
        }
    }

    /// The bind error, naming the exact-port rule when one applies.
    pub fn bind_error(&self, error: &str) -> String {
        match &self.exact {
            Some(source) => format!(
                "port {} set by {source} is unavailable ({error}); an exact port never falls back to a range: free the port, pick another, or unset {source} to scan a range",
                self.start
            ),
            None => error.to_string(),
        }
    }
}

/// An exact port variable: a single nonzero port.
fn exact_port(name: &str, text: &str) -> Result<u16, String> {
    match parse_port_range(text) {
        Ok((start, end)) if start == end => Ok(start),
        Ok(_) => Err(format!(
            "{name}={text}: expected one port (exact); use {name}_RANGE for a range"
        )),
        Err(error) => Err(format!("{name}={text}: {error}")),
    }
}

/// Resolve one listener's port: `builder_exact`, then `exact_var`, then
/// `range_var`, then `builder_range`, then `default`. `Err`: the exact
/// variable is invalid (the listener must not start on a guess). An invalid
/// range variable is returned in the second field and skipped.
pub fn resolve_port(
    lookup: impl Fn(&str) -> Option<String>,
    exact_var: &str,
    range_var: &str,
    builder_exact: Option<u16>,
    builder_range: Option<(u16, u16)>,
    default: (u16, u16),
) -> Result<(PortPlan, Option<String>), String> {
    if let Some(port) = builder_exact.filter(|p| *p != 0) {
        return Ok((PortPlan::exact(port, "ConnectorBuilder"), None));
    }
    let set = |name: &str| lookup(name).filter(|v| !v.trim().is_empty());
    if let Some(text) = set(exact_var) {
        return Ok((
            PortPlan::exact(exact_port(exact_var, &text)?, exact_var),
            None,
        ));
    }
    let fallback = builder_range.unwrap_or(default);
    match range_from(&lookup, range_var, None) {
        Ok(range) => Ok((PortPlan::range(range.unwrap_or(fallback)), None)),
        Err(error) => Ok((PortPlan::range(fallback), Some(error))),
    }
}

/// [`resolve_port`] against the process environment.
pub fn env_port(
    exact_var: &str,
    range_var: &str,
    builder_exact: Option<u16>,
    builder_range: Option<(u16, u16)>,
    default: (u16, u16),
) -> Result<(PortPlan, Option<String>), String> {
    resolve_port(
        |name| std::env::var(name).ok(),
        exact_var,
        range_var,
        builder_exact,
        builder_range,
        default,
    )
}

/// An exact port variable from the process environment, if set and valid.
pub fn env_exact_port(exact_var: &str) -> Option<u16> {
    std::env::var(exact_var)
        .ok()
        .and_then(|text| exact_port(exact_var, &text).ok())
}

/// Parse `START-END` or `PORT` into an inclusive, nonzero range.
pub fn parse_port_range(text: &str) -> Result<(u16, u16), String> {
    let port = |s: &str| {
        s.trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| format!("invalid port '{}'", s.trim()))
    };
    let (start, end) = match text.split_once('-') {
        Some((start, end)) => (port(start)?, port(end)?),
        None => {
            let p = port(text)?;
            (p, p)
        }
    };
    if start > end {
        return Err(format!("port range {start}-{end} is reversed"));
    }
    Ok((start, end))
}

/// The range from `range_var`, else `start_var` (as `PORT..=PORT+100`),
/// using `lookup` to read variables. `Ok(None)`: neither is set.
pub fn range_from(
    lookup: impl Fn(&str) -> Option<String>,
    range_var: &str,
    start_var: Option<&str>,
) -> Result<Option<(u16, u16)>, String> {
    let set = |name: &str| lookup(name).filter(|v| !v.trim().is_empty());
    if let Some(text) = set(range_var) {
        return parse_port_range(&text)
            .map(Some)
            .map_err(|e| format!("{range_var}={text}: {e}"));
    }
    if let Some(name) = start_var {
        if let Some(text) = set(name) {
            let (start, _) = parse_port_range(&text)
                .ok()
                .filter(|(s, e)| s == e)
                .ok_or_else(|| format!("{name}={text}: expected a single nonzero port"))?;
            return Ok(Some((start, start.saturating_add(START_PORT_SPAN))));
        }
    }
    Ok(None)
}

/// The range from the process environment, or `default`. An invalid value
/// is reported in the second field and the default is used.
pub fn env_range(
    range_var: &str,
    start_var: Option<&str>,
    default: (u16, u16),
) -> ((u16, u16), Option<String>) {
    match range_from(|name| std::env::var(name).ok(), range_var, start_var) {
        Ok(range) => (range.unwrap_or(default), None),
        Err(error) => (default, Some(error)),
    }
}

/// The CLI's discovery scan: the WebSocket range variable, else the default.
/// (`TAURI_CONNECTOR_PORT` alone names one endpoint and skips the scan.)
pub fn ws_scan_range() -> std::ops::RangeInclusive<u16> {
    let ((start, end), _) = env_range(WS_RANGE_VAR, None, DEFAULT_WS_RANGE);
    start..=end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn exact_ports_follow_builder_env_range_default_precedence() {
        let plan = |vars: &[(&str, &str)], exact: Option<u16>, range: Option<(u16, u16)>| {
            resolve_port(
                env(vars),
                WS_PORT_VAR,
                WS_RANGE_VAR,
                exact,
                range,
                DEFAULT_WS_RANGE,
            )
        };
        // Builder exact beats everything.
        let (p, _) = plan(&[(WS_PORT_VAR, "9701")], Some(9800), None).unwrap();
        assert_eq!(
            (p.start, p.end, p.exact.as_deref()),
            (9800, 9800, Some("ConnectorBuilder"))
        );
        // Env exact beats env range and builder range: exactly that port.
        let vars = [(WS_PORT_VAR, "9701"), (WS_RANGE_VAR, "9900-9910")];
        let (p, _) = plan(&vars, None, Some((9600, 9610))).unwrap();
        assert_eq!(
            (p.start, p.end, p.exact.as_deref()),
            (9701, 9701, Some(WS_PORT_VAR))
        );
        assert!(p.describe().contains("exact"));
        // Env range beats builder range; builder range beats the default.
        let (p, _) = plan(&[(WS_RANGE_VAR, "9900-9910")], None, Some((9600, 9610))).unwrap();
        assert_eq!((p.start, p.end, p.exact), (9900, 9910, None));
        let (p, _) = plan(&[], None, Some((9600, 9610))).unwrap();
        assert_eq!((p.start, p.end), (9600, 9610));
        let (p, _) = plan(&[], None, None).unwrap();
        assert_eq!((p.start, p.end), DEFAULT_WS_RANGE);
        // An invalid exact port is an error, never a silent fallback.
        assert!(plan(&[(WS_PORT_VAR, "9700-9800")], None, None)
            .unwrap_err()
            .contains("_RANGE"));
        assert!(plan(&[(WS_PORT_VAR, "x")], None, None).is_err());
        // An invalid range is reported and skipped.
        let (p, warning) = plan(&[(WS_RANGE_VAR, "9910-9900")], None, None).unwrap();
        assert_eq!((p.start, p.end), DEFAULT_WS_RANGE);
        assert!(warning.unwrap().contains(WS_RANGE_VAR));
        // A busy exact port names its source and the no-fallback rule.
        let exact = PortPlan::exact(9701, WS_PORT_VAR);
        let message = exact.bind_error("Address in use");
        assert!(message.contains("9701") && message.contains(WS_PORT_VAR));
        assert!(message.contains("never falls back"));
    }

    #[test]
    fn ranges_and_single_ports_parse_inclusively() {
        assert_eq!(parse_port_range("9700-9710").unwrap(), (9700, 9710));
        assert_eq!(parse_port_range(" 9700 ").unwrap(), (9700, 9700));
        assert!(parse_port_range("9710-9700").is_err());
        assert!(parse_port_range("0").is_err());
        assert!(parse_port_range("70000").is_err());
        assert!(parse_port_range("abc").is_err());
    }

    #[test]
    fn range_variables_win_over_start_ports_and_defaults() {
        let both = [(WS_RANGE_VAR, "9800-9810"), (WS_PORT_VAR, "9700")];
        assert_eq!(
            range_from(env(&both), WS_RANGE_VAR, Some(WS_PORT_VAR)).unwrap(),
            Some((9800, 9810))
        );
        let start = [(MCP_PORT_VAR, "9701")];
        assert_eq!(
            range_from(env(&start), MCP_RANGE_VAR, Some(MCP_PORT_VAR)).unwrap(),
            Some((9701, 9801))
        );
        assert_eq!(range_from(env(&[]), BRIDGE_RANGE_VAR, None).unwrap(), None);
        let edge = [(WS_PORT_VAR, "65500")];
        assert_eq!(
            range_from(env(&edge), WS_RANGE_VAR, Some(WS_PORT_VAR)).unwrap(),
            Some((65500, 65535))
        );
        let bad = [(BRIDGE_RANGE_VAR, "9400-9300")];
        let error = range_from(env(&bad), BRIDGE_RANGE_VAR, None).unwrap_err();
        assert!(error.contains(BRIDGE_RANGE_VAR), "{error}");
        let not_single = [(WS_PORT_VAR, "9700-9800")];
        assert!(range_from(env(&not_single), WS_RANGE_VAR, Some(WS_PORT_VAR)).is_err());
    }
}
