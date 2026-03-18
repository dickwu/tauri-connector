---
name: tauri-connector-release
description: "Release workflow for tauri-connector. Use when bumping versions, publishing to crates.io, creating GitHub releases, or shipping new versions. Covers version sync across all 4 workspace crates, README/SKILL.md updates, cargo publish, git tagging, and gh release creation. Trigger when user says: release, bump version, publish, ship it, cut a release, cargo publish, new version, tag and release."
---

# Tauri Connector Release

Step-by-step release workflow for the tauri-connector workspace. The project has 4 crates that need version coordination, a plugin published to crates.io, CLI binaries built by GitHub Actions, and skill files to keep in sync.

## Workspace Crates

| Crate | Path | Published | Binary |
|---|---|---|---|
| `tauri-plugin-connector` | `plugin/` | crates.io | no (library) |
| `connector-cli` | `crates/cli/` | no | `tauri-connector` |
| `connector-mcp-server` | `crates/mcp-server/` | no | `tauri-connector-mcp` |
| `connector-client` | `crates/client/` | no | no (library) |

The plugin version is the "source of truth". CLI and MCP server versions should match the plugin. The client is an internal dependency and can version independently.

## Release Checklist

### 1. Decide Version

Determine the new version based on changes since the last release:
- **Patch** (0.3.1 → 0.3.2): bug fixes, clippy fixes, CI changes
- **Minor** (0.3.x → 0.4.0): new features, new MCP tools, new CLI commands
- **Major** (0.x → 1.0): breaking API changes

Check what changed since the last tag:
```bash
git log $(git describe --tags --abbrev=0)..HEAD --oneline
```

### 2. Bump Versions

Update version in all Cargo.toml files that should match:
- `plugin/Cargo.toml` — the plugin version (this is the canonical version)
- `crates/cli/Cargo.toml` — CLI version (should match plugin)
- `crates/mcp-server/Cargo.toml` — MCP server version (should match plugin)

Also update any version references in documentation:
- `README.md` — the `tauri-plugin-connector = "0.X"` dependency line
- `skill/SKILL.md` — the setup section mentioning `tauri-plugin-connector = "0.X"`
- `skill/SETUP.md` — the Cargo dependency line

### 3. Update README & Skill

If there are new features, CLI commands, MCP tools, or changed behavior:
- Update `README.md` with new features, changed usage, or new examples
- Update `skill/SKILL.md` if the skill workflow or available commands changed
- Update `skill/SETUP.md` if setup steps changed

### 4. Verify Build & Clippy

```bash
cargo build --release -p connector-cli -p connector-mcp-server
cargo clippy --all-targets --all-features
cargo test -p connector-cli -p connector-mcp-server -p connector-client
```

The CLI should output the new version:
```bash
target/release/tauri-connector --version
```

### 5. Commit Release Changes

```bash
git add -A
git commit -m "chore: bump to vX.Y.Z"
git push
```

### 6. Publish Plugin to Crates.io

The plugin is the only crate published to crates.io. It has a `links` key so only one version can exist per project.

```bash
cd plugin
cargo publish --dry-run    # verify first
cargo publish              # publish for real
cd ..
```

If `cargo publish` fails with dependency issues, check that all workspace path dependencies are also published or that the plugin's Cargo.toml uses version specs for published deps.

Note: `connector-client`, `connector-cli`, and `connector-mcp-server` are NOT published to crates.io — they are distributed as GitHub release binaries.

### 7. Tag and Push

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

This triggers `.github/workflows/release.yml` which:
1. Builds CLI + MCP server for 5 targets (x86_64/aarch64 Linux, x86_64/aarch64 macOS, x86_64 Windows)
2. Generates SHA256 checksums
3. Creates a GitHub Release with all binaries attached

### 8. Verify Release

```bash
# Watch the release workflow
gh run list --limit 3
gh run watch <run-id> --exit-status

# Check the release
gh release view vX.Y.Z
```

The release should have 10 binaries (2 per platform × 5 platforms) plus SHA256SUMS.txt.

### 9. Post-Release

After verifying:
- Check the crates.io page: https://crates.io/crates/tauri-plugin-connector
- Check the GitHub release page for download links
- If this was a minor/major bump, consider updating the GitHub repo description

## Quick Release (Patch)

For simple patch releases where only code changed (no docs updates needed):

```bash
# 1. Bump versions in all Cargo.toml files
# 2. Build + verify
cargo clippy --all-targets --all-features
target/release/tauri-connector --version  # should show new version

# 3. Commit, publish, tag
git add -A && git commit -m "chore: bump to vX.Y.Z"
git push
cd plugin && cargo publish && cd ..
git tag vX.Y.Z && git push origin vX.Y.Z

# 4. Verify
gh run list --limit 1
gh release view vX.Y.Z
```

## Troubleshooting

### cargo publish fails
- **"crate version already exists"**: You forgot to bump the version
- **"failed to verify package"**: Run `cargo package -p tauri-plugin-connector` to see the error
- **dependency issues**: The plugin uses workspace path deps — make sure `connector-client` isn't referenced by the plugin (it isn't currently)

### GitHub release has no binaries
- Check that the tag matches `v*` pattern (e.g., `v0.3.2` not `0.3.2`)
- Check the workflow run: `gh run list --workflow release.yml`

### Version mismatch
- `tauri-connector --version` should match `plugin/Cargo.toml` version
- If not, rebuild: `cargo build --release -p connector-cli`
