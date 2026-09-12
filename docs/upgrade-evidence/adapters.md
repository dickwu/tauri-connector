# Client routing and four-entry adapter evidence

Date: 2026-09-12. This evidence concerns the client/CLI/MCP/WS adapter slice. It does not claim native desktop input or screenshot verification.

## Implemented changes

- `connector-client` adds typed normalized picker requests, canonical request fingerprints, exact action-field validation, bounded static inspection locators, shared result/exit/image conversion, and negotiated MCP result shaping.
- Authenticated identity discovery validates explicit app/instance/PID-file constraints, process start and canonical workspace containment. It collects all matching candidates and reports ambiguity with minimum identity summaries; no newest/first/default fallback. The endpoint host is part of connection/cache identity.
- Connections pin their expected application instance, retain that binding through reconnect and reject a changed process before inspection/workflow use. Picker/capture/artifact handles carry instance bindings. CLI retains original workflow UUID→instance/endpoint hints in private, bounded files; missing modern run mappings require an explicit original instance.
- Modern CLI refs preserve opaque semantic identity, page epoch and semantic version and do not fall back after stale resolution. Host/instance/window keys cannot collide merely through punctuation normalization.
- CLI exposes picker start/get/cancel and convenience selection, runtime health, identity, IPC v2 sessions/query, rich screenshot source/target/redaction flags and explicit redacted image export. Request keys are generated; status and recovery hints use stderr; results use JSON stdout. Accepted cancellation preserves the prior terminal state and exits 0.
- Embedded MCP and direct WS use one `inspection_call` host dispatch/auth boundary. Standalone MCP and CLI forward the same inspection envelope after capability and live identity checks. The old `select_element` WS alias now reaches the actual picker service.
- Protected artifact identifiers route to the protected screenshot/picker stores. Legacy aliases cannot conflict silently. Image bytes are optional MCP image blocks; source/context/dimensions remain in metadata. Legacy SSE/2025-03-26 sessions retain text+image without unnegotiated structuredContent.
- Canonical MCP schema is byte-identically vendored into the published standalone crate. Picker example fixtures are self-contained in the client package and parity-tested against checked-in workspace examples.
- README, inspection design, picker usage and migration documents describe the actual contracts and deliberate compatibility changes.

## Commands and actual results

| Command | Result | Evidence |
|---|---|---|
| `cargo test -p connector-client --test inspection_contract --locked` before the picker contract module existed | Expected RED: unresolved `connector_client::inspection` import | Captured tool output at the beginning of this implementation; superseded by passing contract tests |
| `cargo test -p connector-client --test inspection_contract mcp_only_emits --locked --quiet` before negotiated shaping existed | Expected RED: missing `shape_mcp_result` | Captured tool output before implementing protocol shaping |
| `cargo test -p connector-cli -p connector-mcp-server -p connector-client --locked --quiet` | PASS: 149 tests across 8 nonempty suites; 0 failures | `adapters-core.log` |
| `cargo test -p tauri-plugin-connector --lib --locked --quiet` | PASS at this checkpoint: 144 library tests; 0 failures | `adapters-plugin.log` |
| `cargo test -p tauri-plugin-connector picker_inspection_alias --locked --quiet` | PASS: shared WS/alias/embedded validation and authorization | `adapters-host.log` |
| `cargo test -p tauri-plugin-connector negotiated_tool_content --locked --quiet` | PASS: HTTP old/new negotiation and legacy message endpoint | Subsequent targeted output; also in full plugin run |
| `cargo test -p connector-mcp-server --bin tauri-connector-mcp --locked --quiet` | PASS: negotiated standalone tool shaping | Also in core log |
| `cargo clippy -p connector-client -p connector-cli -p connector-mcp-server --all-targets --locked -- -D warnings` | PASS at recorded checkpoint | `adapters-clippy.log` |

The number of Rust tests is not the number of UP-T/UP-PK acceptance requirements. Root integration/native/CI runs may add further tests after this checkpoint. One earlier full plugin run exposed the concurrently introduced `up_pk033_late_capture_never_registers_after_monotonic_deadline` regression; the later full plugin log passes after its owner fixed the deadline check.

## Acceptance mapping for this slice

