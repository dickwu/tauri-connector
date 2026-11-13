# AGENTS.md

This file provides guidance to Codex (claude.ai/code) when working with code in this repository.

## What this is

Tauri v2 connector: a plugin + CLI + MCP server that lets agents inspect and drive running Tauri desktop apps (DOM snapshots, JS exec, clicks, screenshots, logs, IPC).

## Workspace layout (4 crates, versions locked together)

| Path | Package | What it is |
|------|---------|------------|
| `plugin/` | `tauri-plugin-connector` | Tauri v2 plugin; embedded WS bridge + MCP HTTP server. Edition 2024. |
| `crates/client/` | `connector-client` | Shared WebSocket client. No Tauri deps. Edition 2021. |
| `crates/cli/` | `connector-cli` | CLI binary `tauri-connector`. Edition 2021. |
| `crates/mcp-server/` | `connector-mcp-server` | Standalone stdio MCP binary `tauri-connector-mcp`. Edition 2021. |

All 4 crates share one version — never bump one alone. Releases go through the `/tauri-connector-release` skill (publish order: client → plugin → mcp-server → cli). Push the `vX.Y.Z` tag only after CI has passed on that exact `main` commit; `release.yml` fails the release otherwise.

`examples/workflow-fixture/` (the native test app) is a separate workspace with its own `Cargo.lock`, and CI runs every cargo step `--locked`. After changing a crate version or a `plugin/` or `crates/client/` dependency, refresh that lockfile too and commit it:

```bash
cargo update --manifest-path examples/workflow-fixture/Cargo.toml -p tauri-plugin-connector -p connector-client
```

## Done gate

Run after any code change; all must pass before work is complete:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p connector-cli -p connector-mcp-server -p connector-client
```

If `plugin/` changed, also:

```bash
cargo test -p tauri-plugin-connector
cargo check -p tauri-plugin-connector --no-default-features # docs.rs builds this way
npm test # about 2 minutes; the JS suites also read plugin Rust sources
```

then run the native harness against the fixture app: the commands from `npm run fixture:prepare` onward in README.md § Development verification (sequentially, on an unlocked desktop, with ports 19555/19556 free).

Also run `npm test` when `skill/scripts/`, `scripts/`, `examples/workflow-fixture/`, `package.json`, `package-lock.json` or `.github/workflows/ci.yml` changed. It needs `npm ci && npx playwright install chromium` first, and again after every `package-lock.json` change: with a stale install or a missing Chromium build it hangs (in `plugin/tests/picker.test.cjs`) instead of failing.

If a file was moved, renamed or deleted, or `docs/upgrade-acceptance.json` changed, also `npm run acceptance:check`: the acceptance report links source and test files as evidence and CI fails on a missing path. `docs/upgrade-implementation-status.md` is generated from that JSON — regenerate it with `npm run acceptance:render`.

## Gotchas

- **CI builds, tests and lints `plugin/` (`-D warnings`) under four feature sets** — `''`, `xcap`, `native-screenshot`, `xcap,native-screenshot` — on Linux, macOS and Windows. After touching `#[cfg(feature = …)]` or OS-gated code, run `cargo clippy -p tauri-plugin-connector --all-targets --no-default-features --features "<set>" -- -D warnings` for each set.
- **The workflow journal is Unix-only by design.** Elsewhere it fails closed (`persistence_unavailable`) and the non-Unix tests assert exactly that; tests of successful durable workflows go in the `cfg(unix)` module (`plugin/src/workflow/service_tests.rs`).
- **Editing `ci.yml`**: a new job goes in both `aggregate`'s `needs` and the list in `scripts/ci-aggregate.mjs`. In `native-smoke`, keep rust-cache and the cargo build before the `download-artifact` steps — rust-cache prunes `target/debug` on an inexact restore, deleting a fixture binary staged earlier.
- **Vendored copies.** Each crate is published on its own and must package self-contained, so these files exist twice, guarded by byte-compare tests. Edit the canonical side, then copy it over the other:

  | Canonical | Copy |
  |-----------|------|
  | `plugin/src/mcp_tool_schema.rs` | `crates/mcp-server/src/mcp_tool_schema.rs` |
  | `skill/SKILL.md`, `skill/SETUP.md`, `skill/references/*.md` | `crates/cli/skill/` |
  | `plugin/js/scroll.js` | `crates/cli/src/scroll.js` |
  | `examples/inspection/picker-*.json` | `crates/client/tests/fixtures/inspection/` |

  `mcp_tool_schema.rs` stays serde_json-only — its copy compiles inside `connector-mcp-server`, which has no Tauri dependency — and keeps a single `use` line (its header comment explains why).
- **`skill/` is shipped product**, not local config: it's the Codex skill users install via `npx skills add dickwu/tauri-connector`. Behavior changes to CLI/MCP tools need matching updates in `skill/SKILL.md` and `skill/references/` (then the copy above). `skill/SKILL.md` frontmatter `version:` must match the workspace crate version. A new reference doc also needs an entry in `SKILL_DOCS` (`crates/cli/src/skills.rs`).
- **Top-level `cli/` and `server/` are the TypeScript CLI and MCP server** — outside the Cargo workspace, CI and releases, and covering a subset of the Rust tools. Keep them in sync: when you change a tool or command that also exists there, make the matching change and type-check it with `./node_modules/.bin/tsc --noEmit` in that directory (after `bun install`).
- **`.Codex/` is gitignored** — skills and settings there are local-only, not part of the repo.
- **Ports and discovery.** Defaults: WS 9555-9655, MCP HTTP 9556-9656 (`/mcp`, legacy `/sse`), internal bridge 9300-9400. The port rules — port and port-range variables, exact port vs range, precedence, and (since 0.17.8) each app re-binding the ports it used last time before scanning — are specified at the top of `crates/client/src/ports.rs`; client-side endpoint selection and app discovery are in `crates/client/src/discovery.rs`. Read them before touching port or discovery code. A running app advertises itself in `.connector.json` (nearest `target/` above its executable, else `<app-data>/.tauri-connector/`); the CLI and standalone MCP discover it from there.

## Git

Commit straight to `main`. Subjects are imperative sentence case without type prefixes (e.g. "Keep MCP schema parity tests off Tauri GUI deps", "Bump to v0.17.8").
