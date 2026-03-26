# Snapshot Budget Engine — Design Spec

**Date**: 2026-03-26
**Status**: Approved (rev 2 — post Codex audit)
**Problem**: `webview_dom_snapshot` returns DOM trees exceeding 58,000 tokens for complex apps — far beyond Claude Code's 10,000 token tool result limit.

## Solution Overview

Add a token budget engine to the JS snapshot walker. When the DOM exceeds the budget, the tool returns a compact layout skeleton inline and writes detailed subtrees to reference files identified by a UUID. Claude (or the user) can `Read` subtree files on demand.

## 1. Snapshot Budget Engine (JS side)

### New Parameter

| Parameter | Aliases | Default | Type | Purpose |
|---|---|---|---|---|
| `maxTokens` | `max_tokens` | `4000` | number | Token budget for inline result. `0` = unlimited (legacy behavior) |

### Token Estimation

As the tree walker builds each line, estimate tokens via `Math.ceil(line.length / 3.5)` and track a running total against the budget. No external tokenizer needed — the ~3.5 chars/token heuristic is sufficient for our indented tree format.

### Progressive Compression Levels

| Level | Trigger | Behavior |
|---|---|---|
| **Normal** | tokens < 60% budget | Full output as today |
| **Compact** | tokens 60–85% budget | Strip non-interactive unnamed elements; keep only ref-bearing and named nodes |
| **Split** | tokens > 85% budget | Current section written to subtree file; skeleton gets a reference line |

**Section rendering strategy** (addresses Codex HIGH — order-dependent output): Each top-level child of the snapshot root (or scoped `selector` element) is estimated in isolation first. The walker pre-renders the section to a buffer, measures its token cost, then chooses one mode for the entire section atomically. This prevents partial sections or mid-render mode switches.

### Repeating Sibling Detection

**Detection** (addresses Codex MEDIUM — heterogeneous tails): Instead of sampling only the first 3 children, the walker detects runs of structurally identical siblings across the full sibling sequence. The structural hash includes tag + role + class pattern + ARIA interactive state (checked, selected, expanded, disabled). Text content is excluded from the hash.

**Collapse rule**: A run of 5+ structurally identical siblings triggers collapse. The first 2 examples are serialized inline; the rest go to a subtree file:

```
- list [42 items]:
  - listitem "Patient: John Doe" [ref=e5]
  - listitem "Patient: Jane Smith" [ref=e6]
  - ... 40 more items [-> /tmp/.../subtree-0.txt]
```

Heterogeneous siblings (e.g., a selected row among unselected rows) break the run and are serialized individually.

The collapsed items' full serialization goes to the subtree file. The ref map still includes all refs from collapsed items.

## 2. Subtree File System (Rust side)

### Flow

1. JS returns a new field in the result: `subtrees: [{ label: "list-items", content: "..." }, ...]`
2. Rust handler generates a UUID v4 for the snapshot session
3. **Filename sanitization** (addresses Codex CRITICAL — path traversal): Rust generates filenames as `subtree-<index>.txt` (e.g., `subtree-0.txt`, `subtree-1.txt`). The JS-provided `label` is stored only as metadata in `meta.json`, never used in filesystem paths. This prevents path traversal via hostile DOM content.
4. **Secure directory creation** (addresses Codex HIGH — predictable /tmp): Creates directory at `<temp_dir>/tauri-connector-<pid>/snapshots/<uuid>/` with `0700` permissions. The PID scoping prevents cross-process enumeration.
5. **Atomic writes** (addresses Codex HIGH — race conditions): Each file is written to a temp file first, then atomically renamed into place. Prune operations are serialized via a Mutex per window ID. Before any delete, the handler verifies canonical paths stay under the snapshot root (prevents symlink attacks).
6. Writes `meta.json` with: timestamp, app name, window ID, full ref map, file index with token estimates, and JS-provided labels
7. Replaces subtree placeholder indices in the skeleton with absolute file paths
8. Returns enriched result object

### Directory Structure

