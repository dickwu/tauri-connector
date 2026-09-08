# Workflow implementation status

Date: 2026-09-08. Implementation baseline: `90eb9525fc50af68578b2295d17aba3a01c50abe` (`0.14.0`). The user subsequently authorized pushing and publishing this work as `0.15.0`; all four package versions are synchronized. The pre-existing `Cargo.lock` changes were preserved. No production business data was used. Release details are in [v0.15.0](releases/v0.15.0.md).

Scope: P0–P2 of `tauri-connector-codex-multistep-spec.md` v1.0. P3 remains deferred. The actual contracts are described in [workflow-design.md](workflow-design.md), the [P0 audit](workflow-p0-validation.md), the [native fixture](../examples/workflow-fixture/README.md), and the shipped CLI/MCP references.

## Delivered phases

| Phase | Implementation | Verification |
| --- | --- | --- |
| P0 | Typed execution/verification/effect outcomes; wait and assertion failure propagation; targeted fill/type; intersecting legacy locator constraints; dispatch-aware bridge fallback; pending/listener cleanup; report-save warnings; honest networkidle and per-window monitor acknowledgement. | Rust transport/handler/batch tests, 14 shipped-helper JS regressions, and actual macOS React/Wry/Rust submit and timeout scenarios. |
| P1 | Strict schema and bounded expressions/conditions; unique semantic targets; application-owned run/get/cancel/capabilities; fixed total/step deadlines; pre-dispatch re-resolution; observations established before actions; local data binding; scoped evidence with independent paged reads. | Full example is parsed and executed in service tests and the real native fixture. Browser and Rust tests cover wrong targets, stale context, cancellation, missing bindings, goal semantics and unsupported capabilities. |
| P2 | Atomic runKey/spec deduplication, keyed fingerprints, durable journal, checkpoint CAS, limited continue/reconcile, history recovery without replay, shared application resource leases and quarantine, trusted diagnostic tool allowlist, raw internal outcomes, private persistence, bounded retention/output and redaction. | Fault, concurrency, resume, retention, schema/adapter and journal recovery tests; native reconnect/dedup/no-replay checks; connector-off native build/run check. |
| P3 | Not implemented or advertised. | Parallel workflow/DAG execution, business state/receipt providers, causal traces, general restart continuation, arbitrary scripts, branching and loops are explicitly unsupported. |

## Final verification

