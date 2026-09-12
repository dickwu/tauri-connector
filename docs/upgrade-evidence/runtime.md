# U1 runtime and application-side U2 evidence

Working baseline: `fb43c4ac11f04813b2a5038f52223fe9bcb246ed` (2026-09-12 implementation session). This report records this lane's verification; the consolidated matrix records integration and platform evidence separately.

## Implementation

- Static manifest embeds bootstrap, semantic, workflow, picker, IPC capture, and geometry modules, with dependency validation, explicit versions, consistency hashes, and disposer contracts. FNV-1a hashes identify content and do not authenticate hostile page code.
- Document-start bootstrap has explicit top-frame rejection, a random per-document epoch, pagehide/bfcache invalidation, and an early idle input guard. Held gestures reject activation; cleanup waits for input release and quiescence with a 2-second hard bound and an explicit unconfirmed result.
- `RuntimeManager` serializes preparation per host window. Every operation probes current document identity; only cold preparation sends the full bundle. Failed installations are remembered for that document to prevent installation loops. A successful late ready probe may recover preparation, but never replays a business operation.
- Host process identity is shared with workflow run records. Window identities rotate after destruction; per-document internal bridge credentials rotate on native navigation. Unbound/stale hello messages are rejected. Runtime calls check native URL, configured origin, window lifecycle, page epoch, runtime version, bundle identity and requested context.
- Public rich-data authorization remains in the application services. The authorized runtime variant rechecks revocation at the final WS enqueue/eval boundary, including after preparation waits. Workflow deadlines include runtime preparation; forwarded workflow timeouts are bounded by remaining time.
- Internal JSON uses `JSON.parse` over a serialized string to preserve own `__proto__` keys as data. Executable source is only build-embedded; there is no public script/module installation API.
- New dispatcher global is `__CONNECTOR_INSPECTION_RUNTIME__`. The original `__CONNECTOR_RUNTIME__` frontend event buffer remains intact.
- Health queries probe bounded transport/bridge/runtime layers without installation, reload or business replay. Anonymous bridge status exposes protocol versions and connection bookkeeping, not page title/URL or workspace paths. Rich identity returns canonical workspace information only after service authorization.

## Executed regression commands

1. Before adding bootstrap: `node --test plugin/tests/runtime.test.cjs` failed all 5 initial cases with the missing production bootstrap file, establishing the initial regression baseline.
2. `node --test plugin/tests/runtime.test.cjs`: 13 tests passed after implementation. These execute the shipped bootstrap/installer in a Node VM, and are not native evidence.
3. `cargo test -p tauri-plugin-connector --lib --no-default-features --locked runtime:: -- --nocapture`: 2 manifest tests passed.
4. `cargo test -p tauri-plugin-connector --lib --no-default-features --locked bridge::transport_tests:: -- --nocapture`: 20 tests passed, including actual bridge enqueue accounting with a simulated page responder. The 20-command case sends one bundle, 20 probes, and 20 short business packets. Twelve concurrent ensures share one installation. These prove host orchestration and transport behavior, not native business effects.
5. `cargo build --locked --manifest-path examples/workflow-fixture/Cargo.toml --target-dir target`: macOS fixture rebuilt during native diagnosis.

Additional integration builds initially failed while other owners' modules and fields were being introduced. Those transient missing-module/schema/Send errors were corrected by their owning lanes; they are not recorded as successful tests.

## Requirement mapping for this lane

