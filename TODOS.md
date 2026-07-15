# tauri-connector TODOS

Captured durable items deferred from v0.13.0 multipilot mode planning (`/plan-eng-review` 2026-05-15).
Each TODO has a one-line decision rule for when to pick it up.

## Post-v0.13 architectural debt

### TODO-1: Migrate `enrich_error` manual handler list to compile-time mechanism

**What:** v0.13 ships with a hand-maintained list in `enrich_error` (plugin/src/handlers/user_actions.rs) of which action handlers wrap their errors with `user_actions_since`. Every new action handler must remember to opt in. Migrate to an attribute macro (`#[multipilot_wrapped]`) or trait (`MultipilotHandler`).

**Why:** Forgot-to-wrap is a known bug class. As v0.14-v0.15 add handlers, statistical chance one gets forgotten → user reports "AI didn't see I clicked".

**Decision rule:** Pick up when (a) 2+ wrap-forgotten incidents reported in production, OR (b) handler count exceeds 15. Whichever first.

**Approach options:** Attribute macro is cleaner ergonomics but slow compile-time cost. Trait is faster but forces a signature on every handler. Pick based on which is more painful at the time.

---

### TODO-2: Compound postcondition predicates (AND / OR / NOT)

**What:** v0.13 ships with 4 atomic predicates (`element_present`, `text_equals`, `attr_equals`, `aria_state_is`). Real-world checks may require composition (e.g., "element exists AND aria-selected=true").

**Why:** When a single atomic predicate is insufficient for a meaningful divergence check, AI must either: (a) accept false-negative warnings, or (b) chain multiple wrapped calls — both lose signal.

**Decision rule:** Pick up on first concrete case where a single predicate gives false Diverged / false Satisfied during real admin/front/tool usage. Until then, YAGNI.

**Approach:** Extend the JSON schema with `{"type": "and", "predicates": [...]}` etc. Update Rust-generated JS template to short-circuit evaluate.

---

### TODO-3: `action_counter` cross-restart uniqueness via `process_start_ts` prefix

**What:** Current `action_counter: Arc<AtomicU64>` resets to 0 on process restart. Action IDs after restart can collide with persisted entries from before restart. Add a process-start timestamp prefix: `action_id = format!("a-{}-{}", process_start_ts, counter)`.

**Why:** Persisted `user_actions.log` outlives the process. After a restart, `enrich_error`'s `action_id` filter could match entries from a previous run, leading to incorrect attribution.

**Decision rule:** Pick up if (a) a user reports unexpected `intent_conflict_with` attribution that traces back to a prior session, OR (b) we add multi-session log replay tooling that requires globally unique IDs.

**Approach:** Single-line change to `action_id` generation. Migrate persisted format with a parser tolerant of both `a-N` and `a-TIMESTAMP-N` formats for one release.

---

### TODO-4: `user_actions.log` auto-prune (currently doctor-warning-only)

**What:** Layer 1 ships with `doctor` displaying a size warning when `user_actions.log` exceeds 10MB. No auto-prune. Heavy typing (keydown + keyup + input on autocomplete fields) can push the file past 10MB in a multi-hour debug session.

**Why:** A 50MB+ log slows `enrich_error`'s `read_jsonl_filtered` tail to ~100ms+ per error response. Users will start noticing.

**Decision rule:** Pick up when (a) a user reports a slow error response, OR (b) telemetry shows median log size in active sessions exceeds 20MB.

**Approach options:**
1. **Time-bounded:** drop entries older than 7 days
2. **Size-bounded:** keep last N MB, drop older
3. **Event-kind allow-list:** debounce keyup events (most volume, least signal)
Option 3 is most surgical; consider first.

---

## Deferred from v0.13 scope (already in design doc roadmap)

These live in the v0.13 design doc's "Deferred / v0.14+ Roadmap" section. Reproducing pointers here so TODOS.md is the single index.

- **Layer 5 — MCP `notifications/message` channel for user action stream** (review-driven cut from v0.13)
  - Trigger: after 4 weeks of v0.13 use, if `get_user_actions` polling exceeds 3 polls/min sustained OR external user requests live streaming
- **Cursor / Codex / other MCP client compatibility for streaming** — assess only after Claude Code 2.0+ streaming UX is GA-validated
- **External user demand discovery** — weeks 5-8 (post-v0.13): r/tauri post, Tauri Discord ping, direct outreach to 5 known agentic Tauri builders. Without this, R1 rebut stays self-only.
