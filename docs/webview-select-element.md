# Active element selection

`webview_select_element` activates a real selection UI in a target WebView. Move the pointer to highlight an element, then confirm the highlighted element with the primary pointer button or Enter. Escape and the visible cancel control cancel selection. It does not require Alt+Shift+Click or an injected user script. The existing `webview_get_pointed_element`/`pointed` feature remains separate.

A selection is an observation. It does not dispatch a connector business action, authorize a subsequent click, mutate a submitted workflow spec, erase its original verdict or release unknown-write quarantine. Previously registered application listeners, hover styling and background work can still have effects; the tool does not claim full isolation against an adversarial same-origin page.

## CLI

The host configures its existing workflow token; CLI reads `TAURI_CONNECTOR_WORKFLOW_TOKEN`. Tokens should not appear in shell command arguments or checked-in examples. Select the correct application with `--app-instance-id` or let strict discovery resolve one authenticated workspace match. `--window` aliases `--window-id`.

```bash
tauri-connector picker start --window main --request-key fixture-pick-001 --timeout-ms 60000
tauri-connector picker get PICKER_ID --wait-ms 10000
tauri-connector picker cancel PICKER_ID
tauri-connector select-element --window main --timeout-ms 60000
```

Replace `PICKER_ID` with the exact returned opaque handle. A new CLI invocation recovers its expected application instance from that handle and checks the live endpoint. Use `--host/--port` if the instance is outside the configured scan range. To avoid capture overhead, pass `--no-screenshot`; it conflicts with `--screenshot-source`. Source choices are `auto`, `webview_native`, `window_native` and `dom_rendering`.

The CLI generates a request key when omitted. It prints requestKey/pickerId recovery hints to stderr and reports JSON on stdout. Exit 0 means selected, or an accepted cancel request; selected with screenshot warnings still exits 0. Exit 2 means created/installing/awaiting selection. Exit 1 means an error, expiry, target change or failed/cancelled selection; a cancelled convenience request exits 1. A timeout or output error does not authorize another start with a new key.

## Embedded and independent MCP

Both MCP servers advertise the same `webview_select_element` schema. Sessions negotiating 2025-06-18 or 2025-11-25 receive structuredContent; legacy SSE/2025-03-26 receive the complete text report and optional image without that unnegotiated field. Embedded MCP accepts the trusted caller's `authToken` argument. Independent MCP uses its existing environment credential, negotiates inspection protocol v1, verifies the live application and forwards the same WS command.

Start:

```json
{"action":"start","windowId":"main","requestKey":"fixture-pick-001","timeoutMs":60000,"waitMs":0,"captureScreenshot":true,"screenshotSource":"auto"}
```

Query the existing object and include its already-redacted image when available:

```json
{"action":"get","pickerId":"PICKER_ID","waitMs":10000,"includeImage":true}
```

Cancel:

```json
{"action":"cancel","pickerId":"PICKER_ID"}
```

Omitting `action` creates a new selection and waits at most 10000 ms for a result revision. It may return installing/waiting with a valid ID rather than a selection. Repeated get calls never create another overlay or take another screenshot. MCP waiting/cancelled/expired reports are structured lifecycle data; invalid arguments, unauthorized access and failed installation are tool errors. `includeImage:true` adds a legal image content block beside the structured report; omission preserves metadata and gives a reason.

## Direct WebSocket

```json
{
  "id":"transport-request-1",
  "type":"inspection",
  "operation":"webview_select_element",
  "args":{
    "action":"start",
    "windowId":"main",
    "requestKey":"fixture-pick-001",
    "captureScreenshot":false
  }
}
```

Trusted caller code adds `args.authToken` before sending. For get/cancel, keep the same operation and use the MCP argument shapes above. The older `type:"select_element"` alias also reaches this service, including normalization of the prior `window_id` field; it still requires authorization. Unsupported older plugins are rejected, with no arbitrary execute-JS fallback.

## Request contract

| Field | Actions | Behavior |
|---|---|---|
| `action` | all | `start|get|cancel`; omitted means convenience start |
| `authToken` | all | Host authorization, not part of the selection fingerprint |
| `windowId` | start | Default `main`; get/cancel cannot reroute a handle |
| `requestKey` | start | Nonempty, at most 128 UTF-8 bytes; CLI generates one |
| `timeoutMs` | start | 5000–120000, default 60000; covers installation, input, image and result work |
| `timeout` | start | Milliseconds alias; both timeout values must agree |
| `waitMs` | start/get | 0–10000; explicit start/get default 0, convenience default 10000; does not renew deadline |
| `pickerId` | get/cancel | Required app-owned handle; forbidden on start |
| `captureScreenshot` | start | Default true; false schedules no screenshot |
| `screenshotSource` | start | Default auto; explicit backend failure does not silently fall back |
| `includeImage` | start/get | Default false; returns stored redacted bytes, never triggers capture |

