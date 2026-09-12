# U6 capture implementation and evidence

Owned implementation: `plugin/src/capture/{mod.rs,page.js,store.rs,redact.rs}`. Tests: `plugin/tests/capture.test.cjs`, `plugin/src/capture/{store_tests.rs,service_tests.rs}`. Shared wiring: `plugin/src/bridge.rs` replaces the previous monitoring wrapper with the idempotent shared hook; `plugin/js/ipc-monitor.js` activates the same hook for the legacy projection. The runtime/identity lane owns ingress commands, builder configuration, and page/window lifecycle integration in `lib.rs`.

## Verification commands

- `node --test plugin/tests/capture.test.cjs plugin/tests/monitor.test.cjs`: **27 passed**, including 25 capture tests and 2 existing legacy monitoring tests. Saved output: `capture-js-tests.log` (same directory).
- `cargo test -p tauri-plugin-connector --lib capture:: --no-default-features`: **15 passed**, no failures. Saved output: `capture-rust-tests.log` (same directory; final rerun).
- `git diff --check -- plugin/src/capture plugin/tests/capture.test.cjs plugin/js/ipc-monitor.js`: passed.
- `cargo clippy -p tauri-plugin-connector --all-features --tests -- -D warnings`: capture warnings were corrected. The latest lane run stopped on a then-concurrent `picker/mod.rs` error-code projection lint; its owner was informed. Earlier cross-lane collapsible-if warnings were corrected. Consult the final aggregate Clippy result, not this intermediate run, for overall status.

Initial JS tests failed against the absent implementation. Further regression tests reproduced a stop-after-start terminal leakage defect and missing drain barrier before fixes. Initial Rust capture tests passed 5 cases; expanded coverage now passes 12. Intermediate full-crate builds also encountered sibling modules under construction; those are not capture test passes.

## Implemented contract

One app-owned protected, non-persistent store supports at most 8 retained sessions. Data and indexes have an encoded-byte accounting cap of 16 MiB / 10,000 events and a bounded 10,000-entry invocation index; the store holds a 16 MiB reservation from the existing shared workflow retention budget. Multiple sessions reference the same retained events instead of cloning storage per session. Revocation removes protected sessions and releases the reservation when data becomes inaccessible. Active sessions have a monotonic ten-minute maximum lifetime (the host can shorten it). Page watchdogs and an application-owned maintenance task expire them without another client request; followPages preserves the original deadline. Stopped results are retained for 15 minutes and then automatically reaped, releasing the shared memory reservation when no retained evidence remains. No capture contents go to legacy IPC log files.

Capture management authenticates against the host workflow token generation, rechecks authorization at the bridge dispatch boundary and before returning data, and preserves authorization domains in opaque process-bound session IDs/cursors. Start and opt-in page following first recover existing workflow quarantine and acquire the trusted backend/window resources. Query and stop remain usable for diagnosis without releasing quarantine.

Page previews are sanitized before telemetry transport, then checked again on the host. Metadata is the default. Preview commands are enabled by host `ConnectorBuilder::capture_preview_commands` or comma-separated `TAURI_CONNECTOR_CAPTURE_PREVIEW_COMMANDS`. Strings require exact dotted allowlist paths from `capture_preview_paths` or `TAURI_CONNECTOR_CAPTURE_PREVIEW_PATHS`, for example `id,name,nested.safe`. Sensitive key names override allowlists. Unknown strings become type/length summaries; cyclic values, binary data, Maps/Sets, throwing accessors and `toJSON` do not replace application values or errors. Limits are 4096 UTF-8 bytes per preview, depth 4, 50 collection entries, 16 KiB per encoded event, 1000 page queued events and 4 MiB queued encoded bytes. At most one telemetry batch is in flight.

A bounded stop barrier drains already queued evidence before freezing the session end cursor. Unconfirmed drainage adds a gap. Stopping one session leaves another session's hook active, and the stopped session never observes its peer's later completion. Pending calls stay pending or become `observation_interrupted`; the observer never cancels business invocations. Navigation interrupts old sources and follows new pages only with `followPages:true`, with fresh context acknowledgement and explicit gaps. Readiness includes an excluded, no-business connector nonce roundtrip. Ownership checks preserve later wrappers.

