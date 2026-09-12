# Upgrade an existing app and run a smoke test

Use this guide when a user asks to install or upgrade the connector in an existing Tauri application and verify it. An upgrade/test request does not imply releasing that application or submitting its business forms. Preserve the user's requested test path when they specify one.

## Choose the app and scope the upgrade

1. Read the target application's local instructions, Cargo manifest, feature gates, Tauri config and launch scripts. Record its uncommitted changes and existing lockfile before changing dependencies.
2. Check `tauri-connector --version`, `tauri-connector update --check` and the plugin's published stable version. Updating the CLI does not update a plugin already compiled into an app.
3. Change the existing `tauri-plugin-connector` requirement to the selected compatible release. Preserve the existing optional feature name and dependency pins. For the 0.16 release, the focused update is:

   ```bash
   cargo update --manifest-path src-tauri/Cargo.toml -p tauri-plugin-connector --precise 0.16.0
   cargo tree --manifest-path src-tauri/Cargo.toml --locked --features connector -i tauri-plugin-connector
   ```

   `connector` is an example existing feature; use the app's actual name. Compare the lockfile against the pre-upgrade copy, including changes that were already uncommitted. A bare `cargo update` would refresh unrelated dependencies.
4. Run `tauri-connector doctor --no-runtime` for configuration checks. Rebuild and restart the app before claiming the running plugin has changed. Do not run a release/deployment script merely to test compilation.

## Isolate a test from other work

Inspect active app, dev-server and release processes before launching. Resolve physical paths: different Git checkouts can symlink `src-tauri/target` to the same directory. Separate checkout names alone do not establish isolation.

When a build or release owns shared artifacts:

- Set a fresh `CARGO_TARGET_DIR` for the test. Do not clean, copy over or rebuild into the active target directory.
- Verify that frontend output/cache directories are separate too. Read the development script: some start package synchronizers or background Git pulls. If that would affect unrelated work, start the frontend directly and override Tauri's `build.beforeDevCommand` so it cannot invoke that script indirectly.
- A temporary Tauri `--config` file can set a unique application `identifier`, the test server's actual `build.devUrl`, an empty `build.beforeDevCommand`, and a visible test window. Keep the application's normal config and feature gates intact.
- A distinct identifier isolates state that the app stores through Tauri's application-data path. Inspect any hardcoded storage, authentication or background connections separately; the identifier does not sandbox arbitrary application code.

For a Next.js app whose scripts support this arrangement, the launch shape is:

```bash
# Start the frontend on a verified free port; do not assume its usual port is free.
bun --bun next dev --turbopack --hostname 127.0.0.1 --port <free-port>

# In the test host environment, with the prepared temporary config:
CARGO_TARGET_DIR=<fresh-test-target> bun run tauri dev --no-watch --config <temporary-config.json> -- --locked
```

Use the project's actual frontend command and inspect local `tauri dev --help` before adapting flags. Preserve launcher/process handles so cleanup can stop only this test's processes. Do not stop another app or release just to make a preferred port available.

## Bind to the process you actually launched

Use the new process's startup output or fresh `.connector.json`. The PID file is normally in the executable's target parent (`<target>/.connector.json`), including a custom `CARGO_TARGET_DIR`; use the recorded path rather than assuming `target/debug`.

Verify `pid`, `exe`, `app_id`, `started_at`, `ws_port` and `mcp_port` against the test launch. A stale PID file or an occupied port does not identify the desired app. Do not delete another process's discovery file.

Use the embedded MCP endpoint only after matching it to that process. Otherwise target the verified WS port explicitly:

```bash
tauri-connector --host 127.0.0.1 --port <verified-ws-port> state
tauri-connector --host 127.0.0.1 --port <verified-ws-port> windows
tauri-connector --host 127.0.0.1 --port <verified-ws-port> --window-id main bridge
tauri-connector --host 127.0.0.1 --port <verified-ws-port> workflow capabilities
```

