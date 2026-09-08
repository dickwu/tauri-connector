# Application-owned workflows

Workflow v1 executes known short sequences inside the Tauri application. The CLI and both MCP servers submit or inspect a run; they do not execute another copy of its steps. Existing single tools and `batch_actions` remain available.

## APIs and authorization

The host enables workflows with `ConnectorBuilder::workflow_token(...)` or `TAURI_CONNECTOR_WORKFLOW_TOKEN`. Tokens must contain at least 32 bytes. The CLI and standalone MCP read the same environment variable; embedded MCP callers supply `authToken` outside the spec. Capabilities are readable without a token. Run creation, history, cancellation and resume require authorization. The application rechecks host token authorization before each dispatched step, so revocation stops later dispatch. It also validates the current WebView origin against bundled, development, or explicitly configured host origins before injection; navigation outside those origins is rejected. Do not put tokens in `inputs`, files, logs or prompts containing a workflow spec.

| Tool | Required arguments | Optional arguments |
| --- | --- | --- |
| `workflow_capabilities` | none | `windowId` |
| `workflow_run` | `spec`, `authToken` | `waitMs` (0–30000, default 1000) |
| `workflow_get` | `runId`, `authToken` | `cursor`, `include: ["steps", "events", "evidence"]`, `evidenceId`, `offset` |
| `workflow_cancel` | `runId`, `authToken` | none |
| `workflow_resume` | `runId`, `expectedRevision`, `checkpointId`, `intent`, `authToken` | none |

`intent` is `continue` or `reconcile`. `waitMs` bounds response waiting, not run execution, and is excluded from the spec fingerprint. A disconnected client can retrieve the same run. Reuse the same `runKey` and identical spec if the submission response is lost. A changed spec with the same key produces `run_key_conflict`.

Direct WebSocket clients use `{ "id": "request-id", "type": "workflow", "operation": "workflow_run", "args": { "spec": {}, "authToken": "..." } }`. This is an example envelope; the spec must be valid. `bridge_status.workflowProtocolVersion` is `1` for supporting applications. The CLI/standalone MCP check this before sending workflow commands to an older plugin.

## Specification

Required fields are `schemaVersion: 1`, a nonempty `runKey`, and 1–100 `steps`. Only `mode: "strict"` and `schedule: "sequential"` are accepted. The default `windowId` is `main`; each step can override it. The default deadline is 60000 ms, with a 300000 ms maximum. Specs are capped at 256 KiB; resolved values and inputs are bounded separately.

Each step has an `id`, `op`, optional `timeoutMs`, optional `expect`, and operation-specific fields:

| Operation | Fields |
| --- | --- |
| `click` | `target` |
| `fill`, `type` | `target`, `value` |
| `press` | `key`, optional `target` |
| `wait` | `condition` |
| `query` | `target`, `query: { "kind": "value" \| "text" \| "attribute", "name": "attribute-name" }`; `name` is only accepted for attributes |
| `tool` | allowlisted `tool`, `args`, optional explicit `bindings` |

The trusted `tool` allowlist currently contains `bridge_status` and `ipc_get_backend_state`. Workflow v1 does not permit arbitrary JS or unknown IPC commands.

Locators use `{ "by": "role" | "label" | "testId" | "css", "value": ... }`, optionally constrained by `name`, a nested `scope`, and `entity: { "attribute": "data-id", "value": ... }`. Matching constraints intersect and must identify one target. Legacy `@ref` fallback is not supported in workflow v1. Inputs use synthetic events; they do not claim native OS interaction or business persistence.

Value expressions are scalar literals, `{ "literal": <any JSON> }`, `{ "fromInput": { "key": "input-name" } }`, or `{ "fromStep": { "stepId": "earlier-step", "pointer": "/value" } }`. JSON Pointer paths address earlier outcome `data`, support `~0` and `~1`, and cannot reference future steps. Expression-looking objects in arbitrary business data stay data. An object or array literal must use `literal` explicitly.

Conditions support `element` (visible, hidden, attached, detached, enabled, editable), `valueEquals`, `textContains`, `attributeEquals`, `result`, `all`, and `any`. `result` uses `stepId`, `pointer`, `operator: "eq" | "exists" | "nonEmptyString"`, and `expected` for equality. Composite conditions use a `conditions` array. State observations do not prove action causality or server persistence.

```json
{
  "schemaVersion": 1,
  "runKey": "isolated-fixture-edit-1",
  "inputs": { "title": "Isolated test task" },
  "steps": [
    {
      "id": "title", "op": "fill",
      "target": { "by": "testId", "value": "task-title" },
      "value": { "fromInput": { "key": "title" } },
      "expect": {
        "kind": "valueEquals",
        "target": { "by": "testId", "value": "task-title" },
        "expected": { "fromInput": { "key": "title" } }
      }
    },
    {
      "id": "read-title", "op": "query",
      "target": { "by": "testId", "value": "task-title" },
      "query": { "kind": "value" }
    }
  ],
  "goal": {
    "kind": "result", "stepId": "read-title", "pointer": "/value", "operator": "eq",
    "expected": { "fromInput": { "key": "title" } }
  }
}
```