Unknown fields and action-inappropriate fields are rejected before UI installation. `captureScreenshot:false` conflicts with an explicit screenshotSource or start includeImage:true. A get requesting an image from a noncapture session reports `not_requested`. There is no script, force, unsafe, output-path or arbitrary-coordinate selection parameter.

## Lifecycle, deduplication and cleanup

The primary states are `created → installing → awaiting_selection → selected`, with `cancelled`, `expired`, `target_changed` and `failed` terminal alternatives. Screenshot and cleanup each have separate states; `selected` does not imply the image or cleanup succeeded. Reports include revision, original deadline, `retainedUntil` and `resultComplete`.

The app reserves the request key, window slot and resource lease atomically before page activation. Same key and same normalized window/timeout/capture options return the existing handle without renewing the deadline or repeating capture. Changing waitMs/includeImage does not change that logical request; changing its actual options produces `request_key_conflict`. Deduplication lasts only for this app instance and the advertised retention period.

Limits are one active picker per window, four per app, and 128 retained records with an 8 MiB metadata budget. Unexpired results are not silently evicted to fit a new start. The default retention is five minutes. Capacity exhaustion rejects the new request and preserves existing sessions/workflow history.

External CLI/MCP/WS disconnect detaches the waiter; the host continues to own the picker until its original deadline. Reconnect and get can read the same object. Internal bridge disconnect, reload, window replacement or invalid origin makes the original context invalid. Old messages cannot move a picker to the new page. If navigation follows selection, the historical selection remains but a later new-page image cannot be attached to it.

Cancel is idempotent. A committed selection remains selected; cancel can stop pending image work and drive remaining cleanup. Deadline, cancel and selection compete through one terminal transition. The page watchdog bounds abandoned input interception. Teardown removes the owned overlay, input capture, listeners, observers, animation/timer work and retained page candidates. Confirmed cleanup permits lease release. `cleanup_unconfirmed` is an explicit warning with continuing interaction restrictions, not proof that normal interaction has resumed and not permission to clear business-write quarantine.

## Targets, candidates and images

Supported selection surfaces include visible light-DOM elements, disabled controls, text containers, images and opened HTML dialogs/popovers where the platform guard can safely cover them. Picker decorations are excluded from hit testing. An iframe is its host element; unsupported shadow internals are represented by their host boundary; a canvas is a canvas, not an inferred drawn control. Unsupported surfaces must be reported instead of manufacturing an inner target.

The report contains bounded redacted tag/role/name/description/states, geometry/context and up to five locator candidates. A candidate marked verified was uniquely resolved to the selected object/entity with the same semantic version. Its validity is the observed context only. No valid candidate is preferable to a fabricated first match; redacted/truncated identifiers are not usable selectors. These are independent of workflow v1's prohibition on bare refs.

Requested images follow the protected screenshot path: preserve the selected object/context, hide picker decorations while maintaining input tail protection, validate current geometry, capture the requested source, crop and mask, then recheck authorization before retaining bytes. No implicit scroll, expansion or focus improves framing. Metadata and pixels have separate observation times and do not claim atomicity. Failed capture leaves the selection fact intact with a screenshot warning; get never retries it or asks the user to click again.

MCP image extraction preserves native dimensions and the structured screenshot metadata. CLI/WS normally return references. The picker artifact handle can be read through authenticated `artifact_read`; CLI `artifacts show ARTIFACT_ID --base64` explicitly requests stored redacted bytes. Legacy pointed cache and file-artifact paths cannot read the protected result.

## Errors and verification boundary

| Code/status | Meaning |
|---|---|
| `invalid_arguments` | Malformed/action-inappropriate fields or conflicting options |
| `unauthorized` | Missing/revoked credentials or disallowed origin |
| `resource_busy` | Existing interaction, capacity or workflow/quarantine conflict |
| `request_key_conflict` | Existing key refers to different normalized options |
| `picker_not_found` / `picker_gone` | Unknown or no-longer-retained handle; never recreates UI |
| `picker_install_failed` | Trusted runtime/overlay/guard readiness failed |
| `input_isolation_unavailable` | Supported input isolation cannot be established |
| `target_changed` / `unsupported_surface` | Context invalidated or requested surface unsupported |
| `cleanup_unconfirmed` | Page cleanup could not be proved |
| `redaction_unavailable` / `capture_context_changed` | Image cannot safely accompany the selection |
| `expired` / `cancelled` | Lifecycle terminal state, not an empty successful selection |

The implementation includes desktop platform capture/selection adapters, but native input, images and business-counter isolation must be verified independently for macOS, Windows and Linux. Synthetic page events or fabricated selected messages are not native evidence. The [upgrade status matrix](upgrade-implementation-status.md) records each UP-T/UP-PK result, actual commands, browser/native separation and remaining platform boundaries.
