# U3 semantic implementation evidence

Implementation: `plugin/src/semantic/core.js`, semantic-only snapshot blocks in `plugin/src/bridge.rs`, `plugin/src/workflow/page.js`, `plugin/js/locator.js`, legacy wait/ref helpers in `plugin/src/handlers.rs`, and optional modern ref identity fields in `plugin/src/state.rs`. CLI ref field preservation is integrated by the adapter workstream. Design, license review, supported coverage and migration cases are recorded in `docs/adr/semantic-core-v1.md`.

The tests use the actual shipped semantic/workflow/legacy locator sources and extract the actual snapshot/wait source from the Rust embedding sites; there is no second implementation used as the oracle. Browser evidence is Chromium, **not native WKWebView/WebView2/WebKitGTK evidence**. The root acceptance report must preserve that distinction.

## Commands and evidence

| Command | Result | Evidence |
|---|---|---|
| `node plugin/tests/semantic/core.test.cjs` before implementation | Expected failure: core source absent | `semantic-before.log` |
| `node plugin/tests/semantic/core.test.cjs` after adding independent review's nested-sensitive-reference regression, before fixing it | Expected assertion failure | `semantic-privacy-before.log` |
| `node plugin/tests/semantic/core.test.cjs` | 19 browser checks passed, zero failures; Chromium 153.0.8010.12 | `semantic-browser.log` |
| `node plugin/tests/workflow/page.test.cjs` | 27 browser checks passed, zero failures, real React fixture verified | `semantic-workflow-browser.log` |
| `node --test plugin/tests/locator.test.cjs` | 3 unit checks passed, zero failures | `semantic-locator-unit.log` |
| `node --test plugin/tests/picker.test.cjs` | 9 browser tests including parent suite passed, zero failures | `semantic-picker-browser.log` |
| `cargo test -p tauri-plugin-connector modern_ref_resolver_never_falls_back --no-default-features -- --nocapture` | Passed: 1 Rust test, zero failures (126 filtered); no-default-features plugin test build | `semantic-rust.log` |
| `git diff --check` | Passed at handoff | Local command output |

The initial Rust command mistakenly requested a nonexistent `connector` feature and was corrected, without changing feature definitions. An early correct-command attempt was blocked while another workstream had not yet added `PluginState.screenshot`; neither attempt is presented as passing.

## Per-requirement verification

All listed U3 requirements have implemented code. `unitVerified` below means executed JS function assertions or the stated Rust/unit check; `integrationVerified` means the local shipped-source browser integration, not an external four-entry or native application test. CI jobs are configured by the CI workstream; no hosted CI execution is asserted here.

| ID | Implemented | Unit / local browser integration evidence | Native | CI |
|---|---|---|---|---|
| UP-T037 | Yes | Passed: same ordered name/role resolves exact element and appears in snapshot with semanticVersion | Not run | Not run |
| UP-T038 | Yes | Passed: ordered IDREFs, missing IDs, hidden referenced root, hidden descendants of visible references, fallback precedence | Not required by this corpus row | Not run |
| UP-T039 | Yes | Passed: associated/nested native labels, image icon alternative, submit default, valid role-token fallback, password mapping | Not run | Not run |
| UP-T040 | Yes | Passed: description separately emitted, cannot resolve it as the name | Not run | Not run |
| UP-T041 | Yes | Passed: presentation wrapper skipped without discarding button descendant | Not run | Not run |
| UP-T042 | Yes | Passed: ownership graph cycles and duplicates terminate; snapshot counts each DOM node once | Not run | Not run |
| UP-T043 | Yes | Passed: shared semantic/legacy-wait checks distinguish inherited hidden, visual exposure, inert, enabled and editable | Not run | Not run |
| UP-T044 | Yes | Passed: ambiguous legacy action and strict workflow/scope reject before dispatch | Not run | Not run |
| UP-T045 | Yes | Passed: scope/name/role/entity intersection and nested scope ambiguity | Not run | Not run |
| UP-T046 | Yes | Passed in browser: workflow entity reuse rejection; modern refs reject reused entity, detached/replaced node, stale page and semantics | **Required native layer not run** | Not run |
| UP-T047 | Yes | Passed in existing workflow browser tests: focus replacement, redirected focus and beforeinput replacement cannot become successful fills | **Required native layer not run** | Not run |
| UP-T048 | Yes | Passed locally: token split/subtree/refs/React/portal/overlay; 27 existing workflow checks with real React | **Native layer not run** | Not run |
| UP-T049 | Yes, explicit limits | Passed: shadow/frame traversal requests reject; snapshot opt-in shadow remains a declared boundary rather than workflow support | Not run | Not run |
| UP-T050 | Yes | Passed: bounded resolver rejects incomplete candidates; name output budget returns unavailable, no large output | Native stress not run | Not run |
| UP-T020 (semantic part) | Yes | Passed: active workflow rejects replaced semantic object/version, preserves not-dispatched result | Runtime hot replacement/native handled by runtime workstream | Not run |
| UP-T071 (semantic exclusion part) | Yes | Passed: picker-owned subtree excluded from resolver and snapshots | Picker native layer handled separately | Not run |
| UP-T073 (metadata part) | Yes | Passed: password/markers/chained or nested sensitive names, no safe executable candidate | Picker native layer handled separately | Not run |
| UP-PK024 (semantic candidate part) | Yes | Passed: CSS special characters, unique same-element verifier, entity-bound stale rejection, sensitive no-candidate case | Picker integration handled separately | Not run |
| UP-PK025 (semantic metadata part) | Yes | Passed: 512-byte UTF-8-safe names, huge text unavailable, nested/chained sensitive reference graph, no candidate leakage | Picker native layer handled separately | Not run |

U3.1 shared-core decision and upstream/library/standard/license assessment: implemented ADR. U3.2 separate structural/accessibility/actionability views: implemented. U3.3 all ten requested ARIA states including false/mixed have a browser assertion. U3.4 existing output capabilities are retained; portal cycles and React cache defects were corrected without replacing the compression/traversal system. U3.5 contexts/snapshots/candidates record semanticVersion, active workflow semantic changes reject, and output redaction is never reused as a lookup name.

## Boundaries and review findings

- This is a bounded light-DOM subset, not a complete AccName/WAI-ARIA implementation or platform accessibility tree.
- Strict workflow safety (same run/spec semantics, pre-action resolution, entity check, focus/beforeinput revalidation and native-value verification) remains in workflow code. Modern legacy refs add a private opaque identity check; old serialized refs without identity fields retain their compatibility path and cannot claim the new identity guarantee.
- Name/description privacy now follows descendants, native labels, and ARIA naming/description/ownership chains with a visited set and node/time limits. Failure to establish safety redacts metadata and omits executable candidates.
- No native platform result, benchmark target, CI run, release, tag, push, or deployment is claimed by this subtask.
