# ADR U6-01: Observe locked Tauri invoke through its transport and registered callbacks

Status: implemented; native verification is recorded separately.

The locked Tauri 2.11.5 `scripts/core.js` defines `window.__TAURI_INTERNALS__.invoke`, `ipc`, `postMessage`, and the window's `__TAURI_INTERNALS__` property without writable or configurable attributes. The public global API object is frozen too. Assigning an invoke wrapper either silently does nothing (the previous non-strict bridge code) or throws in strict mode. This was reproduced in the macOS fixture while implementing U6.

`capture/page.js` retains its direct invoke wrapper for writable runtimes. On immutable Tauri it observes the actual mutable transport: custom-protocol `window.fetch` requests to `ipc://localhost/<command>` or `http(s)://ipc.localhost/<command>`, with bounded `window.ipc.postMessage` fallback. It reads only the callback/error routing IDs, command and a bounded input preview. It does not read, transmit, or retain the invoke authentication key or arbitrary headers. The original transport receives the exact receiver and arguments and its return value is returned unchanged.

The exposed Tauri callback Map already contains the success/error callbacks at transport dispatch. The observer replaces only those two callbacks, records the result, then calls the original callback with unchanged receiver and arguments. It restores only callbacks or transport functions it still owns. A transport fallback with the same callback pair is deduplicated. The original immutable invoke, its returned Promise, and its rejection handling are untouched. Connector namespace calls, including the readiness nonce echo, are excluded.

The event `source` accepts the additional explicit value `frontend_tauri_transport_callbacks`; `frontend_invoke_wrapper` remains the direct-wrapper value. Coverage reports `hookSources`, the actual Promise identity behavior, and `preTransportFailures: unobserved_on_immutable_invoke`. This is frontend evidence, never Rust tracing or business persistence evidence. Failures before a request reaches the immutable runtime's observable transport, startup calls before installation, and oversized fallback envelopes cannot be claimed observed. Missing observations remain explicit.

No reference-server code was copied. The implementation was written against the actual locked Tauri implementation. Its `LICENSE_MIT` and source SPDX MIT/Apache-2.0 headers were read locally. Tauri attribution: Copyright (c) 2017 - Present Tauri Apps Contributors. No dependency was introduced for this adaptation.

Rejected alternatives: redefining non-configurable properties (impossible); replacing only a global entry point (misses imported invoke); observing returned fetch bodies (consumes or clones streams and confuses transport success with invocation success); declaring unsupported and claiming U6 complete (does not meet the requested implementation).

Regression evidence: `plugin/tests/capture.test.cjs` exercises readonly descriptors, callback success and rejection identity, unhandled rejection in a child Node process, exact header forwarding, secret-key exclusion, custom-protocol fallback deduplication, and cleanup under later wrappers. Node tests emulate the locked native interface and are not themselves native WebView proof.
