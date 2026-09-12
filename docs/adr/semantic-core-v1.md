# ADR: bounded, shared light-DOM semantics version 1

Status: implemented. Scope: U3.1–U3.5 in the v1.1 upgrade specification.

The four independent name/role interpretations in snapshot, legacy locator, workflow locator, and wait made a name from one tool unreliable as input to another. The new trusted, locally embedded `plugin/src/semantic/core.js` is their common semantic entry point and the picker's candidate verifier. Workflow remains a strict wrapper: every resolution applies scope, role/label/test ID/CSS, name, and entity as an intersection; zero and multiple matches remain failures. New page-owned refs reject changed entity identity, detached/replaced nodes, changed page epoch, and incompatible semantics without fuzzy fallback.

## Reference evaluation and licensing

The reviewed [upstream snapshot at commit 4451b2b](https://github.com/hypothesi/mcp-server-tauri/blob/4451b2b1d1eb3a817674f60957e148224c24d3a8/packages/mcp-server/src/driver/scripts/dom-snapshot.js) delegates role, name, description, states, and `aria-owns` children to `aria-api`; its own traversal handles structure and output. That separation is useful, but its compatibility claims are not evidence for this repository. Its [locked LICENSE](https://github.com/hypothesi/mcp-server-tauri/blob/4451b2b1d1eb3a817674f60957e148224c24d3a8/LICENSE) was read before implementation: MIT, Copyright 2025 Fireside Development, LLC. No upstream source or `aria-api` implementation was copied. New code is independently authored under this repository's MIT license; there is no new production dependency or third-party bundled source requiring added attribution notices.

[Accessible Name and Description Computation 1.2](https://www.w3.org/TR/accname-1.2/) informed ordered ID references, label precedence, hidden-reference handling, and separate description output. A referenced hidden root can contribute its subtree; a visible referenced root does not make unrelated hidden descendants visible. Native form labels and image alternatives contribute names; descriptions do not become locator names. This implementation is an explicit subset, not a conformance claim.

Decision: retain a bounded owned implementation in this increment. Adding a full ARIA library would require a separately pinned bundle, license review, WebView compatibility fixtures, and migration evidence beyond the local light-DOM behavior needed here. The alternative remains viable if future coverage requires it. The shared API means such a replacement need not replace workflow policy or snapshot compression.

## Version and limits

`semanticVersion: "1"` appears in snapshots, modern refs, workflow contexts/coverage, and picker candidates. An active workflow rejects a changed semantic object or version. Matching uses original semantic text; only output is redacted or truncated. Metadata reports unavailable fields when its bounded computation cannot finish. UTF-8 truncation preserves code points.

Supported: ordered/missing IDREFs, `aria-label`, native labels, input/button defaults, image alternatives, valid non-abstract role tokens and the declared native mappings, all ten required ARIA states, presentation descendants, inherited hidden/inert, bounded ownership relationships, visual exposure separate from actionability, and strict light-DOM locator intersections.

Explicit limits: closed/open shadow traversal in workflow and iframe traversal are unsupported; snapshot retains its old opt-in shadow traversal and reports the shared semantic coverage limit. Generated pseudo-element content, full embedded-control value contribution, CSS table accessibility mapping, and native platform accessibility equivalence are unsupported/unobserved. Placeholder is a declared compatibility fallback for otherwise unnamed input/textarea fields. No blanket WAI-ARIA or Playwright equivalence is claimed.

Resolvers cap examined elements, recursion, text size, and elapsed work, and fail rather than return incomplete matches. Snapshot retains token split, subtree reads, refs, React enrichment, virtual-list markers, portal stitching, and overlay priority, adding bounded traversal. Ownership cycles cannot duplicate nodes indefinitely. Page-owned ref identity storage is capped at 20,000 entries and 4 MiB of accounted fingerprint data; oversized/evicted records fail closed, and pagehide clears records.

No native WebView execution was performed by the U3 subtask. JavaScript source is browser tested in the pinned Chromium runner; platform runtime behavior remains a separate integration/native acceptance layer.

## Migration cases

- `aria-labelledby` takes precedence over `aria-label`; legacy snapshot/locator previously disagreed.
- Unknown role tokens no longer override the first recognized fallback token or native role.
- Names no longer include unrelated hidden descendants. Hidden explicitly referenced labels remain usable.
- Password inputs have no implicit textbox role; use their associated label or explicit CSS in authorized workflows.
- Multiple legacy locator action matches now require a unique locator or an explicit positional selector. Workflow never allows positional ambiguity.
- Modern refs become stale when their actual node, semantic identity, entity attributes, page or runtime interpretation changes. Legacy serialized refs without new identity fields retain the old compatibility branch; they have no equivalent identity guarantee.
- `aria-owns`/portal nodes are attached once, with cycles rejected, and the React fiber cache no longer treats the first non-React wrapper as evidence that the entire document lacks React.
