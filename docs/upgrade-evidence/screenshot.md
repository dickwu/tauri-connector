# U4 screenshot implementation and evidence

Date: 2026-09-12. Native runtime evidence is separate from browser, unit, and cross-compile evidence. This file records the screenshot lane; the central acceptance matrix records the integrated result.

## Implemented contract

`plugin/src/screenshot/` provides WKWebView, WebView2, and WebKitGTK adapters plus the protected capture pipeline. `native-screenshot` is independent of `xcap`; `capture-codec` owns the shared image dependency. Defaults enable both native and existing xcap capture. All added platform crates were already in the pinned Tauri/Wry dependency graph. No release/version change was made.

`capture_protected` takes a fixed context, window, source, optional CSS target rectangle, width limit, total deadline, and an async authorization/context/target validator. Its return contains masked PNG bytes and provenance. The picker retains this result in its own authorized record. The rich screenshot handler retains artifacts under opaque `appInstanceId:artifact:uuid` IDs with a five-minute advertised TTL, a 128-entry/32-MiB local cap, and a reservation against the existing shared 64-MiB workflow/inspection retention budget. Expiry and revocation release that reservation. Idle expiry runs every 30 seconds; reads reject immediately at expiry. Artifact access has no filename/path branch, and legacy `shot_` disk artifacts are a separate store.

Explicit sources never cross-fallback. `auto` tries WebView native, xcap window, then the host-bundled snapdom renderer, recording each failed source. xcap is restricted to an exact title and current process with one unique match, then requires provable native client bounds. Unproven extents fail required redaction. The old no-source path remains in the existing handler, with added actual `captureSource` metadata. Legacy save failures now preserve the successful screenshot with an independent warning.

The trusted `geometry` runtime module uses the same semantic resolver for strict target matching. It retains the node, entity-relevant attributes, text signature and rectangle for the bounded capture interval, and validates them before and after capture. It checks document time origin, viewport, scroll, sensitive regions and document extent across capture. A two-animation-frame handshake, bounded to one second, waits for hidden picker decorations to reach the render loop. It does not focus, restore, resize, scroll or synthesize input. Render waits and target timers are cancelled by module disposal.

Native macOS calibration found that a 1120×820-point WKWebView had a 32-point title-bar safe inset and a 1120×788 CSS viewport. A default snapshot allocated 2240×1640 pixels although content already started at its raster origin. Passing the whole `safeAreaRect` incorrectly removed the first 32 CSS content rows; independent corner markers rejected that candidate. The implemented configuration retains native bounds origin and takes the public safe-area **size**, producing 2240×1576 pixels. All four colored corner markers match the measured `[2,0,0,2,0,0]` transform exactly. No geometry tolerance was relaxed. `safeAreaRect` is checked for runtime availability before use.

The mask pipeline handles password inputs, sensitive field heuristics, `data-connector-sensitive`, `data-sensitive="true"`, and additional application-declared viewport rectangles in `window.__CONNECTOR_SENSITIVE_REGIONS__`. Canvas/video/frame/object/embed hosts are fully masked. Visible custom/shadow boundaries, invalid declarations, excessive tree/region counts, sensitive overflow or unproven visual-viewport mapping reject required capture. This is declared-region and field-heuristic coverage, not a claim to identify every business secret.

Masks precede crop, resize, artifact storage and output encoding. The DOM renderer masks its controlled canvas before base64 traverses the bridge; Rust verifies and masks again before retention. PNG/JPEG/WebP requests transcode only the already-masked image. Protected disk persistence has not been configured: `save:true` returns the completed image and a `protected_disk_storage_unavailable` warning, never writes to a public temporary directory. Required redaction and passive window preparation are host policy; clients cannot downgrade them. Existing no-source annotation remains available; protected `annotate:true` currently returns an explicit unsupported-feature error.

