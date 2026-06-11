# Snapshot Budget Engine & Subtree Files

How tauri-connector keeps DOM snapshots inside AI tool-result limits without losing information. Read this when a snapshot comes back with `file=subtree-K.txt` markers, when you need to tune `maxTokens`, or when managing snapshot storage.

## Defaults by transport

| Caller | Default budget | Why |
|---|---|---|
| MCP tools | `maxTokens: 4000` | Tool results compete with conversation context |
| WebSocket / Bun scripts / internal | `maxTokens: 0` (unlimited) | Backward compatibility; pass `max_tokens` explicitly to opt in |

Overrides, any transport:

- `maxTokens: N` — raise or lower the inline budget
- `maxTokens: 0` or `noSplit: true` — full inline output (legacy behavior)

## What splitting looks like

When a snapshot exceeds the budget, the inline result becomes a **layout skeleton**: the overall tree structure stays, but heavy subtrees are replaced with `file=subtree-K.txt` markers. Nothing is discarded — every spilled subtree is on disk.

- `meta.subtreeFiles[].path` — absolute path of each spilled file
- `meta.allRefsPath` — points at `refs.json` when the ref map itself spills

## Reading spilled content

- **Read tool** on the `path` field directly, or
- **CLI**: `tauri-connector snapshots list`, then `tauri-connector snapshots read <uuid> <file>` (`layout.txt` is the default file; paths are canonicalized — traversal attempts are rejected)

```bash
tauri-connector snapshots list
tauri-connector snapshots read <uuid>                 # layout.txt (default)
tauri-connector snapshots read <uuid> subtree-0.txt
tauri-connector snapshots read <uuid> refs.json
```

## Search stays complete

`webview_search_snapshot` matches against the merged full text — skeleton plus every subtree file — so content never hides inside spilled files. When hunting for something specific in a big DOM, search is cheaper and more reliable than raising the budget and reading everything inline.

## Storage & lifecycle

- Subtree files are written atomically under `<log_dir>/snapshots/<snapshotId>/` (`0700` dir on unix)
- The active `log_dir` is exposed in `.connector.json` and in backend/debug state; if it can't be initialized, the plugin falls back to a temp `.tauri-connector` directory
- Old snapshot sessions are auto-pruned by mtime — the newest 5 are kept

## Repeating siblings

Runs of 5+ siblings with the same tag + role + ARIA state collapse to 2 examples plus a count marker (think: long lists, table rows). Under an active budget, the collapsed rows are written to a subtree file so the full set remains recoverable.

## CLI `--compact` / `-c`

Keeps lines containing refs *plus* subtree markers, so a compact view never loses a pointer to spilled content.

## Examples

```bash
# MCP -- default 4000-token budget (splits if needed)
webview_dom_snapshot(mode: "ai")

# MCP -- raise the budget for a big page
webview_dom_snapshot(mode: "ai", maxTokens: 8000)

# MCP -- unlimited (legacy behavior)
webview_dom_snapshot(mode: "ai", maxTokens: 0)
webview_dom_snapshot(mode: "ai", noSplit: true)

# MCP -- search across spilled subtrees (context=3 lines)
webview_search_snapshot(pattern: "submit|confirm", context: 3)

# CLI -- with default budget
tauri-connector snapshot -i

# CLI -- larger budget, or full output
tauri-connector snapshot -i --max-tokens 8000
tauri-connector snapshot -i --no-split

# Bun -- opt in to budgeting (default is unlimited over WS)
bun run $SCRIPTS/snapshot.ts ai --max-tokens 4000
bun run $SCRIPTS/snapshot.ts ai --no-split
```