| ID | Evidence and actual coverage | Remaining verification outside this slice |
|---|---|---|
| UP-T025 | `discovery_identity::explicit_identity_and_stale_pid_are_rejected` passes | Multi-process native routing |
| UP-T026 | `matching_handshake_forwards_exact_inspection_and_rejects_instance_switch` uses real local WS frames and rejects another instance before another picker command | Native process replacement |
| UP-T027 | Scan uses the same tested identity validator and canonical workspace predicate | Full scan/filter fixture scenario not executed here |
| UP-T028 | `ambiguous_candidates_never_choose_recent_or_first` passes; errors include minimum candidates | Multiple native worktrees |
| UP-T029 | `canonical_workspace_boundaries_and_symlinks` passes for app/apple, child containment, missing paths and Unix symlinks | Windows filesystem casing and native monorepo matrix |
| UP-T030 | `modern_ref_cache_is_bound_to_host_instance_and_window` passes, including punctuation-distinct hosts | Two actual remote hosts |
| UP-T031 | Unit rejection of stale PID, startup timestamp and instance identity passes | Actual PID/port reuse fault injection |
| UP-T032 | Live-WS client binding test passes; persisted CLI workflow identity registry and strict missing-binding behavior implemented | Fresh-process CLI workflow recovery against a replacement native app |
| UP-T036 | Shared host adapter test rejects anonymous app_identity and runtime_health | Runtime owner's anonymous status field audit and native probes |
| UP-T101 | Shared host dispatch tested through direct inspection WS, old WS alias and embedded MCP; standalone forwarding uses real WS fixture | One real selected object queried across all four entries |
| UP-T102 | Host adapter auth test covers picker, identity, health, capture/query, screenshot and protected artifact read | Services' revoke-during-action tests |
| UP-T108 | `shipped_picker_examples_parse_as_the_actual_contract` and CLI help parsing pass; published fixture parity enforced | All CLI examples against native fixture |
| UP-PK002 | `picker_defaults_and_bounded_action_specific_fields` passes conflicts, action-only fields, timeout bounds/alias, UTF-8 requestKey, unsupported fields and false capture conflicts | None for the covered parser contract |
| UP-PK003 | Real host dispatch replaces placeholder; canonical schema/vendor parity and forwarding tests pass | Native selection result through adapters |
| UP-PK004 | No client/session-owned picker state; forwarding and shared host authorization/validation pass | Cross-entry selected handle/context/revision scenario |
| UP-PK005 | CLI parser tests and exit classifier cover start/get/cancel/convenience, waiting, failed image, accepted cancel of earlier failed/expired/changed states | CLI subprocess stdout/stderr/exit against running fixture |
| UP-PK006 | `picker_mcp_preserves_metadata_and_only_redacted_images`, native dimension metadata and negotiated protocol shaping tests pass | Actual native image through both MCP transports |
| UP-PK008 | `picker_fingerprint_excludes_wait_image_and_token` passes | Host concurrent same-key request arbitration |
| UP-PK035 | Modern stale refs do not fall back; protected IDs avoid legacy storage; host auth boundary tested | Root picker/workflow/quarantine and legacy artifact bypass regressions |
| UP-PK036 | Docs/help/schema/example parity implemented and checked in core suite | OS-native evidence and required CI execution belong to final matrix |

## Boundaries

No publication, package version change, remote push or business-data operation was performed by this slice. No native Windows/Linux behavior is asserted here. Same-token callers share one authorization domain. Legacy-only access to an older plugin requires an explicit endpoint because the plugin cannot provide the new authenticated process identity. The private CLI workflow registry currently requires Unix-style private filesystem permissions; other platforms can pass the original `--app-instance-id`. The registry does not recreate or mutate workflow history. All evidence logs remove developer-machine absolute workspace paths.

## Running-app entry-point helper

`examples/workflow-fixture/scripts/entry-points.mjs` exports `verifyEntryPoints({rpc,token,root,output})` for the existing isolated native harness. It uses real `target/debug/tauri-connector` and `tauri-connector-mcp` subprocesses, embedded HTTP MCP and direct WS against the same running fixture. It asserts CLI exits 2/0/1 and JSON/stderr separation, same-handle/context sharing, authorization rejection, older negotiated MCP text output, convenience activation and cleanup, and unchanged independent native business counters. It never synthesizes a selected event. `nativeUserSelectionPerformed:false` distinguishes its adapter coverage from native input evidence.

`node --check examples/workflow-fixture/scripts/entry-points.mjs` and `cargo build -p connector-cli -p connector-mcp-server --locked --quiet` passed. Actual running-app execution is left to the root native harness so parallel agents do not launch competing fixture processes; its result is written to the harness output `entry-points-results.json` and must be recorded separately when executed.

Final source checks: `git diff --check`, byte comparison of canonical/vendored MCP schema, and `cargo fmt -p connector-client -p connector-cli -p connector-mcp-server -- --check` passed. After packaging the example fixtures locally, the 13 inspection contract tests and standalone negotiated-protocol test passed again.

The final core refresh also passes `sdk_generates_one_request_key_and_retains_it_after_a_lost_response`: the real WS fixture observes one generated request key, closes without replying, and the SDK returns that recovery key with the original unknown outcome without dispatch replay. `cargo test -p tauri-plugin-connector screenshot_rich_schema --locked --quiet` now passes after the capture lane completed its concurrent RED tests (`adapters-screenshot-schema.log`). This asserts conditional rich schema constraints and runs ten vectors through the real host screenshot parser, while preserving legacy quality zero/numeric schema behavior. CLI/standalone binaries were rebuilt successfully after these changes.
