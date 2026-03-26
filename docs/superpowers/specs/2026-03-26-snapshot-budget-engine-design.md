# Snapshot Budget Engine — Design Spec

**Date**: 2026-03-26
**Status**: Approved
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

Levels are applied per-section as the walker progresses. A "section" is a top-level child of the snapshot root (or of the scoped `selector` element). Each top-level child is evaluated independently against the remaining budget. A section that starts in Normal can trigger Compact or Split as the budget fills.

### Repeating Sibling Detection

Before serializing children of any container, the walker hashes the first 3 children's structure (tag + role + class pattern, ignoring text content). If all 3 match, it serializes the first 2 examples, then collapses the rest to a reference:

```
- list [42 items]:
  - listitem "Patient: John Doe" [ref=e5]
  - listitem "Patient: Jane Smith" [ref=e6]
  - ... 40 more items [-> subtree-<uuid>/list-items.txt]
```

The collapsed items' full serialization goes to the subtree file. The ref map still includes all refs from collapsed items.

## 2. Subtree File System (Rust side)

### Flow

1. JS returns a new field in the result: `subtrees: [{ name: "list-items", content: "..." }, ...]`
2. Rust handler generates a UUID v4 for the snapshot session
3. Writes each subtree to `/tmp/tauri-connector/snapshots/<uuid>/<name>.txt`
4. Writes `meta.json` with: timestamp, app name, window ID, full ref map, file index with token estimates
5. Replaces subtree placeholder names in the skeleton with absolute file paths
6. Returns enriched result object

### Directory Structure

```
/tmp/tauri-connector/snapshots/<uuid>/
  ├── layout.txt              # copy of inline skeleton (for CLI re-read)
  ├── main-content.txt        # subtree file
  ├── sidebar-nav.txt         # subtree file
  ├── table-rows.txt          # subtree file
  └── meta.json               # session metadata
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
    { "name": "main-content.txt", "estimatedTokens": 2100 },
    { "name": "table-rows.txt", "estimatedTokens": 4200 }
  ],
  "totalElements": 1847,
  "inlineTokens": 3800
}
```

### File Lifecycle

- Old snapshots cleaned up when a new snapshot is taken — keep last 5 per window
- `/tmp` means OS handles cleanup on reboot
- No manual cleanup required

### MCP Response Shape

```json
{
  "snapshot": "<inline skeleton text>",
  "refs": { "e0": { ... } },
  "meta": {
    "elementCount": 1847,
    "truncated": false,
    "snapshotId": "a1b2c3d4-...",
    "subtreeFiles": [
      { "path": "/tmp/tauri-connector/snapshots/a1b2c3d4/main-content.txt", "name": "main-content", "estimatedTokens": 2100 },
      { "path": "/tmp/tauri-connector/snapshots/a1b2c3d4/table-rows.txt", "name": "table-rows", "estimatedTokens": 4200 }
    ],
    "inlineTokens": 3800,
    "budgetUsed": true
  }
}
```

When `maxTokens: 0` or the DOM fits within budget, `subtreeFiles` is an empty array and `budgetUsed` is `false`.

## 3. API Changes & Backward Compatibility

### MCP Tool

- New `maxTokens` / `max_tokens` parameter (default `4000`, `0` = unlimited)
- Existing calls without `maxTokens` get the new default — this is a **behavior change**
- To opt out: pass `maxTokens: 0`
- `webview_search_snapshot` cache stores full tree + subtree file paths; searches work across the complete DOM

### CLI

```
tauri-connector snapshot -i [--max-tokens 4000] [--no-split]
tauri-connector snapshots list                    # list recent snapshots with UUIDs
tauri-connector snapshots read <uuid> [file]      # read a subtree file
tauri-connector snapshots diff <uuid1> <uuid2>    # compare two snapshots
```

- `--no-split` is the escape hatch for full output (pipes, scripts, non-AI usage)
- UUID is always printed to stderr so it's accessible even when stdout is piped

### Bun Script

`skill/scripts/snapshot.ts` gets `--max-tokens` flag, forwarded through the WebSocket protocol.

### SKILL.md

Document the new parameter, subtree file pattern, and CLI snapshot management commands.

## 4. Edge Cases & Error Handling

| Scenario | Handling |
|---|---|
| Skeleton itself exceeds budget | Apply compact-mode filtering to skeleton; write complete depth-1 view to `full-layout.txt` subtree file |
| Single element has huge text content | Truncate inline text to 200 chars with `... [truncated, N chars]`. Full text via `webview_execute_js` or `webview_interact(action: "get-value")` |
| Dynamic DOM (mutations during snapshot) | No change — TreeWalker runs synchronously in JS main thread. Mutations queued during walk apply after |
| Portal trigger in skeleton, content split to file | Skeleton reference line includes portal annotation: `[portal -> subtree-<uuid>/modal-content.txt]`. Ref map in meta.json links portal refs to their file |
| Filesystem write failure (sandbox, permissions) | Fall back to returning full snapshot inline. Set `meta.splitFailed: true` with reason. No crash |
| UUID collision | UUID v4 — effectively impossible. Cleanup (keep last 5) prevents unbounded growth regardless |

## 5. Testing Strategy

### Unit Tests (JS engine, in-browser)

- Token estimation accuracy: compare heuristic vs actual for various DOM patterns
- Repeating sibling detection: identical list items, table rows, mixed children, single child (no collapse)
- Budget levels: verify Normal -> Compact -> Split transitions at correct thresholds (60%, 85%)
- Skeleton correctness: subtree references point to correct content
- Edge cases: empty DOM, single element, deeply nested (>20 levels), huge text content (>10K chars)

### Integration Tests (Rust plugin + JS)

- MCP call with `maxTokens: 4000` on large DOM -> inline result under budget, subtree files exist on disk
- MCP call with `maxTokens: 0` -> full output inline, no subtree files written
- File cleanup: take 6 snapshots -> verify only last 5 snapshot directories exist
- Filesystem fallback: mock write failure -> verify inline fallback with `splitFailed` warning
- `webview_search_snapshot` with split snapshots -> verify search covers subtree file content

### CLI Tests

- `snapshot -i --max-tokens 4000` -> UUID printed, subtree files listed
- `snapshots list` -> recent snapshots shown with timestamps and token counts
- `snapshots read <uuid> <file>` -> correct subtree content printed
- `--no-split` -> full output regardless of DOM size

### Real-World Validation

- Test against the Ant Design admin app that produced the 58K token snapshot
- Verify skeleton shows useful page structure (section names, key refs, navigation landmarks)
- Verify subtree files contain actionable content (refs preserved, element details intact)

## Files to Modify

| File | Change |
|---|---|
| `plugin/src/bridge.rs` | JS `__CONNECTOR_SNAPSHOT__`: add budget tracking, compression levels, sibling collapse, subtrees array |
| `plugin/src/handlers.rs` | `dom_snapshot`: pass `maxTokens` to JS, receive subtrees, write files, generate UUID |
| `plugin/src/mcp_tools.rs` | Parse `maxTokens` param, include `snapshotId` and `subtreeFiles` in response |
| `crates/cli/src/commands.rs` | Add `--max-tokens`, `--no-split` flags; add `snapshots` subcommand |
| `crates/cli/src/snapshot.rs` | Pass `maxTokens` to JS, print UUID and subtree info |
| `skill/scripts/snapshot.ts` | Add `--max-tokens` flag |
| `skill/SKILL.md` | Document new parameter and snapshot management |