```
<temp_dir>/tauri-connector-<pid>/snapshots/<uuid>/
  ├── layout.txt       # copy of inline skeleton (for CLI re-read)
  ├── subtree-0.txt    # first split section
  ├── subtree-1.txt    # second split section
  ├── subtree-2.txt    # third split section
  └── meta.json        # session metadata
```

### meta.json Schema

```json
{
  "snapshotId": "<uuid>",
  "timestamp": 1711900000000,
  "appName": "admin",
  "windowId": "main",
  "refs": { "e0": { "tag": "button", "role": "button", "name": "Save", "selector": "..." } },
  "files": [
    { "name": "subtree-0.txt", "label": "main-content", "estimatedTokens": 2100 },
    { "name": "subtree-1.txt", "label": "table-rows", "estimatedTokens": 4200 }
  ],
  "totalElements": 1847,
  "inlineTokens": 3800
}
```

### File Lifecycle

- Old snapshots cleaned up when a new snapshot is taken — keep last 5 per window
- Prune operations serialized via Mutex to prevent concurrent cleanup races
- Canonical path verification before any delete operation
- OS handles final cleanup on reboot (temp dir)

### MCP Response Shape

**Inline refs budgeting** (addresses Codex HIGH — refs map blowing token limit): The inline `refs` object only includes refs that appear in the skeleton text. The full ref map is stored in `meta.json` on disk. This ensures the serialized MCP response stays within token budget.

**Completeness contract** (addresses Codex MEDIUM — truncated semantics): Replace `truncated: false` with explicit `split: true` / `inlineComplete` field so callers know inline output is not the full DOM.

```json
{
  "snapshot": "<inline skeleton text>",
  "refs": { "e5": { ... }, "e6": { ... } },
  "meta": {
    "elementCount": 1847,
    "split": true,
    "inlineComplete": false,
    "snapshotId": "a1b2c3d4-...",
    "subtreeFiles": [
      { "path": "/tmp/.../subtree-0.txt", "label": "main-content", "estimatedTokens": 2100 },
      { "path": "/tmp/.../subtree-1.txt", "label": "table-rows", "estimatedTokens": 4200 }
    ],
    "allRefsPath": "/tmp/.../meta.json",
    "inlineTokens": 3800
  }
}
```

When `maxTokens: 0` or the DOM fits within budget: `split: false`, `inlineComplete: true`, `subtreeFiles: []`, and `refs` contains the full map.

## 3. API Changes & Backward Compatibility

### MCP Tool

- New `maxTokens` / `max_tokens` parameter (default `4000`, `0` = unlimited)
- Existing calls without `maxTokens` get the new default — this is a **behavior change**
- To opt out: pass `maxTokens: 0`

### Internal callers use maxTokens: 0

(Addresses Codex HIGH — auto-push/cache shrinkage): The `maxTokens` default of 4000 applies **only to the MCP tool entry point** (`mcp_tools.rs`). Internal callers are not affected:

- `autoPushDom` (background cache push): continues using `maxElements: 5000`, no `maxTokens` constraint
- `webview_search_snapshot` internal re-snapshot: uses `maxTokens: 0` for full-fidelity capture
- WebSocket protocol `dom_snapshot` command: uses `maxTokens: 0` unless explicitly provided

### Search cache update

(Addresses Codex HIGH — search misses split content): When a budgeted snapshot produces subtree files, the search cache (`DomEntry`) stores the merged full-text (skeleton + all subtree content concatenated) for search purposes. Search results from subtree content include the subtree file path in the match context so Claude knows which file to read.

### WebSocket protocol extension

(Addresses Codex HIGH — missing protocol changes): Extend `Command::DomSnapshot` in `protocol.rs` to include optional `max_tokens` and `no_split` fields. Update `server.rs` dispatch to forward these to `handlers::dom_snapshot`. This enables CLI and Bun script to use the new parameters.

### CLI

```
tauri-connector snapshot -i [--max-tokens 4000] [--no-split]
tauri-connector snapshots list                    # list recent snapshots with UUIDs
tauri-connector snapshots read <uuid> [file]      # read a subtree file
tauri-connector snapshots diff <uuid1> <uuid2>    # compare two snapshots
```

