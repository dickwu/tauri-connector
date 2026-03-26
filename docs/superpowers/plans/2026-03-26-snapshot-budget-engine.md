# Snapshot Budget Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent `webview_dom_snapshot` from exceeding Claude Code's 10K token tool result limit by adding JS-side token budget tracking with progressive compression and UUID-identified subtree file splitting.

**Architecture:** The JS `__CONNECTOR_SNAPSHOT__` function gains a `maxTokens` budget (default 4000). It pre-renders each top-level DOM section, picks a compression mode (Normal/Compact/Split) per section atomically, detects repeating sibling runs, and returns split subtree content for the Rust handler to write to PID-scoped temp files with counter-based filenames.

**Tech Stack:** Rust (Tauri plugin, CLI), JavaScript (bridge-injected snapshot engine), TypeScript (Bun fallback script)

**Spec:** `docs/superpowers/specs/2026-03-26-snapshot-budget-engine-design.md` (rev 2, post Codex audit)

---

## File Map

| File | Responsibility | Action |
|---|---|---|
| `plugin/src/bridge.rs:548-911` | JS `__CONNECTOR_SNAPSHOT__` -- budget engine, sibling detection, subtrees | Modify |
| `plugin/src/handlers.rs:32-64` | `dom_snapshot` -- pass maxTokens, write subtree files, UUID, cleanup | Modify |
| `plugin/src/handlers.rs:1382-1460` | `search_snapshot` -- search merged full-text including subtrees | Modify |
| `plugin/src/mcp_tools.rs:90-108` | MCP dispatch -- parse maxTokens, filter inline refs, build response | Modify |
| `plugin/src/state.rs:28-46` | `DomEntry` -- add snapshot session fields for search | Modify |
| `plugin/src/protocol.rs:37-44` | `Command::DomSnapshot` -- add max_tokens, no_split fields | Modify |
| `plugin/src/server.rs:139-141` | WebSocket dispatch -- forward new fields | Modify |
| `crates/cli/src/main.rs:48-75` | CLI arg parsing -- add --max-tokens, --no-split, snapshots subcommand | Modify |
| `crates/cli/src/snapshot.rs:22-83` | `SnapshotOptions`, `build_snapshot_script` -- add maxTokens | Modify |
| `crates/cli/src/commands.rs:18-60` | `snapshot()` -- print UUID, subtree info | Modify |
| `skill/scripts/snapshot.ts` | Bun script -- add --max-tokens flag | Modify |
| `skill/SKILL.md` | Documentation -- new parameter, subtree files | Modify |

---

### Task 1: JS Budget Engine -- Token Estimation and maxTokens Parameter

**Files:**
- Modify: `plugin/src/bridge.rs:548-560` (JS `__CONNECTOR_SNAPSHOT__` options parsing)
- Modify: `plugin/src/bridge.rs:880-911` (render and return)

This task adds the `maxTokens` parameter and token estimation function to the JS snapshot engine. No compression or splitting yet -- just the measurement infrastructure.

- [ ] **Step 1: Add maxTokens option parsing**

In `plugin/src/bridge.rs`, inside the `window.__CONNECTOR_SNAPSHOT__` function definition (line 548), add `maxTokens` to the options destructuring. Find the block starting at line 549:

```javascript
    const opts = options || {{}};
    const mode = opts.mode || 'ai';
    const maxDepth = opts.maxDepth || 0;
    const maxElements = opts.maxElements || 0;
    const reactEnrich = opts.reactEnrich !== false;
    const followPortals = opts.followPortals !== false;
    const shadowDom = opts.shadowDom === true;
```

Replace with:

```javascript
    const opts = options || {{}};
    const mode = opts.mode || 'ai';
    const maxDepth = opts.maxDepth || 0;
    const maxElements = opts.maxElements || 0;
    const maxTokens = opts.maxTokens || 0;
    const reactEnrich = opts.reactEnrich !== false;
    const followPortals = opts.followPortals !== false;
    const shadowDom = opts.shadowDom === true;
```

- [ ] **Step 2: Add token estimation helper function**

After the `buildSelector` function (around line 721), add:

```javascript
    // Token estimation: ~3.5 chars per token for indented tree format
    function estimateTokens(text) {{
      return Math.ceil(text.length / 3.5);
    }}
```

- [ ] **Step 3: Replace render phase with budget-aware section rendering**

Replace the existing render and return section (lines 880-911) with a version that tracks tokens, pre-renders sections atomically, and filters inline refs when splitting. The new code:

1. Pre-renders each top-level child to measure tokens
2. For each section: if it fits in remaining budget, emit inline; otherwise push to `subtrees` array
3. Extracts only skeleton-visible refs for inline `refs` (full set goes to `allRefs`)
4. Returns new `meta` fields: `split`, `inlineComplete`, `inlineTokens`
5. Returns `subtrees` array and `allRefs` for the Rust handler

Full replacement code is in the spec Section 1. Key output shape:

```javascript
    return {{
      snapshot: snapshot,
      refs: inlineRefs,        // only refs visible in skeleton
      allRefs: split ? refs : null,  // full ref map (null when no split)
      subtrees: subtrees,      // [{label, content}, ...]
      meta: {{
        elementCount, truncated, split, inlineComplete: !split,
        portalCount, virtualScrollContainers, inlineTokens: usedTokens
      }}
    }};
```

- [ ] **Step 4: Build and verify no regressions**

Run: `cargo build -p tauri-plugin-connector 2>&1 | tail -5`
Expected: Build succeeds (JS is embedded as a format string, not compiled)

- [ ] **Step 5: Commit**

```
feat: add token budget engine to JS snapshot walker
```

---

### Task 2: JS Budget Engine -- Repeating Sibling Run Detection

**Files:**
- Modify: `plugin/src/bridge.rs` (add sibling detection before render phase)

This task adds the repeating sibling collapse logic that detects runs of 5+ structurally identical siblings and collapses them to 2 examples plus a marker.

- [ ] **Step 1: Add structural hash and collapse functions**

After the `estimateTokens` function added in Task 1, add two functions:

`structHash(node)` -- builds a hash from label + non-ref/component attrs (includes ARIA state like checked, selected, expanded, disabled)

`collapseRepeats(children)` -- scans child array for runs of 5+ siblings with identical structHash. Keeps first 2, replaces rest with a marker node containing `_collapsedNodes` array for subtree file content.

- [ ] **Step 2: Apply collapse recursively before rendering**

Before the pre-render loop, add `collapseTree(rootNode)` that recursively applies `collapseRepeats` to all containers.

- [ ] **Step 3: Collect collapsed content for subtree files**

Add `collectCollapsed(node)` that walks the tree and renders `_collapsedNodes` content. When budget is active, push collapsed content to the `subtrees` array.

- [ ] **Step 4: Build and verify**

Run: `cargo build -p tauri-plugin-connector 2>&1 | tail -5`
Expected: Build succeeds

- [ ] **Step 5: Commit**

```
feat: add repeating sibling run detection to snapshot engine
```

---

### Task 3: Rust Handler -- Subtree File Writing with Secure Storage

**Files:**
- Modify: `plugin/src/handlers.rs:1-14` (imports)
- Modify: `plugin/src/handlers.rs:32-64` (`dom_snapshot` function)
- Modify: `plugin/src/state.rs:28-46` (`DomEntry`) and `plugin/src/state.rs:76-80` (`PluginState`)

- [ ] **Step 1: Add snapshot_prune_lock to PluginState**

In `plugin/src/state.rs`, add `pub snapshot_prune_lock: Arc<Mutex<()>>` to `PluginState` struct. Initialize it in the constructor.

- [ ] **Step 2: Add search_text and snapshot_id to DomEntry**

In `plugin/src/state.rs`, add to `DomEntry`:
- `pub search_text: String` (merged full-text for search, default empty)
- `pub snapshot_id: Option<String>` (UUID if subtrees written)

Both with `#[serde(default)]`.

- [ ] **Step 3: Rewrite dom_snapshot handler**

Add new parameters: `max_tokens: Option<u64>` and `state: &PluginState`.

The handler:
1. Passes `maxTokens` to the JS script
2. Checks if JS returned `subtrees` array
3. If no subtrees: return result as-is (backward compat)
4. If subtrees: generate UUID, create PID-scoped dir (`0700`), write counter-based files with atomic rename
5. Write `layout.txt` and `meta.json`
6. Update search cache `DomEntry.search_text` with merged content
7. Prune old snapshots (keep last 5) under per-window Mutex with canonical path verification
8. Return enriched result with `snapshotId`, `subtreeFiles`, `allRefsPath`
9. On filesystem error: bounded fallback with `splitFailed: true` (never unbounded inline)

- [ ] **Step 4: Build (expect caller errors -- fixed in Task 4)**