The locked runtime requires an immutable-invoke adaptation. See `capture-immutable-invoke-adr.md`: actual transport callbacks supply frontend invoke observations while preserving the original immutable function and Promise. Source/coverage distinguish this from the direct wrapper and disclose pre-transport gaps.

## ID mapping and completion layers

All IDs below have implementation and the stated unit/JS/service evidence. `nativeVerified` and `ciVerified` remain pending in this lane document; the root native harness and aggregate CI records must supply those layers. The readonly-interface Node tests are not native proof.

| ID | Unit / service / JS evidence | Remaining native or broader integration evidence |
|---|---|---|
| UP-T081 | direct wrapper, imported/global forwarding, readonly transport callback tests, exact connector namespace filtering; one started/terminal pair and one original call | Native module/global invoke paths |
| UP-T082 | exact receiver, all arguments, additional options; native-interface header forwarding and no invoke-key disclosure | Native caller options/headers |
| UP-T083 | same exception object thrown synchronously by writable invoke wrapper; transport synchronous throws forwarded unchanged | No native synchronous invoke claim for immutable Promise-returning implementation |
| UP-T084 | success object and rejection identity on direct and readonly callback paths | Native invoke results |
| UP-T085 | unhandled rejection preserved in isolated Node child processes for both paths | Native WebView unhandled event |
| UP-T086 | cycle, BigInt, throwing getter and toJSON do not alter business success | Additional native value fixtures |
| UP-T087 | binary size summaries, untouched streams, overridden byteLength getter is not invoked | Native binary/stream behavior |
| UP-T088 | nested password/token/header removal before page transport; host second boundary; legacy file remains empty | Native sentinel scan |
| UP-T089 | huge UTF-8 result constrained to preview/event limits, full query response byte-budget test | Native large-object fixture |
| UP-T090 | 1200 synchronous business invokes stay successful under 1000-event queue saturation; drops increase | Native pressure run |
| UP-T091 | independent sessions; stop barrier; late completion visible only to still-active session | Multiple external clients against native fixture |
| UP-T092 | end-before-start, duplicate packet, replay after eviction, fetch-to-postMessage deduplication | Native reordered transport injection |
| UP-T093 | pending before completion; stop does not cancel application Promise; stopped pending remains interrupted | Native long/never-completing command |
| UP-T094 | foreign cursor rejected; expired cursor returns gap and available range | Native long-lived cursor rotation |
| UP-T095 | authenticated service query preserves v2 evidence through anonymous legacy clear; legacy file has no v2 event | Four-entry legacy compatibility |
| UP-T096 | own functions/callbacks restored; later wrappers preserved; legacy can reactivate owned hook | Native external wrapper coexistence |
| UP-T097 | context validation, fresh-source lifecycle, explicit follow opt-in, connector nonce readiness roundtrip | Native reload/window lifecycle |
| UP-T098 | hostile causal/action ID and persistence claims discarded; query/eviction retain quarantine | Native failed-write diagnosis |
| UP-T102 | anonymous rejected before dispatch; token-generation isolation; host revocation ack stops page hook | Cross-entry revocation timing |
| UP-T103 | capture eviction and legacy clear do not remove quarantine; shared retention reservation test | Existing workflow journal regression suite |
| UP-T104 | UTF-8 preview and complete query output respect byte budgets | Aggregate pagination integration |
| UP-T107 | owned hook/queue cleanup, stop barrier and revoked-memory release | App-exit and native fault cleanup |

## Current boundaries

No source claims business persistence or Rust internal tracing. Capture source metadata cannot authorize workflow replay or modify an immutable spec. Immutable Tauri failures before the observable transport are explicitly unobserved. Oversized postMessage fallback envelopes are skipped with drop accounting; no unbounded parsing is used. Session expiration is lazy on subsequent service activity, with fixed memory limits in between. Native/CI verification must be recorded per platform by the integration lane.