Native callbacks run on the owning UI loop/apartment and transfer owned data through a oneshot. There is no blocking `recv_timeout`. Raw dimensions are capped at 16 MiPixels, raw buffers at 64 MiB, encoded buffers at 16 MiB. At most two native callbacks may be outstanding; their owned permits survive caller timeout/cancellation until callback completion/destruction. At most two codec/xcap workers run, and worker permits survive an abandoned async waiter. Late results cannot register an artifact. The retained-byte budget is shared; transient image buffers have separate stated caps.

## Verification performed

| Command / layer | Result |
|---|---|
| `cargo test -p tauri-plugin-connector --lib screenshot:: -- --nocapture` | Passed 11 tests, including the native-callback permit regression and shared retention-budget release assertions. |
| `node plugin/src/screenshot/geometry.test.cjs` | Passed 10 Chromium contract checks. Its DOM renderer is a deterministic test renderer; this is not native rendering evidence. |
| `cargo check -p tauri-plugin-connector --no-default-features` | Passed on macOS; intentional feature warnings were subsequently cleaned with cfg gates. |
| `cargo check -p tauri-plugin-connector --no-default-features --features xcap` | Passed on macOS. |
| `cargo check -p tauri-plugin-connector --no-default-features --features native-screenshot` | Passed on macOS. |
| `cargo check -p tauri-plugin-connector --features native-screenshot` | macOS native code compiled; early attempts were blocked by concurrent state wiring, then screenshot tests linked and passed. |
| `cargo check -p tauri-plugin-connector --target x86_64-pc-windows-msvc --no-default-features --features native-screenshot --target-dir /tmp/tauri-upgrade-windows-check` | Passed from macOS. Plain `cargo check` was used, not xwin. This validates the pinned Windows bindings; it is not Windows execution or a linked application build. |
| `cargo check -p tauri-plugin-connector --target x86_64-unknown-linux-musl --no-default-features --features native-screenshot` | Environment-blocked in GTK/GLib/Cairo dependency build scripts: pkg-config has no Linux sysroot/cross configuration. The screenshot Rust module was not reached, so this does not count as a Linux code-build pass. |
| `CONNECTOR_FIXTURE_OUTPUT=/tmp/tauri-screenshot-native-verified node examples/workflow-fixture/scripts/screenshot-native-test.mjs` | Passed three native WKWebView groups: four corner markers, nine password mask samples, an entirely masked password crop, real button pixels, clipped crop, stable authorized artifact reads, rejected anonymous read, explicit DOM failure, save warning, incompatible comparison, unchanged independent business counters. See `macos-screenshot-native.json` and verified masked PNGs. |
| Native WebView2 / WebKitGTK fixture | Not run in this macOS environment. |

Tests cover measured 1x/2x/fractional scaling, clipped negative CSS rectangles, mapping composition after resize, malformed/unproven geometry, decoded pixel masks, premultiplied BGRA normalization, invalid image/stride bounds, one-shot/late callbacks, retained native callback slots, authorization generation isolation, expiry and no premature capacity eviction. The browser corpus checks password/opaque-host mask geometry, redaction before DOM base64, safe probe metadata, stale entity refusal and visible picker-decoration rejection.

## Per-requirement screenshot status