Run this only against an isolated fixture with the named field. The example verifies the UI value, not a persisted task.

## Execution and recovery

The shared client crate contains serializable specs, reference resolution, validation and `ExecutionOutcome`. The plugin owns the run registry, dispatch, journal, page context, observations and resource arbiter. Embedded batch dispatch returns typed outcomes directly; MCP formatting occurs at the edge.

An outcome separates `execution` (`not_dispatched`, `completed`, `failed`, `outcome_unknown`), `verification` (`not_requested`, `passed`, `failed`, `inconclusive`) and `effect` (`none`, `possible`, `confirmed`). A required expectation must pass before the next step runs. A completed schedule without a goal returns `goalStatus: "not_requested"`. Reports preserve `originalTestVerdict` after reconciliation and retain possible effects after cancellation.

The journal acknowledges dispatch intent before potentially effectful steps and records sanitized run checkpoints. A keyed fingerprint compares normalized specs without exposing a plain hash of low-entropy input. Private storage is enforced on supported Unix hosts. Workflow storage uses the stable application data namespace. If that namespace or journal cannot be used safely, workflow execution fails closed even if legacy logging falls back to a temporary directory; temporary logs never establish a fresh deduplication identity. Startup recovery loads history and isolates interrupted effects; it does not replay steps or restore sensitive inputs for execution. Records are not silently evicted to make room for new keys. This is bounded history and deduplication, not a transaction log or general exactly-once guarantee.

`continue` requires the current revision/checkpoint, the same application instance, an undispatched paused step, and remaining original deadline. `reconcile` only rechecks a supported postcondition; it neither replays the original action nor releases uncertain write isolation based on elapsed time. History after restart is read-only. Missing capabilities and unsupported modes fail explicitly.

Legacy tools and workflow share application-owned resource leases. Mutations and focus-sensitive work serialize or return `resource_busy`. Unknown write outcomes retain quarantine, including after client disconnect. Read-only diagnostics remain available. Caller-supplied owner/read-only/idempotency fields cannot bypass arbitration.

Evidence is scoped and bounded. `maxInlineBytes` is 1024–65536 (default 16384). Use `workflow_get` with `include: ["evidence"]` to inspect retained evidence; heed coverage and truncation fields. To read a retained evidence reference independently of the summary budget, pass its `evidenceId`, then follow `evidencePage.nextOffset` with `offset`. The response contains minimal `runId`, `revision`, `status` and `evidencePage: { id, offset, content, nextOffset, totalBytes }`. `content` is a JSON text chunk, and offsets count UTF-8 bytes; use returned offsets rather than splitting Unicode text yourself. `offset` requires `evidenceId`; `nextOffset: null` marks the final chunk. DOM state does not establish absence of backend errors, native event semantics, or business persistence.

## CLI and migration

```sh
tauri-connector workflow capabilities
tauri-connector workflow run fixture.json --wait-ms 30000
tauri-connector workflow get <runId> --include evidence
tauri-connector workflow get <runId> --evidence-id <evidenceRef> --offset 0
tauri-connector workflow cancel <runId>
tauri-connector workflow resume <runId> --expected-revision 7 --checkpoint-id <checkpointId> --intent reconcile
```

CLI stdout contains the JSON report and run ID. Exit `0` means completed (or successful capabilities lookup), `1` means failure/cancellation or transport/validation error, and `2` means running, paused, interrupted, or unknown. A nonzero exit is not permission to resubmit with a fresh run key.

Intentional compatibility corrections:

- Wait timeout and failed action verification now fail single tools and batch dependencies while preserving diagnostics.
- Business JSON containing `error`, `ok: false`, or `found: false` is not generally a tool failure; only known tool contracts decide status.
- `fill` replaces the specified target's value; `type` appends to that target.
- `networkidle` returns `unsupported_condition` instead of claiming document completion proves network idleness.
- Bridge fallback is allowed only before dispatch. Execution errors and lost/late responses never trigger automatic replay.
- Batch report write failures return the execution report plus `persistenceWarning`; they do not erase completed effects.
- Batch screenshots default to artifact persistence and compact references. Explicit legacy `save: false` retains inline image data; results without a valid artifact retain evidence rather than silently discarding it.
- Parallel legacy batches can receive `resource_busy` for conflicting application resources. Lifecycle workflow tools cannot be nested in batches.
- IPC monitoring reports desired/applied state for the selected window.

P3 capabilities, transition-event assertions, native-input guarantees and automatic business recovery are not implemented. See `workflow-implementation-status.md` for actual validation and platform gaps.
