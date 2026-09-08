# P0 validation and legacy migration

Baseline inspected: `90eb9525fc50af68578b2295d17aba3a01c50abe` (the actual checkout HEAD before these changes), plugin version `0.14.0`. No version bump, release, deployment or business-data operation is part of this work.

## Baseline audit

| ID | Baseline confirmed | Implemented boundary / evidence |
| --- | --- | --- |
| B01 | `wait_for` returned Success for `{found:false,timeout:true}`. | Wait has an explicit typed outcome. Timeout is `condition_timeout`, with failed verification and retained elapsed/last observation data. Handler and WebSocket boundary tests cover this. |
| B02 | `act_and_verify` inspected top-level `error`; a false wait could produce a passed verdict. Legacy batch did not recognize failed verification as a typed failure. | Action and wait outcomes are combined without losing the completed action. A failed expect is an error with `execution:completed`, `verification:failed`; typed adapters and batch retain diagnostics. |
| B03 | `fill` called keyboard `type`, ignored selector, appended to whichever field had focus. | Act-and-verify fill/type resolve exactly one explicit target. Fill replaces, type appends, press targets/focuses when a selector is supplied. A shared native setter/event helper is used by legacy input and locator. |
| B04 | WebSocket errors, including after dispatch, fell back to eval. | Bridge distinguishes not dispatched, failed, and unknown. Only not dispatched can fall back. Transport tests count dispatches and cover the former two-second boundary. |
| B05 | Report save errors propagated from `run_from_value` after business actions had completed. | Batch returns its report with `persistenceWarning`; `savedTo` is absent when persistence fails. |
| B06 | Legacy batch arguments were static, with no result bindings or shared resource declarations. | Legacy scheduling is preserved; the separate workflow implementation provides bindings and application-side arbitration. See the main implementation status for its acceptance coverage. |
| B07 | Embedded batch dispatch round-tripped through MCP envelopes. | Shared internal dispatch now returns outcomes directly. MCP text/image encoding occurs at the edge; protocol tests retain JSON-like strings and business error fields. |
| B08 | Timeout/cancellation/connection teardown could leave pending bookkeeping, and did not establish a remote terminal state. | Client and bridge guards remove local pending mappings on exit. Disconnect/timeout retain unknown outcomes and request identity. Resource ownership follows the typed outcome. |
| B09 | Locator merged matches from separate constraints into a union. | Supplied role/text/label/attribute/name constraints now intersect. Legacy first/last/nth selection is retained; workflow uniqueness is a separate strict contract. Three JS regressions reproduce and reject the former union. |
| B10 | `networkidle` was an alias for `document.readyState === 'complete'`. | It fails before injection with `unsupported_condition`. `load` still means document completion. |
| B11 | Existing log writers flush synchronously; log readers hold their source lock while scanning. | Existing log implementation remains. The new workflow journal uses its own bounded writer queue and durable acknowledgement; this change does not claim to optimize all legacy log access. |
| B12 | IPC monitor injection result was discarded and a single global flag was set, using the default window. | Selected window is passed through the adapters. Toggles serialize, require an acknowledgement matching window/state/page epoch, and retain `desired` versus nullable `applied`. Missing or mismatched acknowledgement is a tool error. |

## Intentional compatibility corrections