| Requirement | Code / current evidence | Remaining evidence |
|---|---|---|
| UP-T051 | Three platform adapters implemented; macOS and Windows cross-compile succeed; real WKWebView full and cropped images pass native pixel assertions. | Windows/Linux native images; Linux build. |
| UP-T052 | Native harness requests unavailable explicit DOM source and receives failure without a native fallback result. | Additional backend-specific failure injection. |
| UP-T053 | Only auto expands the ordered source plan; errors and actual source are returned. | Integrated fallback trace. |
| UP-T054 | Existing xcap/DOM handlers retained; no-source remains legacy. | Final existing regression suite/native smoke. |
| UP-T055 | macOS base/xcap/native/default pass; Windows native-only cross-check passes. | Linux build and CI feature combinations. |
| UP-T056 | Unit mappings pass at 1x, 1.25x, 1.5x, 2x; four real 2x WKWebView markers match exactly. | Native 1x and fractional scales. |
| UP-T057 | Negative-origin clipping/crop-resize tests pass; native x=-10,width=30 target clips to a 40×40-pixel visible crop at 2x with expected color. | Additional platforms/scales. |
| UP-T058 | Full fixed context and geometry probes bracket capture. | Native navigation/resize race injection. |
| UP-T059 | Requires readyState interactive/complete after ContentLoading and host page/runtime identity before and after CapturePreview. | Windows native navigation timing fixture. |
| UP-T060 | Pixel/Chromium pre-base64 tests and nine real native password samples pass; the 451×76 password crop is entirely black. | Additional native declaration/boundary scenarios. |
| UP-T061 | Invalid/unknown mapping and uncovered required regions reject the image. | Native zoom/unknown-geometry scenarios. |
| UP-T062 | No automatic input/window preparation in native code; explicit preparation rejected by host policy. Focused fixture state/scroll and independent business counters are unchanged by capture. The harness explicitly focuses before each case, outside the capture operation. | Native unfocused/minimized scenarios. |
| UP-T063 | One-shot, closed receiver and held native callback slot tests; late output has no store registration path. | Native window-close/timeout callback race. |
| UP-T064 | Dimension/stride/decoder/encoder limits and malformed image tests. | Extended fuzz campaign, if required. |
| UP-T065 | Native save:true succeeds with protected-memory image and independent disk-unavailable warning; legacy save warning preserves capture outcome. | Legacy disk I/O fault injection. |
| UP-T066 | Native whole-viewport vs element artifact comparison returns comparable:false/equal:null; contract checks include source/window/image/geometry/mask fingerprint and encoded-byte equality is named honestly. Legacy output adds bytesDifferent and pixelComparison:false while retaining its old alias. | Additional source/mask-change cases. |
| UP-PK026 | Picker consumes native protected element crop after decoration frame handshake. | Actual native picker image. |
| UP-PK027 | Capture returns typed errors; parent picker retains selection separately. | Parent integrated screenshot-failure selection test. |
| UP-PK028 | Capture result is retained and read-only artifact access never recaptures. | Parent picker repeated-get assertion. |
| UP-PK031 | Fixed context and selected-target validation around capture reject a new page image. | Parent native reload/rebuild fixture. |
| UP-PK033 | Native callbacks retain safe ownership, discard closed results, and cannot independently publish artifacts. | Parent native cancel/timeout race. |
| UP-PK034 | Screenshot retention charges existing global budget and releases it on expiry/revocation; local caps reject new entries. | Parent full picker quota verification. |
| UP-PK035 | New opaque memory IDs cannot enter legacy filename-based read/compare/prune. | Parent adapter bypass tests. |
| UP-PK036 | Real build, cross-check, browser mock and native execution statuses explicitly separated. | Windows/Linux native execution; Linux build. |

## Primary-source review

- Apple WKWebView snapshot API: https://developer.apple.com/documentation/webkit/wkwebview/takesnapshot(with:completionhandler:) ; public rectangle and safe-area APIs: https://developer.apple.com/documentation/webkit/wksnapshotconfiguration/rect and https://developer.apple.com/documentation/appkit/nsview/safearearect . Native marked-pixel tests establish the actual raster/content-origin behavior described above.
- Microsoft WebView2 CapturePreview and ContentLoading ordering: https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2#capturepreview
- WebKitGTK asynchronous snapshot and visible-region behavior: https://webkitgtk.org/reference/webkit2gtk/stable/method.WebView.get_snapshot.html
- The pinned H6 reference implementation was reviewed at `4451b2b1d1eb3a817674f60957e148224c24d3a8`; its LICENSE was verified as MIT, copyright 2025 Fireside Development, LLC: https://github.com/hypothesi/mcp-server-tauri/blob/4451b2b1d1eb3a817674f60957e148224c24d3a8/LICENSE . The implementation uses the documented platform APIs with a different async/ownership/privacy pipeline; it does not adopt the reference's blocking wait or automatic window preparation. See the central provenance document for the reference-source ledger.