- `--no-split` is the escape hatch for full output (pipes, scripts, non-AI usage)
- UUID is always printed to stderr so it's accessible even when stdout is piped
- **Compact mode compatibility** (addresses Codex MEDIUM): When budget splitting is active, the legacy `-c/--compact` post-filter preserves subtree reference lines (`... [->`) in addition to `ref=` and named element lines

### Bun Script

`skill/scripts/snapshot.ts` gets `--max-tokens` flag, forwarded through the WebSocket protocol.

### SKILL.md

Document the new parameter, subtree file pattern, and CLI snapshot management commands.

## 4. Edge Cases & Error Handling

| Scenario | Handling |
|---|---|
| Skeleton itself exceeds budget | Apply compact-mode filtering to skeleton; write complete depth-1 view to `subtree-0.txt` |
| Skeleton still exceeds after compact | **Last-resort manifest** (addresses Codex MEDIUM): return a hard-bounded manifest-only output: `snapshotId`, element count, subtree file index with labels/token estimates. Guaranteed < 500 tokens regardless of DOM size. |
| Single element has huge text content | Truncate inline text to 200 chars with `... [truncated, N chars]`. Full text via `webview_execute_js` or `webview_interact(action: "get-value")` |
| Dynamic DOM (mutations during snapshot) | No change — TreeWalker runs synchronously in JS main thread. Mutations queued during walk apply after |
| Portal trigger in skeleton, content split to file | Skeleton reference line includes portal annotation: `[portal -> .../subtree-N.txt]`. Ref map in meta.json links portal refs to their file |
| Filesystem write failure | **Bounded fallback** (addresses Codex HIGH): fall back to a bounded emergency skeleton (compact-filtered, hard-capped at `maxTokens`) plus `meta.splitFailed: true` with reason. Never fall back to unbounded inline. |
| UUID collision | UUID v4 — effectively impossible. Cleanup (keep last 5) prevents unbounded growth regardless |
| Multi-window snapshots | (Addresses Codex HIGH): Snapshot storage is keyed by actual `window_id` from the request, not hardcoded "main". Cleanup "keep last 5" applies per window ID. |
| Concurrent snapshot requests | Prune operations serialized via per-window Mutex. Atomic file writes prevent partial reads. |

## 5. Testing Strategy

### Unit Tests (JS engine, in-browser)

- Token estimation accuracy: compare heuristic vs actual for various DOM patterns
- Repeating sibling detection: identical list items, table rows, mixed children, single child (no collapse), heterogeneous tails (selected/error rows break the run)
- Budget levels: verify Normal -> Compact -> Split transitions at correct thresholds (60%, 85%)
- Section-atomic rendering: verify each section gets one mode, no partial output
- Skeleton correctness: subtree references point to correct content
- Last-resort manifest: verify output < 500 tokens when skeleton overflows after compact
- Edge cases: empty DOM, single element, deeply nested (>20 levels), huge text content (>10K chars)

### Integration Tests (Rust plugin + JS)

- MCP call with `maxTokens: 4000` on large DOM -> inline result under budget, subtree files exist on disk
- MCP call with `maxTokens: 0` -> full output inline, no subtree files written
- **Inline refs filtering**: verify MCP response `refs` only contains refs from skeleton, full map in `meta.json`
- **Total response size**: verify serialized MCP response (skeleton + refs + meta JSON) stays under 10K tokens
- File cleanup: take 6 snapshots -> verify only last 5 snapshot directories exist
- Filesystem fallback: mock write failure -> verify bounded emergency skeleton with `splitFailed`, not full inline
- `webview_search_snapshot` with split snapshots -> verify search covers subtree file content
- **Path traversal**: inject `../` in DOM class names -> verify filenames are counter-based, not derived from DOM
- **Directory permissions**: verify snapshot dir has `0700` permissions
- **Concurrent snapshots**: two simultaneous MCP calls -> no file corruption or race

### CLI Tests