| ID | Evidence and limitation |
| --- | --- |
| UP-T009 | Host cold-install handshake in `warm_runtime_transports_one_bundle_for_twenty_operations`; actual native outcome tracked below. |
| UP-T010 | Same test: exactly one bundle across 20 operations; all other scripts below 4096 bytes; counters recorded at WS enqueue/eval injection. |
| UP-T011 | `concurrent_runtime_preparation_is_single_flight`: 12 concurrent callers receive one identity from one install. |
| UP-T012 | JS distinct-document epoch test; native reload verification belongs to fixture run. |
| UP-T013 | JS repeated bootstrap same-document identity; workflow always re-resolves targets. Native SPA case not run by this lane. |
| UP-T014 | `identity::tests::label_reuse_changes_host_identity`; actual native window recreation tracked separately. |
| UP-T015 | JS expected-context race and Rust stale-operation test reject before handler/business transport. |
| UP-T016 | JS helper-text exception and `runtime_business_error_is_never_reprepared_or_replayed` preserve one dispatch. |
| UP-T017 | Cold missing-runtime preparation is separate from business dispatch. Conservative policy: no automatic retry after a short operation has been sent, even for a returned runtime-missing marker. Existing transport fallback requires trusted NotDispatched. |
| UP-T018 | Existing bridge late/timeout/no-replay cases retained; native independent write counter requires fixture evidence. |
| UP-T019 | JS disposer/observer test and drain cleanup tests; active module disposer bookkeeping prevents replacement while cleanup is unproven. |
| UP-T020 | Ready incompatible contexts/versions require reload; no silent active-version replacement. JS disposed runtime rejects later dispatch. |
| UP-T021 | Explicit bootstrap/dispatcher top-frame guards and host configured-origin checks. JS child-frame regression passed; native foreign-origin/subframe execution not separately run. |
| UP-T022 | JS pagehide/pageshow test changes restored epoch and does not restore active guard/session. Platform bfcache behavior remains separate. |
| UP-T023 | Host JSON serialization regression plus JS hostile strings/prototype keys test. |
| UP-T024 | Expired packet and spent transport deadline regressions; 8 failed ensures send only one failed install in the same document. |
| UP-T031 | Internal per-document bridge credentials reject stale hello after navigation; process identity/PID metadata use one start timestamp source. External discovery covered by routing lane. |
| UP-T033 | Health test preserves transport responsive/runtime unavailable, does not install. Native deep probe evidence separately recorded. |
| UP-T034 | Health reports `executionReplayed:false`; business helper exception never replayed. Existing unknown-write regression suite remains required. |
| UP-T035 | Health bounded validation and bridge waiter timeout/drop cleanup tests. |
| UP-T036 | Anonymous status excludes title/URL/workspace; rich service authorization is enforced by shared dispatcher tests. |
| UP-PK008 | Early idle guard makes no prevention; active suppression precedes picker callback. |
| UP-PK011 | Drain swallows trailing dblclick before returning to idle. |
| UP-PK018 | Held gesture prevents start; cleanup waits for held Escape release and resets quiet time on trailing events. |
| UP-PK022 | Drain finishes with an explicit confirmation; pagehide resets owned guard state. |
| UP-PK031 | Old picker context is rejected before its module runs on a different runtime. |

## Native diagnosis and boundaries

The actual isolated macOS fixture initially exposed integration defects that the small VM tests did not cover:

1. New runtime global collided with the original legacy event buffer. Corrected by using `__CONNECTOR_INSPECTION_RUNTIME__` and adding a compatibility regression.
2. Tauri 2.11.5 defines `__TAURI_INTERNALS__.invoke` and `.ipc` as non-writable/non-configurable. The first capture wrapper threw before the legacy bridge reached `connect()`. Native page inspection confirmed the descriptors. The capture lane fixed immutable-property handling and owns the compatible observation implementation.

Private diagnosis outputs remain in isolated temporary directories, not committed; they are failure evidence, not acceptance passes. Native input was not sent while installation failed. macOS initialization inspection did confirm top-level early bootstrap and native `tauri://localhost` origin. Linux/Windows native execution was not run by this lane.

The locally installed Tauri 2.11.5 plugin initialization and IPC scripts were inspected; the official initialization semantics were checked at <https://docs.rs/tauri/latest/tauri/plugin/struct.Builder.html>. No reference-project source was copied. No version bump, push, release, workflow-history reset or quarantine removal was performed.

## Final lane verification update

