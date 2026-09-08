# Isolated native workflow fixture

This standalone Tauri/Wry application exercises the actual plugin in a native WebView. Its React form submits through a Tauri command into a small Rust store. The store has no business-service connection, and the harness independently checks its save count and submitted values from an atomic JSON evidence file.

The app uses only local WebSocket port **19555** and MCP port **19556**, plus the plugin's internal bridge. The harness refuses occupied external ports, creates its own random host credential and application identifier, and terminates only the fixture process it starts. Generated React assets, evidence and the fixture lockfile are ignored. No production application or dependency installation is involved.

From the repository root, point the asset generator at an existing React workspace, then build and run:

```bash
CONNECTOR_REACT_MODULE_DIR=/path/to/existing/react-workspace node examples/workflow-fixture/scripts/prepare.mjs
cargo build --manifest-path examples/workflow-fixture/Cargo.toml --target-dir target
node examples/workflow-fixture/scripts/native-test.mjs
```

The full native suite takes about 40 seconds, including the genuine 30-second lost-response timeout. `--skip-unknown` skips that long test for development. `CONNECTOR_FIXTURE_BINARY` can select a different fixture binary; `CONNECTOR_FIXTURE_OUTPUT` can select an evidence directory. Do not point either harness at a production application's binary.

The suite checks:

- The four-step create-task example, strict dialog scope, result binding and goal verification, with quotes, backslashes, newlines, Unicode and template-expression text in an actual React controlled textarea.
- The exact value observed by the Rust save command and one native save per intended action.
- Four separate caller submissions compared with one combined workflow, over the same UI/service path and assertions. Timings are recorded, with no claim of a fixed speedup.
- Two-client run-key deduplication, conflicting inputs, disconnect/resubmit, cancellation and subsequent resource availability.
- A native write that takes over two seconds, an exception after a native write, and a never-returning write response. The latter must report `outcome_unknown`, retain one write, clean pending waiters and block a conflicting write.

The exception test also checks that partial effects retain resource quarantine. The independent lost-response test then starts a fresh fixture process and journal; it never clears or bypasses the prior quarantine.

Evidence includes `native-results.json`, the combined report, the independent native store and the app log. Credentials are not written to those files. A unique application identifier gives each harness invocation a separate local journal; that test journal remains in the platform application-data directory for inspection.

The checked-in records in `validation/macos-native-results.json` and `validation/macos-feature-off-results.json` capture the actual macOS runs. Native Windows and Linux runs remain unverified.

To verify the optional connector feature is absent from a running app:

```bash
cargo check --manifest-path examples/workflow-fixture/Cargo.toml --no-default-features --target-dir target
cargo build --manifest-path examples/workflow-fixture/Cargo.toml --no-default-features --target-dir target
node examples/workflow-fixture/scripts/feature-off-test.mjs
```

Rebuild with default features before running the main native suite again. The feature-off capability directory contains no connector permission; connector permissions are registered only inside the enabled feature.

These tests prove synthetic DOM interactions in an actual native WebView and the isolated Rust fixture's state. They do not prove operating-system-native input, production persistence, business-provider correlation, arbitrary exactly-once execution or untested operating systems. The DOM unit/browser fixture remains in `plugin/tests/workflow/`; it is a separate layer, not a substitute for this native run.