Confirm the intended window and frontend URL. Two instances may share a normal app identifier, so also match the PID and executable. CLI auto-discovery can use `TAURI_CONNECTOR_PID_FILE`; explicit ports remain useful when several apps are running.

## Authenticate without storing the credential in the test

Generate an ephemeral `TAURI_CONNECTOR_WORKFLOW_TOKEN` with at least 32 bytes and pass the same value privately to the app and test client. Configure it before launching the host. Do not print it, place it in the workflow spec, pass it as a process argument, or save it in a checked-in config/report.

Capabilities must report workflow protocol support and `authentication.configured: true`. This proves a host token is configured; successful authenticated `workflow_run`/`get` proves the client supplied the matching value.

## Exercise a bounded local flow

Choose a benign existing UI action or a clearly isolated test form. If no safe business fixture exists, a temporary DOM panel in the actual WebView can contain a labelled textarea, a check button and an output element. Its check handler copies the input into the output's `data-value` attribute and increments the panel's `data-clicks` attribute from `"0"`; it must not call business APIs or attach to an existing submit handler.

Use a unique scope and test IDs such as `connector-smoke-panel`, `connector-smoke-input`, `connector-smoke-check` and `connector-smoke-output`. Remove the panel after testing. This tests the connector's real WebView path but does not prove the application's React-controlled form submission or backend behavior; use the actual controlled component and an isolated backend fixture when those are the requested claims.

A useful single workflow is:

1. Wait until the unique panel is visible.
2. Fill the scoped input from `inputs`; verify the resulting value.
3. Click the scoped local check button once; wait for its explicit output condition.
4. Query the output value and pass it into a `result` condition.
5. Query the local click counter and require `"1"` in the final `goal`.

Include quotes, a backslash, a newline and Unicode in the test value. Use `schemaVersion: 1`, a fresh `runKey` and an explicit `goal`. The [MCP reference](mcp-tools.md) defines the expression and condition shapes.

### Example workflow

A complete five-step spec for the isolated local form described in this guide. Create that fixture first; these test IDs must not address business controls. Use a new `runKey` for a new test and retain it unchanged for a lost-response retry.

```json
{
  "schemaVersion": 1,
  "runKey": "isolated-smoke-unique-001",
  "deadlineMs": 30000,
  "inputs": {
    "message": "Connector \"smoke\" \\ path\n中文 ${literal}"
  },
  "steps": [
    {
      "id": "ready",
      "op": "wait",
      "condition": {
        "kind": "element",
        "target": {
          "by": "testId",
          "value": "connector-smoke-panel"
        },
        "state": "visible"
      }
    },
    {
      "id": "fill",
      "op": "fill",
      "target": {
        "by": "testId",
        "value": "connector-smoke-input",
        "scope": {
          "by": "testId",
          "value": "connector-smoke-panel"
        }
      },
      "value": {
        "fromInput": {
          "key": "message"
        }
      },
      "expect": {
        "kind": "valueEquals",
        "target": {
          "by": "testId",
          "value": "connector-smoke-input",
          "scope": {
            "by": "testId",
            "value": "connector-smoke-panel"
          }
        },
        "expected": {
          "fromInput": {
            "key": "message"
          }
        }
      }
    },
    {
      "id": "check",
      "op": "click",
      "target": {
        "by": "testId",
        "value": "connector-smoke-check",
        "scope": {
          "by": "testId",
          "value": "connector-smoke-panel"
        }
      },
      "expect": {
        "kind": "attributeEquals",
        "target": {
          "by": "testId",
          "value": "connector-smoke-output",
          "scope": {
            "by": "testId",
            "value": "connector-smoke-panel"
          }
        },
        "name": "data-value",
        "expected": {
          "fromInput": {
            "key": "message"
          }
        }
      }
    },
    {
      "id": "read",
      "op": "query",
      "target": {
        "by": "testId",
        "value": "connector-smoke-output",
        "scope": {
          "by": "testId",
          "value": "connector-smoke-panel"
        }
      },
      "query": {
        "kind": "attribute",
        "name": "data-value"
      },
      "expect": {
        "kind": "result",
        "stepId": "read",
        "pointer": "/value",
        "operator": "eq",
        "expected": {
          "fromInput": {
            "key": "message"
          }
        }
      }
    },
    {
      "id": "count",
      "op": "query",
      "target": {
        "by": "testId",
        "value": "connector-smoke-panel"
      },
      "query": {
        "kind": "attribute",
        "name": "data-clicks"
      },
      "expect": {
        "kind": "result",
        "stepId": "count",
        "pointer": "/value",
        "operator": "eq",
        "expected": "1"
      }
    }
  ],
  "goal": {
    "kind": "all",
    "conditions": [
      {
        "kind": "result",
        "stepId": "read",
        "pointer": "/value",
        "operator": "eq",
        "expected": {
          "fromInput": {
            "key": "message"
          }
        }
      },
      {
        "kind": "result",
        "stepId": "count",
        "pointer": "/value",
        "operator": "eq",
        "expected": "1"
      }
    ]
  }
}
```