- Successful legacy payloads keep their shape. A top-level optional `outcome` contains structured execution, verification, effect and error facts. Failed waits and failed act-and-verify calls now use the legacy error channel; diagnostics remain in `outcome.data`. Clients that treated any returned JSON as success must inspect the transport error / typed outcome.
- `act_and_verify` without a wait keeps legacy `verdict:inconclusive`, with typed `verification:not_requested`. A completed action alone does not verify a business goal.
- `fill` and `type` inside act-and-verify now require a selector; they cannot silently modify an unrelated focused field. The separate legacy keyboard tool still explicitly uses current focus and reports the actual target tag and ID.
- Input uses synthetic events, reports `interactionMode:synthetic`, and confirms retained DOM input value. It does not claim native operating-system keystrokes or business persistence. Text-like inputs and textareas are supported; unsupported custom/contenteditable components require an adapter. Disabled/readonly targets and cancelled input are rejected.
- All input values and IPC command names are serialized as JSON data. Quotes, backslashes, newlines, Unicode and `${...}` stay data.
- Multiple locator criteria now narrow the same candidate set. Existing explicit first/last/nth selection and legacy default selection remain compatible. Workflow strict targeting rejects ambiguity separately.
- Replace `networkidle` with `load` only when document loading is the intended condition. Application readiness requires an explicit selector, text, predicate or workflow condition.
- IPC monitor responses include `windowId`, `desired`, `applied`, `pageEpoch` and `acknowledgedAt`. These describe the last acknowledged page flag; they do not prove lossless IPC collection or business causal correlation. A new page must be re-enabled/rechecked. The internal aggregate legacy flag is not evidence that every window is monitored.
- Dropping a caller, cancelling its wait, or losing a response does not roll back a dispatched action. Unknown outcomes propagate through mutation handlers, including raw JS, IPC execution, interaction, input, locator and monitor/listener changes.

## P0 evidence

The JS suites run actual shipped helper code in Node with small DOM/event fixtures. They are deterministic behavior regressions, not a native WebView or React compatibility claim.

| Test IDs | Evidence |
| --- | --- |
| T01–T03 | Rust handler tests verify timeout diagnostics and failed verification over a real local WebSocket bridge with controlled replies. A single click is dispatched exactly once; arbitrary query `{error:"user saved text",ok:false}` remains successful business data. Batch dependency propagation is tested in the client suite. |
| T04–T07 | `plugin/tests/input.test.cjs`: target isolation, replace versus append, controlled-state simulation plus independent submission, exact hostile-string data, readonly input and cancelled beforeinput. Actual React/native submission needs the integration fixture recorded in the main status document. |
| T08–T11 | Bridge/client transport suites cover slow completion, exceptions, enqueue failure, remaining deadline, duplicate in-flight identity, future drop, disconnect, pending cleanup and eval JSON encoding. `plugin/tests/wait.test.cjs` covers deadline cleanup, navigation cleanup and an async predicate that never settles. |
| T12 | Client batch persistence regression checks completed results survive an unwritable report path. |
| T13 | Rust preflight test rejects `networkidle` before a script is created. |
| T14 | JS monitor test rejects a mismatched actual window before changing the flag. Rust acknowledgement tests and the WebSocket integration test reject wrong-window replies and keep `applied:null`; a valid reply changes it to true. |

Commands run in this checkout:

```sh
node --test plugin/tests/*.test.cjs
cargo test -p tauri-plugin-connector handlers::tests --lib
cargo test -p connector-client --test batch_outcomes
cargo test -p tauri-plugin-connector --lib
```

- Node helper suite: **14 passed**.
- Targeted Rust handler suite: **9 passed**.
- Client batch outcome/persistence suite: **6 passed**, including dependent-dispatch suppression, business error JSON preservation, retained execution after report persistence failure, and screenshot artifact compaction.
- Final integrated plugin suite: **96 passed / 0 failed**. Transport deadlines, input, monitoring, workflow redaction and the remaining orchestration regressions all pass. See the main implementation status document for platform boundaries.

The original keyboard/locator scripts and original networkidle behavior were exercised with the new tests before implementation; target/replace/controlled-state/escaping, locator intersection and networkidle tests failed as expected. Additional JS cleanup and wrong-window-monitor regressions also failed before their corresponding helpers were implemented.

## Limits

This document alone does not certify macOS/Windows/Linux native WebView behavior, true React submission, application-owned asynchronous writes, or lossless observation. Native fixture results, complete crate validation, client batch fault tests and the P1/P2 acceptance matrix belong in `workflow-implementation-status.md`. JavaScript polling releases its own timers/listeners on success, failure, deadline and pagehide; a user-provided promise may keep application work running, and its cancellation is not claimed.