- `snapshot -i --max-tokens 4000` -> UUID printed, subtree files listed
- `snapshots list` -> recent snapshots shown with timestamps and token counts
- `snapshots read <uuid> <file>` -> correct subtree content printed
- `--no-split` -> full output regardless of DOM size
- `-c` with split output -> subtree reference lines preserved

### Real-World Validation

- Test against the Ant Design admin app that produced the 58K token snapshot
- Verify skeleton shows useful page structure (section names, key refs, navigation landmarks)
- Verify subtree files contain actionable content (refs preserved, element details intact)

## Files to Modify

| File | Change |
|---|---|
| `plugin/src/bridge.rs` | JS `__CONNECTOR_SNAPSHOT__`: add budget tracking, section-atomic rendering, sibling run detection, subtrees array |
| `plugin/src/handlers.rs` | `dom_snapshot`: pass `maxTokens` to JS, receive subtrees, write files with counter-based names, generate UUID, atomic writes, secure dir with `0700`, per-window Mutex for cleanup, bounded fallback on write failure |
| `plugin/src/mcp_tools.rs` | Parse `maxTokens` param, filter inline refs to skeleton-only, include `snapshotId`/`subtreeFiles`/`split`/`inlineComplete` in response |
| `plugin/src/state.rs` | Update `DomEntry` to store merged full-text for search; add snapshot session state |
| `plugin/src/protocol.rs` | Extend `Command::DomSnapshot` with `max_tokens` and `no_split` fields |
| `plugin/src/server.rs` | Forward `max_tokens`/`no_split` from WebSocket dispatch to handlers |
| `crates/cli/src/commands.rs` | Add `--max-tokens`, `--no-split` flags; add `snapshots` subcommand; update compact filter |
| `crates/cli/src/snapshot.rs` | Pass `maxTokens` to JS, print UUID and subtree info |
| `skill/scripts/snapshot.ts` | Add `--max-tokens` flag |
| `skill/SKILL.md` | Document new parameter, subtree files, and snapshot management commands |

## Appendix: Codex Audit Resolution

All 14 findings (1 CRITICAL, 9 HIGH, 4 MEDIUM) from the GPT-5.4 xhigh audit have been addressed in this revision:

| ID | Severity | Finding | Resolution |
|---|---|---|---|
| 1 | CRITICAL | Path traversal via JS-controlled subtree `name` | Counter-based filenames in Rust; JS label is metadata only (Section 2) |
| 2 | HIGH | `refs` map can blow token budget | Inline refs filtered to skeleton-visible only; full map in `meta.json` (Section 2, MCP Response) |
| 3 | HIGH | Search cache misses split content | Merged full-text stored in `DomEntry` for search (Section 3) |
| 4 | HIGH | Auto-push/cache affected by default maxTokens | Internal callers use `maxTokens: 0`; default applies only to MCP entry point (Section 3) |
| 5 | HIGH | WebSocket protocol missing maxTokens | `Command::DomSnapshot` extended with `max_tokens` and `no_split` (Section 3) |
| 6 | HIGH | `/tmp` world-readable | PID-scoped dir with `0700` permissions (Section 2) |
| 7 | HIGH | Concurrent cleanup races / symlink attacks | Per-window Mutex, atomic writes, canonical path verification (Section 2, 4) |
| 8 | HIGH | `window_id` hardcoded to "main" | Storage keyed by actual `window_id` from request (Section 4) |
| 9 | HIGH | Write-failure fallback recreates the problem | Bounded emergency skeleton; never unbounded inline (Section 4) |
| 10 | HIGH | Budget algorithm order-dependent | Section-atomic rendering: pre-render, measure, choose mode, emit (Section 1) |
| 11 | MEDIUM | Sibling detection misses heterogeneous tails | Full-sequence run detection with ARIA state in hash (Section 1) |
| 12 | MEDIUM | `truncated: false` misleading when split | Explicit `split`/`inlineComplete` contract (Section 2, MCP Response) |
| 13 | MEDIUM | No hard-bounded last-resort output | Manifest-only output < 500 tokens (Section 4) |
| 14 | MEDIUM | Compact filter drops subtree reference lines | Preserve `[->` lines in compact mode (Section 3, CLI) |
