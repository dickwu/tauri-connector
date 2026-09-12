# Inspection protocol v1 implementation

This is the current unreleased upgrade design. Existing workflow protocol v1, immutable specs, runKey fingerprints, outcomes, journal and quarantine stay in place. See [upgrade status](upgrade-implementation-status.md) for implementation and verification levels; examples describe interfaces, not native-test evidence.

## One host service across four entries

```text
CLI ─┐
     ├─ ConnectorClient::inspect ─ WS inspection envelope ─┐
MCP ─┘                                                    │
Direct WS ────────────────────────────────────────────────┤
Embedded MCP ────────────────────────────────────────────┤
                                                         ▼
                                        mcp_tools::inspection_call
                                             host authorization
                                             fixed app context
                                   picker / capture / screenshot / health
```

The independent MCP process and CLI do not hold picker/capture lifecycle state. They add the existing environment credential, verify `inspectionProtocolVersion:1`, verify the live application identity and forward one command. An unsuccessful send, timeout or disconnect never causes automatic replay. MCP text/image envelopes are produced only at the adapter boundary; service results remain JSON values.

Canonical external WS request:

```json
{"id":"inspection-request-1","type":"inspection","operation":"runtime_health","args":{"windowId":"main","depth":"runtime","timeoutMs":2000}}
```

A trusted caller inserts its host token as `args.authToken`. The operations are `app_identity`, `runtime_health`, `webview_select_element`, `webview_screenshot`, `ipc_capture`, `ipc_query`, and protected `artifact_read|list|compare|prune`. The existing `select_element` WS alias normalizes `window_id` and reaches the same picker service. Workflow envelopes continue to use `type:"workflow"`.

## Identity and runtime

The plugin owns a random process `appInstanceId` and a common process-start timestamp for authenticated identity and PID-file metadata. It owns a separate `windowInstanceId` for each window lifecycle. Runtime context carries application/window identity, `pageEpoch`, `runtimeId`, runtime/semantic version and bundle hash. A same-label replacement window or same-URL reload is a new context.

Local PID files suggest endpoints. Discovery verifies their PID/start stamp/app identity against the actual endpoint, filters explicit constraints and canonical workspace path segments, and refuses ambiguity. A connection's expected instance persists through reconnect. Modern picker/capture/artifact handles encode opaque instance binding information; the handle alone does not authorize access. Workflow UUIDs retain the private CLI recovery mapping described in [migration](upgrade-migration.md).

Runtime installation is distinct from invocation. The host manager sends trusted local modules during installation, caches the acknowledged full context, and issues bounded declarative module calls after readiness. Ordinary runtime errors do not authorize replay. The semantic module provides shared role/name/description/state and strict locator resolution; modern refs include opaque object identity plus page and semantic versions and have no fallback after invalidation.

Health performs bounded transport/bridge/runtime probes without installing missing runtime, resetting the application, collecting a full snapshot or changing a prior operation's outcome. Each layer reports its own responsiveness or unavailability. A successful probe does not prove an earlier write was unexecuted; a timeout does not prove a deadlock.

## Authorization and ownership

Rich APIs validate the configured workflow token at the host boundary and recheck its authorization generation before returning data. Individual services additionally recheck at dispatch/capture/output boundaries. Revocation prevents retained metadata/image disclosure and drives service cleanup. Tokens are not sent to page code or included in selection fingerprints.

Resource classification is application-owned. Picker owns a long-lived UI interaction lease; screenshot preparation and IPC hook installation acquire their actual resource class. Query/cancel and bounded health paths remain available where permitted. Services never clear business-write quarantine to make a new observation possible. Multiple clients sharing a token are members of one authorization domain, not isolated users.

## Active picker

The complete public lifecycle and candidate/image semantics are documented in [webview-select-element](webview-select-element.md). Host state atomically reserves the request key, window slot, quota and UI lease before activating the trusted page module. Page UI, guarded input, hit testing and semantic candidate verification produce one bounded selection. The host verifies the actual runtime context, retains the observation and independently tracks screenshot/cleanup completion.

An external caller disconnect detaches that caller; the application retains the original deadline and result. Page disconnect/navigation/window destruction invalidates the original page context. Cancel, deadline and selected events compete through one primary transition, while late image completion cannot replace the primary terminal result. Cleanup confirmation controls resource release. An unconfirmed cleanup is reported separately and does not erase the independent workflow quarantine.

## Protected images

`webview_native` uses the platform WebView capture backend; `window_native` uses the window capture path; `dom_rendering` reconstructs pixels from DOM. Explicit sources do not switch backends. `auto` may perform bounded fallback and reports the actual source/reasons. Browser-generated images are not evidence of OS occlusion.

Protected capture checks context and geometry, crops the visible target region, masks sensitive regions before exporting bytes, and records source/coordinate/mapping metadata. A target must still identify the same connected object/entity. Unknown geometry or unavailable required masks rejects the image. Preparation does not scroll, focus, restore or resize the window under current passive host policy.

Rich screenshot artifacts reside in a bounded authorization-checked memory store (128 artifacts, 32 MiB, five-minute retention). Picker images are retained with their picker record and exposed by the same authority. New artifact handles never resolve through the legacy file manifest. Durable-save failure is an explicit warning; it cannot silently create an unprotected temporary file. Metadata, pixel capture and later reads are separate observations with separate times/context.

MCP emits metadata as text and, for negotiated 2025-06-18 or 2025-11-25 sessions, structured content. Legacy SSE/2025-03-26 sessions retain text and optional image blocks without the unnegotiated structuredContent field. Image bytes are separate optional image content and removed from JSON duplicates. Image-budget omission preserves dimension/source/context metadata and a reason. CLI defaults to artifact metadata and can explicitly export a redacted screenshot or request protected artifact bytes.

## IPC capture v2

`ipc_capture start` selects a window and options, confirms the shared invoke hook's applied state, and creates an independent session. `status` reports desired/applied coverage; `stop` changes only that session. `ipc_query` pages `started|succeeded|failed` records, pending invocations, sequence/gap/drop metadata and bounded redacted previews. A buffer clear in the legacy monitor path is independent of these sessions.

The hook preserves business receiver, arguments, options, return values and throw/rejection behavior for supported invocation paths. It does not invent coverage for unsupported transports or pre-install calls. The platform fixture must verify actual module/global invocation interception; browser substitutes alone cannot establish native coverage. Current verified transport coverage belongs in the acceptance status, including any immutable native invoke-property limitations.

Default `argumentPolicy` and `resultPolicy` are `metadata`; `preview` additionally requires host command/path allowlists. The host environment controls `TAURI_CONNECTOR_CAPTURE_PREVIEW_COMMANDS` and `TAURI_CONNECTOR_CAPTURE_PREVIEW_PATHS`. Pages do not choose their own disclosure policy. These are invoke observations, not automatic Rust internal traces, causal chains or database durability receipts.

## Evidence and compatibility

Contract tests cover strict picker arguments/defaults/fingerprints, exit and MCP result semantics, instance mismatch, canonical path boundaries and live WebSocket capability negotiation. Host adapter tests exercise identical authorization/validation through direct WS, the compatibility alias and embedded MCP. Schema copies are compared byte-for-byte; CLI help parsing and shipped example parsing are executable tests.

Native active input, native screenshot rendering and invoke interception must be verified separately on macOS, Windows and Linux. This design document does not mark those tests passed. [The status matrix](upgrade-implementation-status.md) records the actual commands, outcomes and remaining boundaries, including private Windows storage restrictions.
