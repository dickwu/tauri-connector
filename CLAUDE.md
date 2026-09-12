# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Tauri v2 connector: a plugin + CLI + MCP server that lets agents inspect and drive running Tauri desktop apps (DOM snapshots, JS exec, clicks, screenshots, logs, IPC).

## Workspace layout (4 crates, versions locked together)

| Path | Package | What it is |
|------|---------|------------|
| `plugin/` | `tauri-plugin-connector` | Tauri v2 plugin; embedded WS bridge + MCP HTTP server. Edition 2024. |
| `crates/client/` | `connector-client` | Shared WebSocket client. No Tauri deps. Edition 2021. |
| `crates/cli/` | `connector-cli` | CLI binary `tauri-connector`. Edition 2021. |
| `crates/mcp-server/` | `connector-mcp-server` | Standalone stdio MCP binary `tauri-connector-mcp`. Edition 2021. |

All 4 crates share one version — never bump one alone. Releases go through the `/tauri-connector-release` skill (publish order: client → plugin → mcp-server → cli).

## Done gate

Run after any code change; all must pass before work is complete:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p connector-cli -p connector-mcp-server -p connector-client   # what CI runs
```

If `plugin/` changed, also:

```bash
cargo test -p tauri-plugin-connector                        # desktop CI matrix; non-Unix journal tests assert fail-closed
cargo check -p tauri-plugin-connector --no-default-features # docs.rs builds this way
```

then verify against a running app (`/verify-plugin-live`).

## Gotchas

- **CI checks `plugin/` on all three desktop platforms**, installs Linux WebKit/GTK dependencies, verifies screenshot feature combinations and runs isolated native fixture smoke. Windows workflow journal tests assert the existing fail-closed boundary; protected memory inspection is tested separately. Browser helper tests use the locked root npm dependencies.
- **MCP tool schemas live in `plugin/src/mcp_tool_schema.rs`** (canonical), with a byte-for-byte vendored copy at `crates/mcp-server/src/mcp_tool_schema.rs` so the standalone crate packages self-contained for crates.io (a `#[path]` include of the plugin file breaks `cargo publish` — the file falls outside the tarball). After editing the canonical file, re-copy it over the vendored one. Keep the module serde_json-only (no Tauri imports) or the mcp-server build breaks on CI, and keep it to a single `use` line — each crate formats its copy, and editions 2021/2024 sort multi-item imports differently, making `cargo fmt --all -- --check` fail forever. After changing tool definitions, run `cargo test -p connector-mcp-server` — parity tests enforce that the copies are identical and that embedded and standalone schemas match.
- **`skill/` is shipped product**, not local config: it's the Claude Code skill users install via `npx skills add dickwu/tauri-connector`. Behavior changes to CLI/MCP tools need matching updates in `skill/SKILL.md` and `skill/references/`, then a re-copy into `crates/cli/skill/` (a connector-cli test byte-compares them). `skill/SKILL.md` frontmatter `version:` must match the workspace crate version.
- **`.claude/` is gitignored** — skills and settings there are local-only, not part of the repo.
- Plugin runtime defaults: WS bridge on `TAURI_CONNECTOR_PORT` (default 9555), MCP HTTP on port 9556 (`/mcp`, legacy `/sse`); a running app writes `.connector.json` (pid + ports) for auto-discovery.

## Git

Commit straight to `main`. Subjects are imperative sentence case without type prefixes (e.g. "Keep MCP schema parity tests off Tauri GUI deps").