Run: `cargo build -p tauri-plugin-connector 2>&1 | tail -10`
Expected: Errors in callers (mcp_tools.rs, server.rs) due to new parameters

- [ ] **Step 5: Commit**

```
feat: add subtree file writing to dom_snapshot handler
```

---

### Task 4: MCP Tool Dispatch -- Parse maxTokens and Update All Callers

**Files:**
- Modify: `plugin/src/mcp_tools.rs:90-108`
- Modify: `plugin/src/protocol.rs:37-44`
- Modify: `plugin/src/server.rs:139-141`
- Modify: `plugin/src/handlers.rs:1382-1430` (search_snapshot)

- [ ] **Step 1: Extend Command::DomSnapshot**

Add `max_tokens: Option<u64>` and `no_split: Option<bool>` fields to `Command::DomSnapshot` in `protocol.rs`.

- [ ] **Step 2: Update WebSocket dispatch**

In `server.rs`, destructure new fields and forward to `handlers::dom_snapshot`. When `no_split` is true, pass `max_tokens: Some(0)`.

- [ ] **Step 3: Update MCP dispatch with default maxTokens: 4000**

In `mcp_tools.rs`, parse `maxTokens`/`max_tokens` from args with default `Some(4000)`. Pass to handler along with `state`.

- [ ] **Step 4: Add maxTokens to MCP tool definition schema**

Add `"maxTokens": { "type": "number" }` to the `webview_dom_snapshot` tool properties.

- [ ] **Step 5: Update search_snapshot for merged text**

In `search_snapshot`: prefer `entry.search_text` over `entry.snapshot` when available. Use `maxTokens: 0` for fallback re-snapshot calls.

- [ ] **Step 6: Build and verify full workspace**

Run: `cargo build --workspace 2>&1 | tail -10`
Expected: Build succeeds

- [ ] **Step 7: Commit**

```
feat: wire maxTokens through MCP, WebSocket, and search paths
```

---

### Task 5: CLI -- Add --max-tokens, --no-split, and snapshots Subcommand

**Files:**
- Modify: `crates/cli/src/main.rs:48-75`
- Modify: `crates/cli/src/snapshot.rs:22-83`
- Modify: `crates/cli/src/commands.rs:18-60`

- [ ] **Step 1: Add max_tokens and no_split to SnapshotOptions**

- [ ] **Step 2: Update build_snapshot_script to pass maxTokens and return subtrees/meta**

- [ ] **Step 3: Update compact filter to preserve subtree reference lines**

Add `l.includes('subtree:')` to the compact filter condition.

- [ ] **Step 4: Add CLI args and SnapshotActions subcommand**

Add `--max-tokens` (default 4000), `--no-split` to `Snapshot` variant. Add `Snapshots { List, Read { uuid, file } }` subcommand.

- [ ] **Step 5: Update snapshot command to print UUID and subtree info**

Print subtree info to stderr when result contains subtrees.

- [ ] **Step 6: Implement snapshots_list and snapshots_read**

`snapshots_list`: read PID-scoped snapshots dir, print UUID + file count + timestamp.
`snapshots_read`: read file from snapshot dir with canonical path verification.

- [ ] **Step 7: Build and verify**

Run: `cargo build -p tauri-connector-cli 2>&1 | tail -10`

- [ ] **Step 8: Commit**

```
feat: add --max-tokens, --no-split, and snapshots subcommand to CLI
```

---

### Task 6: Bun Script and Skill Documentation

**Files:**
- Modify: `skill/scripts/snapshot.ts`
- Modify: `skill/SKILL.md`

- [ ] **Step 1: Add --max-tokens to Bun script**

Parse `--max-tokens N` from argv, forward as `max_tokens` in the WebSocket message.

- [ ] **Step 2: Update SKILL.md**

Add `maxTokens` parameter to snapshot examples. Add new "Snapshot Budget & Subtree Files" section documenting: default behavior, unlimited mode, subtree file locations, how to read subtrees, repeating sibling collapse, CLI snapshot management commands.

- [ ] **Step 3: Commit**

```
docs: update Bun script and SKILL.md for snapshot budget engine
```

---

### Task 7: Build Verification and Cargo.lock Update

- [ ] **Step 1: Full workspace build**

Run: `cargo build --workspace 2>&1 | tail -20`

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace 2>&1 | tail -20`

- [ ] **Step 3: Commit Cargo.lock if changed**

```
chore: update Cargo.lock after snapshot budget engine
```