- `cargo test -p tauri-plugin-connector --lib --no-default-features --locked bridge::transport_tests::`: **20 passed**, including revocation during installation preventing final dispatch. Output: [runtime-rust-tests.txt](runtime-rust-tests.txt).
- `node --test plugin/tests/runtime.test.cjs`: **13 passed**. Output: [runtime-js-tests.txt](runtime-js-tests.txt).
- `cargo clippy -p tauri-plugin-connector --all-targets --all-features --locked -- -D warnings`: **passed** after coordinated integration.
- An isolated actual macOS native smoke passed after all three startup defects were corrected: separate runtime global, immutable Tauri invoke handling, and final-document bridge credential rebind. It confirmed `bridge.connected=true`, both bridge/runtime responsive, full native page/runtime identity, one successful runtime install, and explicit picker cancellation with confirmed cleanup. Evidence: [runtime-native-macos.json](runtime-native-macos.json) (derived smoke during diagnosis). This smoke did not select an element with OS input and is not evidence for input isolation or screenshot correctness.
- Reproducible dedicated native command is `node examples/workflow-fixture/scripts/runtime-native-smoke.mjs` after building the fixture. Its checked-in script separately asserts cold health does not install, then uses picker preparation to establish the native runtime and cancels it. The consolidated native fixture runs provide the current selection/input/screenshot evidence.

Native startup regression repair also added a host loading gate: a navigation invalidates old transport credentials immediately; a completed document rebinds its existing bridge without reinstalling wrappers. Window identity remains stable across navigation and changes on destruction. No operation is replayed while repairing this connection binding.

- `cargo test -p tauri-plugin-connector --lib --no-default-features --locked identity::`: **3 passed**, including navigation credential rotation/loading and stable window lifetime. Output: [runtime-identity-tests.txt](runtime-identity-tests.txt).

## Shared capability and bound-cleanup audit

`bridge_status.inspection` and `workflow_capabilities.inspection` now use the same pure capability helper. It describes protocol/runtime/semantic versions and the embedded bundle hash; identity lifetimes; authenticated picker actions, convenience calls, entry points, top-level light-DOM scope, boundary-only elements, the actual four-active-picker limit, and the 5–120 second timeout contract. Screenshot sources expose `compiled` and `runtimeAvailability` separately. Compiled support never claims a current page, OS permission, input-isolation, or native capture probe has passed. Preview and IPC observation boundaries are also explicit. The helper accepts no application state and contains no handles, page observations, tokens, paths or images. Workflow v1 operations remain unchanged.

Feature tests cover all 32 combinations of three compile-feature flags and four platform cases, compare the exported helper against actual `cfg!` values, check parser timeout boundaries, and reject private field names. An application-side integration test compares both anonymous views while private state exists, verifies no new workflow operation appears, and proves no runtime request or installation occurs.

A follow-up audit found that full-context cleanup could previously call `ensure` before rejecting an old page/runtime identity. Fully bound calls (`runtimeId` and `pageEpoch` in `context` or `expectedContext`) now only probe and validate the existing runtime. Missing/stale runtimes fail without sending a bundle. Initial calls without a full binding retain cold preparation. The regression covers picker, capture, geometry and workflow followups, and a matching-binding case preserves normal execution without reinstalling.

Verification of the audit followups:

- `cargo test -p tauri-plugin-connector --lib --no-default-features --locked capabilities::`: **3 passed**, including all 32 feature/platform combinations. [Output](capabilities-base-tests.txt).
- `cargo test -p tauri-plugin-connector --lib --all-features --locked capabilit`: **5 passed**, including both anonymous transport views and the existing shared authorization/resource boundary. [Output](capabilities-all-feature-tests.txt).
- `cargo test -p tauri-plugin-connector --lib --no-default-features --locked bridge::transport_tests::`: **23 passed**, including missing old-runtime cleanup with zero installation attempts and matching full-context reuse. [Output](runtime-bound-context-tests.txt).
