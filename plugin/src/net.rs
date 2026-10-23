//! Port selection shared by the bridge, WebSocket and MCP listeners.

use std::net::TcpListener;

/// Bind the first free port in the inclusive range `start..=end` and keep the
/// listener, so no other process can take the port between selection and use.
///
/// The returned listener is non-blocking, ready for
/// `tokio::net::TcpListener::from_std`.
pub(crate) fn bind_in_range(
    addr: &str,
    start: u16,
    end: u16,
) -> Result<(TcpListener, u16), String> {
    if start > end {
        return Err(format!("Invalid port range {start}-{end}"));
    }
    let mut last_error = None;
    for port in start..=end {
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

#[cfg(test)]
mod tests {
    use super::bind_in_range;

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
    fn reversed_range_is_rejected() {
        assert!(bind_in_range("127.0.0.1", 10, 9).is_err());
    }
}
