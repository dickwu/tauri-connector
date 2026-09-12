# U5 implementation and verification evidence

Status: implementation and integration in progress. This report does not claim native or CI acceptance from browser/unit results.

## Implemented core

- Shared application `PickerService`, atomic request-key fingerprint/deduplication and per-window leases, four active interactions, 128 retained records, five-minute TTL and bounded tombstones.
- Retained metadata and images charge the existing workflow retention budget. Image storage remains protected and separate from legacy pointed/file artifacts.
- A host authority generation identifies shared-token ownership. Revocation prevents further output/selection/capture and leaves a minimal cleanup path.
- Detached service runner preserves the original deadline across transport disconnects. Selection, screenshot and cleanup have separate states; first accepted main terminal state wins.
- Build-embedded picker UI uses the document-start idle input guard, top-layer popover, exact hit element, bounded semantic metadata, revalidated unique candidates, gesture confirmation and bounded event-tail drain.
- Native screenshot task validates the fixed selected node/entity/rect/context before and after capture. Screenshot failure preserves selection; querying cannot recapture.
- Existing persisted workflow unknown effects are recovered before acquiring an inspection lease. Fresh non-Unix hosts retain the documented protected-memory inspection path; durable workflow remains fail-closed.

## Commands already executed

| Command | Observed result | Layer |
|---|---|---|
| `cargo test -p tauri-plugin-connector inspection_authorization_generation --lib` | New missing-method regression failed before implementation, then passed | Rust unit |
| `cargo test -p tauri-plugin-connector picker::tests --lib --no-default-features` | Initial five lifecycle/dedup/quota tests passed; final expanded rerun pending integration | Rust unit |
| `cargo test -p tauri-plugin-connector up_pk033_late_capture --lib --no-default-features` | Failed before adding final monotonic-deadline check in image registration; rerun included below when finalized | Rust unit |
| `node --test plugin/tests/picker.test.cjs` | Initial actual UI assertion failed before implementation; expanded suite passed 19/19, including 100 full drain cycles, 50.19s | Chromium browser |
| `cargo build --manifest-path examples/workflow-fixture/Cargo.toml --locked --target-dir target` | macOS fixture built successfully; ongoing native integration has later rebuilds | Native platform build |
| `node examples/workflow-fixture/scripts/upgrade-native-test.mjs` | First native attempt exposed runtime global-name collision; second exposed immutable Tauri invoke properties. Neither failure is reclassified as native success | Native macOS, failing integration evidence |

Browser coverage includes active visible UI, exact span/SVG/disabled hits, native-browser mouse/touch input, stale covered-hover Enter, visible cancel and toolbar isolation, modifiers/repeat/IME, multi-pointer/cancel/lost capture, privacy of recursively referenced labels, no repeated observations on get, double-click tails, immutable selected results, and 100-cycle listener/timer/RAF/guard cleanup. Browser synthetic edge sequences are identified in the test, not described as OS input.

## Independent review fixes

Cancelled-before-install requests cannot reopen. Final capture registration checks the monotonic deadline under the entry lock. Expired metadata is reaped without evicting unexpired results. Failed activation can acknowledge its own cleanup; unconfirmed cleanup retains the lease and supports bounded later confirmation. Keyboard confirmation rechecks hit occlusion. Sensitive labels/descendants are recursively redacted. Existing image dimensions are preserved when attaching base64.

## Remaining verification

Native input driver sanity, actual selected-element PNG pixels/masks, all four entry points, cancellation/reload/native IPC scenarios, native benchmarks, final Clippy/tests, and per-ID status reconciliation remain required. Windows code can be cross-built here; no Windows native desktop or Linux sysroot/runtime is available on this host. Remote CI is configured but no push/run is authorized.
