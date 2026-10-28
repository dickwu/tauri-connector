//! Port ranges shared by the plugin's listeners and the CLI's discovery
//! scan, with environment overrides (0.17.6):
//!
//! | Listener  | Default    | Range variable                     | Start-port variable        |
//! |-----------|------------|------------------------------------|----------------------------|
//! | WebSocket | 9555-9655  | `TAURI_CONNECTOR_PORT_RANGE`       | `TAURI_CONNECTOR_PORT`     |
//! | MCP HTTP  | 9556-9656  | `TAURI_CONNECTOR_MCP_PORT_RANGE`   | `TAURI_CONNECTOR_MCP_PORT` |
//! | Bridge    | 9300-9400  | `TAURI_CONNECTOR_BRIDGE_PORT_RANGE`| -                          |
//!
//! A range is `START-END` (inclusive) or a single `PORT`. A start-port
//! variable means `PORT..=PORT+100`. Set in the app's environment, a
//! variable wins over `ConnectorBuilder::port_range`/`mcp_port_range`.

pub const DEFAULT_WS_RANGE: (u16, u16) = (9555, 9655);
pub const DEFAULT_MCP_RANGE: (u16, u16) = (9556, 9656);
pub const DEFAULT_BRIDGE_RANGE: (u16, u16) = (9300, 9400);

pub const WS_RANGE_VAR: &str = "TAURI_CONNECTOR_PORT_RANGE";
pub const WS_PORT_VAR: &str = "TAURI_CONNECTOR_PORT";
pub const MCP_RANGE_VAR: &str = "TAURI_CONNECTOR_MCP_PORT_RANGE";
pub const MCP_PORT_VAR: &str = "TAURI_CONNECTOR_MCP_PORT";
pub const BRIDGE_RANGE_VAR: &str = "TAURI_CONNECTOR_BRIDGE_PORT_RANGE";

/// Ports scanned above a start-port variable.
const START_PORT_SPAN: u16 = 100;

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