The following required workspace sequence completed successfully against the implementation:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p connector-cli -p connector-mcp-server -p connector-client
cargo test -p tauri-plugin-connector
cargo check -p tauri-plugin-connector --no-default-features
```

Rust totals: **223 tests passed**, consisting of CLI 59, client 53 (32 unit + 6 batch + 15 workflow contract), standalone MCP 15, and plugin 96. No ignored tests or failed checks in this sequence. Plugin tests include 22 service integration cases, 13 journal, 6 redaction, 5 output-budget, 5 retention-budget and 5 resource tests. Mock backends prove orchestration and effect counts; they are not presented as native WebView tests.

```sh
node --test plugin/tests/input.test.cjs plugin/tests/locator.test.cjs plugin/tests/monitor.test.cjs plugin/tests/wait.test.cjs
WORKFLOW_REACT_MODULES=/Volumes/GWD/inter/tool node plugin/tests/workflow/page.test.cjs
```

JavaScript totals: **40 passed** (14 shipped-helper cases and 26 isolated Chromium cases). React was loaded from the existing local installation; no package installation or production dependency was added. The browser runner reports `reactCoverage: verified`.

The separate native fixture is built outside the production workspace and has its own explicit optional connector feature. Reproduction commands are in its README. Checked-in machine-readable native evidence:

- [macOS native scenarios](../examples/workflow-fixture/validation/macos-native-results.json): actual React controlled textarea → Wry → Tauri command → isolated Rust store. Assertions check submitted values and native write counts independently of workflow reports, including hostile string data, two clients, reconnect, cancellation, a slow write, a post-write exception and a lost response.
- [macOS connector-disabled run](../examples/workflow-fixture/validation/macos-feature-off-results.json): the actual application remained running, ports 19555/19556 were absent, and the connector was absent from the dependency tree.

The native fixture uses isolated random application identifiers, private credentials, dedicated ports and a local in-memory store. Each uncertain-write fault scenario must use its own application instance because quarantine intentionally survives the test operation. No workflow result or UI row is called production business-persistence proof.

## Acceptance matrix and limits of evidence

| Spec tests | Evidence and boundary |
| --- | --- |
| T01–T03 | Real local WS handler boundary and typed batch dependency tests; business JSON error-like fields remain data. |
| T04–T07 | Helper regressions, real React browser submission and actual React/Wry/Rust native submission. |
| T08–T11 | Client/bridge lifecycle and dispatch-count tests; native >2 second write, exception after write, 30 second lost response and pending cleanup. No claim that dropping a future cancels application work. |
| T12–T14 | Report-save failure tests, networkidle rejection, per-window monitoring acknowledgement/mismatch tests. Monitor status is the last acknowledged page state, not lossless collection. |
| T15 | The shipped create-task JSON is loaded by tests, completes through the service and runs in the native fixture. |
| T16–T19 | Browser scope/role/entity intersections, ambiguity, disabled/occluded/inert targets, node reuse, focus-time replacement and stale page identity. Workflow refs are rejected; real multi-window reload behavior is not separately certified by the native harness. |
| T20–T22 | Pure Rust pointer/reference/type/limit tests plus service example binding. Internal bindings survive public projection; missing/oversized required values stop execution. |
| T23–T24 | Observer-ready-before-dispatch checks, mutation counters and fresh current-state tests. Transition/causal assertions are rejected; vanished transient state is not latched as business success. |
| T25–T29 | Service deadlines (including a stalled backend), absent/requested goal behavior, strict no-fallback paths, input/queue/retention limits and wait/in-flight cancellation. |
| T30–T37 | Concurrent run identity, conflict, reconnect, unknown write, cancel/CAS, partial continuation and sticky original-verdict tests; native dedup/reconnect/effect counts. Reconciliation records current-state late verification only; no business receipt provider is implemented. |
| T38–T39 | Durable intent/reopen/interrupted history, no replay, lock ownership, partial/corrupt tails, quota/write faults and abandoned acknowledgement tests. These are real filesystem recovery/fault tests with simulated event boundaries, not certification of every OS power-loss or kill timing. |
| T40–T43 | Embedded/WS adapter boundary parity, resource hierarchy/atomicity/drop tests, inherited internal dispatch, failed-verification/unknown-effect quarantine, cancellation and subsequent legal work. Native tests verify uncertain writes block conflicts. |
| T44–T47 | Batch artifact compaction, bounded output preserving unknown effects, scoped evidence paging, honest source coverage, sensitive-field refusal/redaction, keyed fingerprints and retention rejection without eviction. |
| T48–T49 | CLI/MCP/WS outcome and schema/skill parity; real optional-feature-off build and application run. |
| T50 | Actual native macOS verified. Windows and Linux native WebViews were not run. No cross-platform success is claimed. |

## Deliberate implementation choices

- Keep the four existing crates. GUI-independent spec/outcome code lives in `connector-client`; all execution and persistent ownership stays in the plugin.
- Workflow access is host-enabled with a token of at least 32 bytes. All new durable APIs share authorization; credentials are outside the spec, and the host token is rechecked before each backend dispatch. Existing diagnostic access is not described as a complete remote identity platform.
- Unknown UI handlers can touch focus and backend state. The first implementation conservatively leases the whole UI/backend range for a workflow segment. Conflicts return `resource_busy`; diagnostics remain readable. Parallel legacy batches retain their input format but may encounter contention. No model-supplied resource ownership or idempotence declaration overrides this policy.
- A completed UI event or DOM value does not prove a backend operation ended. Unknown writes, partial write failures and failed/inconclusive required verification retain quarantine. Neither elapsed time nor DOM-only reconciliation unlocks it automatically.
- Dispatch and observation use unique request/page identities. Legacy transport fallback only happens when dispatch is known not to have occurred. New semantic locators are strict; structured/bare workflow refs and `stabilityMs` are explicitly rejected instead of partially accepted. Geometric actionability is checked synchronously around focus; movement stabilization, shadow DOM, iframes and full accessibility-tree parity are not claimed.
- Supported conditions are `current_state`, with `correlation: state_only`. Polling and evidence grace use the requested bounded defaults. Event-transition assertions and true `networkidle` are unsupported.
- `continue` revalidates the last successful boundary and only resumes undispatched work within the original deadline. Completed writes are never replayed. `reconcile` can inspect retained same-step data and current DOM postconditions but does not turn an earlier failed test into a pass. Restart history is read-only and never automatically re-executes a spec.
- Journal storage is private, exclusively owned JSONL with `sync_data` acknowledgements and a private HMAC key. Caps are 64 MiB, 100,000 events and 64 KiB per event; a damaged tail fails closed. Sensitive specs/results are not persisted for restart execution. Windows ACL support is not implemented: durable workflow access refuses unsafe persistence there. Existing single-tool behavior remains available within its existing contract.
- Up to 4 execution slots and 16 additional queued runs are admitted, subject to resource conflicts. Retained runs are capped at 1,000 and a 64 MiB conservative allocation-accounting budget; this is not a promise about total process RSS. Records are not evicted to make room for another key. A host archive/reset intentionally ends that namespace's deduplication lifetime.
- Public output budgets preserve execution/effect/error/request identity before ordinary details. Scoped evidence contains structural rows, not input values or arbitrary DOM text. Every retained evidence reference can be read in bounded UTF-8 pages. Credential-like fields and explicitly sensitive ancestors refuse query extraction; arbitrary app-specific secrecy still needs a host adapter.
- Reports use `savedTo` only after a successful ordinary report save. A report/artifact warning never erases executed results; a required journal failure prevents further side effects.

## Performance evidence

The native harness compares the same four UI steps submitted separately with one combined workflow. It verifies one intended native save in each case and records request counts, bytes, elapsed time and report size in the linked JSON. This proves the caller need not relay each known step or result binding. The small sample is not a latency benchmark, does not establish percentile distributions, and supports no fixed speedup or general exactly-once claim.

No P3 implementation remains in the accepted P0–P2 scope. Remaining verification extensions are native Windows/Linux, native multi-window navigation stress, and exhaustive abrupt-process/power-loss fault injection; current unsupported boundaries are visible in capabilities and documented above.
