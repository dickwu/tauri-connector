# Inspection protocol v1 migration

This describes the v0.16.0 inspection upgrade. All four package versions are 0.16.0; the inspection, workflow, runtime, semantic and application package versions are independent. Actual platform verification is recorded in [upgrade implementation status](upgrade-implementation-status.md).

## Credentials and negotiation

The host configures its existing workflow token. CLI and standalone MCP read `TAURI_CONNECTOR_WORKFLOW_TOKEN`; embedded MCP and direct WS provide `authToken` in arguments, outside any workflow spec. The page receives no management token. Missing or revoked credentials reject identity, health, active picker, rich screenshots, capture v2 and protected artifact requests. Sharing a token shares one authorization domain; this does not implement independent users.

`bridge_status` adds only minimum inspection capability information to the existing anonymous status. `inspectionProtocolVersion: 1` must be advertised before a new client sends an inspection request. An old plugin returns `capability_unavailable`; clients never emulate inspection with arbitrary JavaScript. Existing legacy commands remain available against explicitly selected older endpoints. Strict authenticated discovery cannot establish instance identity for an older plugin; specify its endpoint for legacy-only operations or upgrade it.

## Application and handle routing

`--app-instance-id` (or `TAURI_CONNECTOR_APP_INSTANCE_ID`) selects the exact process instance. An explicit endpoint is checked against every accompanying app/instance/PID-file constraint. PID files are hints: the live PID, process-start stamp, app identifier and available instance identifier must agree. An explicitly missing PID file does not trigger a scan fallback. Without an explicit identity, canonical CWD containment is checked by filesystem path segments. Multiple matches produce `ambiguous_app`, never the newest process or first responding port.

Connections remain pinned across reconnects. A different process, even at the same port with the same app ID, cannot assume an earlier connection's inspection or workflow identity. A standalone MCP process must be restarted to deliberately establish an unrelated application session after it has bound an instance.

Picker, capture and new screenshot artifact handles contain opaque application-instance binding information. CLI lifecycle calls recover that expected instance and authenticate the live endpoint. The handle is not a credential. Existing workflow UUIDs remain unchanged. On supported private filesystems the CLI stores a bounded-size per-run identity record under its user cache after receiving a run report. The next get/resume/cancel uses that original instance and endpoint. If no trusted record is available, modern workflow lifecycle commands require `--app-instance-id` from the original report. On platforms without this private registry implementation, pass that explicit identity. Failure to retain the record is a warning and does not overwrite the original workflow result.

Modern CLI ref caches include host, application instance and window in their key. Modern refs carry semantic/page identity and call the shared semantic resolver without CSS or first-name fallback. Reload, replaced/reused nodes and semantic disposal invalidate these refs. Legacy global caches are not imported into a modern application context.

## Active element selection

`webview_select_element` now starts real active selection. Omitted `action` means start with a response wait of up to 10000 ms. Explicit `start` returns without waiting by default; `get` queries the same app-owned object; `cancel` is idempotent. `timeout` remains a milliseconds alias for `timeoutMs`; conflicting values are rejected. Get/cancel cannot change `windowId`. Unknown or action-inappropriate fields are rejected before UI installation.

The CLI exposes `picker start|get|cancel` and `select-element`. The CLI and SDK generate `requestKey` when omitted; a dispatched SDK request that loses its reply retains the key in the error report. The CLI prints recovery identifiers on stderr, with JSON on stdout. Exit 0 means selected or a processed cancellation; exit 2 means created/installing/waiting; exit 1 means errors or other unsuccessful terminal states. A user-cancelled convenience request exits 1. A selected result with a failed screenshot remains selected and may exit 0 with its warning.

Selection metadata, screenshot completion and cleanup confirmation are separate facts. MCP preserves the metadata report while optionally attaching a bounded, already-redacted image. structuredContent is emitted only after negotiating 2025-06-18 or 2025-11-25; legacy SSE/2025-03-26 retain text and image representations. Image omission does not discard image dimensions, context or locator candidates. `get` never takes another screenshot. The older Alt+Shift+Click pointed-element cache remains a separate legacy feature.

See [active picker usage](webview-select-element.md) and [inspection design](inspection-design.md). Neither selection nor diagnostics mutate an existing workflow spec, erase its original failure, clear unknown-write quarantine or authorize replay.

## Screenshots and protected artifacts

Rich WebSocket screenshot requests use `type: "inspection"`, `operation: "webview_screenshot"` and an `args` object. The old `type: "screenshot"` envelope rejects rich source, mask, or authorization fields before capture; it cannot silently discard those fields and take an unmasked legacy screenshot.

Absent rich fields, `webview_screenshot` follows the legacy source path. Explicit `source` selects `webview_native`, `window_native` or `dom_rendering`; only `auto` allows fallback. Supplying `target` and `selector` together is invalid. Rich requests require authorization and required redaction; the current host policy rejects window preparation or a redaction downgrade. Protected capture masks a lossless PNG in controlled memory; rich screenshot requests can then encode JPEG/WebP. Picker captures remain PNG. Private disk storage may be unavailable; results then use a bounded, expiring protected memory store and report save warnings rather than silently using public temporary files.

CLI rich screenshots return metadata and an artifact reference by default. An explicit output filename exports the redacted image; giant base64 data is not printed by default. Protected artifact read/compare routes are selected by their new handle format; explicit `authToken` selects protected storage for MCP list/prune as well. Protected compare checks capture-contract compatibility before reporting encoded-byte equality. It is not perceptual diff and does not accept a nonzero CLI threshold. Protected pruning removes expired artifacts only, preserving announced retention. Legacy `shot_` artifacts and path-based operations retain their separate existing behavior and cannot locate protected in-memory images.

Native backend code, build verification, browser fixture evidence and native desktop evidence are distinct. Consult the status matrix before asserting a platform was tested.

## IPC capture v2

The existing `connector:default` capability now explicitly declares its six legacy page-data callbacks and the bounded `capture_self_test` diagnostic and `push_capture_events` ingress commands. The ingress validates the actual WebView, registered source/context and active host authorization; this does not grant business commands or protected result reads. Rebuild the app to load the updated permission manifest.

`ipc_capture` manages independent `start/status/stop` sessions; `ipc_query` returns bounded event pages, completion phases and pending invocations. Defaults expose metadata. Preview requires host command and path allowlists; a client request alone cannot permit payload disclosure. Module/global invoke coverage and gaps are reported as observed, not as Rust internal tracing or database durability evidence.

CLI paths are `ipc capture start|status|stop` and `ipc query`. IPC pagination uses the opaque string `nextCursor`; pass it unchanged through `--cursor` or the MCP `cursor` field. It is bound to the original session/process and must not be converted to an integer. `resultPolicy` and `argumentPolicy` accept `metadata|preview`; `followPages` defaults false. Legacy monitor/clear commands do not clear v2 session buffers or reveal protected results. One session stopping does not stop other sessions' shared hook.

## Direct WS and MCP

All new operations use one canonical WS envelope:

```json
{"id":"request-1","type":"inspection","operation":"webview_select_element","args":{"action":"start","windowId":"main","requestKey":"fixture-pick-001","captureScreenshot":false}}
```

Trusted transport code adds the configured `authToken` to `args`; it is deliberately absent from this checked-in example. Embedded MCP invokes the same host service. Standalone MCP forwards the same envelope through `ConnectorClient::inspect`, which verifies capability and identity. The legacy `type:"select_element"` alias also reaches the same service and preserves its former `window_id` alias; it does not bypass authentication. Workflow envelopes and immutable workflow v1 specs remain unchanged.