After success, resubmit the identical spec once with the same key. Verify that the returned `runId` is unchanged and independently inspect the counter: it must still be one. Do not repeat a real business write to test deduplication.

Required pass evidence: `status: completed`, `goalStatus: passed`, the expected completed-step count, exact input round-trip and the independent side-effect count. Save compact redacted reports; do not infer correctness merely from a successful transport response.

If an outcome is unknown, inspect `get`, `allowedNextActions` and available reconciliation. Do not reset the journal, remove quarantine or create a fresh key to force the same write through.

## Finish and report the actual boundary

- Remove the temporary UI and stop the processes started for this test, including their dev server. Preserve other processes and the user's working changes.
- Confirm the manifest/lockfile delta contains the intended dependency upgrade and necessary transitive changes. Keep the test credential out of source and artifacts.
- Report the dependency version, build/config/type checks actually run, tested application/window, workflow and goal verdicts, duplicate-submit count and cleanup. Distinguish configuration checks, mock tests, real WebView tests and backend persistence evidence.

A macOS WebView pass does not certify Windows/Linux, every application flow, or a production deployment. Workflow v1 currently refuses durable persistence without supported Unix private-file guarantees; capability errors must not be bypassed by silently switching the requested test to another execution path.


## Inspection upgrade and verification layers

After rebuilding, check `bridge_status.inspectionProtocolVersion` and authenticate `app_identity` before using rich inspection tools. A missing inspection protocol is an explicit unsupported result; do not install arbitrary helper text to emulate it. Bind a returned picker or capture session to its stored application/window/page identity across CLI, WS and both MCP adapters.

Use the repository's isolated fixture with `npm ci`, `npm run fixture:prepare` and `cargo build --locked --manifest-path examples/workflow-fixture/Cargo.toml --target-dir target`. Test dependencies are pinned in the root npm lockfile and fixture Rust dependencies in its Cargo.lock. Browser tests use the actual shipped JavaScript with locked Playwright and React; no external workspace path or CDN is required.

A picker smoke test must activate UI, select through real input, compare the independent fixture business counters, inspect the selected metadata and actual redacted element image, and verify cleanup. A schema test, mocked event, or static PNG does not satisfy native input/capture verification. Report implementation, unit, integration, native and CI independently, with separate macOS, Windows and Linux states; Linux Xvfb is a native WebKitGTK session but is not an interactive physical desktop. Windows workflow journal ACL remains fail-closed and is not relaxed by screenshot support.

The implementation's complete 144-ID acceptance record lives in `docs/upgrade-acceptance.json`, with human-readable evidence in `docs/upgrade-implementation-status.md`. Unrun remote CI or an untested native platform stays unverified. Preserve prior workflow result semantics, run-key immutability, resource arbitration, redaction and quarantine throughout migration.
