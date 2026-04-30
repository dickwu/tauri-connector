//! `tauri-connector doctor` — diagnose the current project's setup.
//!
//! Walks the working directory, inspects Tauri plugin files, the frontend
//! `package.json`, `.mcp.json`, and the runtime `.connector.json` PID file,
//! then probes live connectivity to the WebSocket and MCP ports. Anything
//! missing is reported with a concrete fix instruction. Exits non-zero when
//! one or more required checks fail so it is CI-friendly.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use connector_client::ConnectorClient;

use crate::skills;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const SYM_OK: &str = "✓";
const SYM_FAIL: &str = "✗";
const SYM_WARN: &str = "!";

const DEFAULT_WS_PORT: u16 = 9555;
const DEFAULT_MCP_PORT: u16 = 9556;
const PROBE_HOST: &str = "127.0.0.1";

// Canonical snippets used in Fix messages. Kept in one place so we render the
// exact same text the README/SETUP doc ship with. The default snippet uses
// the recommended feature-gated form; the legacy `cfg(debug_assertions)` form
// is still accepted by the doctor — see README "Quick Start → Alternative".
const SNIPPET_PLUGIN_REGISTER: &str = r#"#[cfg(feature = "dev-connector")]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}"#;

const SNIPPET_CAPABILITY: &str = r#"{
  "permissions": ["connector:default"]
}"#;

const SNIPPET_CAPABILITY_DEV: &str = r#"{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "dev-connector",
  "description": "Permissions for tauri-plugin-connector dev tooling. Lives outside capabilities/ so tauri-build's default ./capabilities/**/* glob does NOT auto-load it. Registered at runtime via app.add_capability(include_str!(...)) gated on cfg(feature = \"dev-connector\").",
  "windows": ["main"],
  "permissions": ["connector:default"]
}"#;

const SNIPPET_FEATURES_BLOCK: &str = r#"[features]
default = []
dev-connector = ["dep:tauri-plugin-connector"]"#;

const SNIPPET_RUNTIME_ADD_CAPABILITY: &str = r#"// in setup(|app| { ... })
#[cfg(feature = "dev-connector")]
app.add_capability(include_str!("../capabilities-dev/dev-connector.json"))?;"#;

const SNIPPET_WITH_GLOBAL_TAURI: &str = r#""app": {
  "withGlobalTauri": true
}"#;

const SNIPPET_MCP_JSON: &str = r#"{
  "mcpServers": {
    "tauri-connector": { "url": "http://127.0.0.1:9556/mcp" }
  }
}"#;

const SNIPPET_DEV_SCRIPT: &str = r#""scripts": {
  "tauri:dev": "tauri dev --features dev-connector"
}"#;

/// Short version hint used in Cargo.toml fix snippets (e.g. "0.9" from "0.9.0").
fn cargo_version_hint() -> String {
    let mut parts: Vec<&str> = CURRENT_VERSION.split('.').collect();
    parts.truncate(2);
    parts.join(".")
}

/// Indent every line of a snippet by two spaces so it nests cleanly under a
/// Fix: header when rendered by `print_text`.
fn indent_snippet(s: &str) -> String {
    s.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Ok,
    Fail,
    Warn,
}

impl Status {
    fn symbol(self) -> &'static str {
        match self {
            Status::Ok => SYM_OK,
            Status::Fail => SYM_FAIL,
            Status::Warn => SYM_WARN,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Fail => "fail",
            Status::Warn => "warn",
        }
    }
}

struct Check {
    label: String,
    status: Status,
    detail: Option<String>,
    fix: Option<String>,
}

impl Check {
    fn ok(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Ok,
            detail: None,
            fix: None,
        }
    }
    fn fail(label: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Fail,
            detail: None,
            fix: Some(fix.into()),
        }
    }
    fn warn(label: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Warn,
            detail: None,
            fix: Some(fix.into()),
        }
    }
    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

struct Section {
    name: &'static str,
    checks: Vec<Check>,
}

/// Options for the doctor command.
pub struct Options {
    /// Emit machine-readable JSON instead of the text checklist.
    pub json: bool,
    /// Skip live WebSocket/MCP probes (offline/CI mode).
    pub no_runtime: bool,
}

/// Run the doctor. Returns `Err` when one or more FAIL checks were reported so
/// the process exits non-zero; warnings and skips do not fail the run.
pub async fn run(opts: Options) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| format!("Cannot read cwd: {e}"))?;
    let project_root = find_project_root(&cwd);

    let mut sections = Vec::new();

    sections.push(check_environment(&cwd, project_root.as_ref()));

    let pattern = project_root
        .as_ref()
        .map(|r| classify_setup(r))
        .unwrap_or(SetupPattern::None);

    if let Some(root) = project_root.as_ref() {
        sections.push(check_plugin_setup(root, pattern));
    }

    if !opts.no_runtime {
        let pid_info = project_root
            .as_ref()
            .and_then(|r| find_pid_file(r))
            .and_then(|p| read_pid_file(&p).map(|info| (p, info)));
        sections.push(check_runtime(project_root.as_ref(), pid_info).await);
    }

    if let Some(root) = project_root.as_ref() {
        sections.push(check_integration(root));
    }

    if opts.json {
        print_json(&sections, pattern);
    } else {
        print_text(&sections);
    }

    if any_fail(&sections) {
        Err("doctor found problems — see the Fix lines above".to_string())
    } else {
        Ok(())
    }
}

// ----- project detection ---------------------------------------------------

/// Walk up from `start` looking for a Tauri project root (a directory
/// containing `src-tauri/tauri.conf.json`). Returns `None` for non-Tauri dirs.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur: PathBuf = start.to_path_buf();
    for _ in 0..5 {
        if cur.join("src-tauri").join("tauri.conf.json").is_file() {
            return Some(cur);
        }
        if !cur.pop() {
            break;
        }
    }
    // Some projects keep tauri.conf.json at the root (rare, older layout).
    if start.join("tauri.conf.json").is_file() {
        return Some(start.to_path_buf());
    }
    None
}

// ----- section: environment ------------------------------------------------

fn check_environment(cwd: &Path, project: Option<&PathBuf>) -> Section {
    let mut checks = Vec::new();

    checks.push(Check::ok(format!("CLI version {CURRENT_VERSION}")));
    checks.push(Check::ok(format!("Working directory {}", cwd.display())));

    match project {
        Some(root) => checks.push(
            Check::ok("Tauri v2 project detected").with_detail(format!(
                "{}",
                root.join("src-tauri").join("tauri.conf.json").display()
            )),
        ),
        None => checks.push(
            Check::warn(
                "No Tauri project found near the working directory",
                "run from the directory that contains `src-tauri/`:\n  $ cd path/to/your-tauri-app\n  $ tauri-connector doctor",
            )
            .with_detail(
                "setup/runtime checks skipped — open a Tauri v2 project first".to_string(),
            ),
        ),
    }

    Section {
        name: "Environment",
        checks,
    }
}

// ----- setup pattern classification ----------------------------------------

/// Which Tauri-side registration pattern is the project using for the
/// connector plugin? Drives doctor's downstream checks (which capability
/// directory to scan, which cfg gate is acceptable, whether to nudge the
/// user toward migration).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupPattern {
    /// Recommended layout: `optional = true` dep, a `[features] <feature>
    /// = [...]` declaration, a matching `cfg(feature = "<feature>")` gate, and the
    /// capability JSON under `capabilities-dev/` registered at runtime via
    /// `app.add_capability(...)`. Release `tauri build` skips the dep entirely.
    FeatureGated,
    /// Legacy layout: plain (non-optional) dep, a `cfg(debug_assertions)`
    /// gate, and the capability under `capabilities/`. Plugin is still
    /// compiled in release; dead-code elimination strips it at link time.
    Legacy,
    /// Half-migrated: signals from both patterns are present, or neither
    /// pattern matches cleanly even though the plugin is referenced.
    Mixed,
    /// Plugin not registered at all in the project's lib.rs/main.rs.
    None,
}

impl SetupPattern {
    fn as_str(self) -> &'static str {
        match self {
            SetupPattern::FeatureGated => "feature-gated",
            SetupPattern::Legacy => "legacy",
            SetupPattern::Mixed => "mixed",
            SetupPattern::None => "none",
        }
    }
}

/// Read both `src-tauri/src/lib.rs` and `src-tauri/src/main.rs` (whichever
/// exists) and return the concatenated text. Empty string when neither file
/// is readable.
fn read_plugin_source(root: &Path) -> String {
    let candidates = [
        root.join("src-tauri").join("src").join("lib.rs"),
        root.join("src-tauri").join("src").join("main.rs"),
    ];
    let mut combined = String::new();
    for p in &candidates {
        if let Ok(t) = fs::read_to_string(p) {
            combined.push_str(&t);
            combined.push('\n');
        }
    }
    combined
}

/// True when the source mentions the plugin crate in a way that looks like a
/// real registration (init() / ConnectorBuilder).
fn source_mentions_plugin(src: &str) -> bool {
    src.contains("tauri_plugin_connector::init")
        || src.contains("tauri_plugin_connector::ConnectorBuilder")
        || (src.contains("tauri_plugin_connector") && src.contains("ConnectorBuilder"))
}

/// Cargo feature names found in `cfg(feature = "...")` attributes.
fn source_feature_gates(src: &str) -> Vec<String> {
    let mut names = Vec::new();
    for raw in src.lines() {
        let line = raw.trim();
        if !line.contains("cfg") || !line.contains("feature") {
            continue;
        }
        let Some(feature_pos) = line.find("feature") else {
            continue;
        };
        let after_feature = &line[feature_pos + "feature".len()..];
        let Some(eq_pos) = after_feature.find('=') else {
            continue;
        };
        let after_eq = after_feature[eq_pos + 1..].trim_start();
        let Some(quoted) = after_eq.strip_prefix('"') else {
            continue;
        };
        let Some(end) = quoted.find('"') else {
            continue;
        };
        names.push(quoted[..end].to_string());
    }
    names.sort();
    names.dedup();
    names
}

fn source_has_feature_gate_for(src: &str, feature_name: &str) -> bool {
    source_feature_gates(src)
        .iter()
        .any(|name| name == feature_name)
}

/// True when the source contains a `cfg(debug_assertions)` attribute.
fn source_has_debug_assertions_gate(src: &str) -> bool {
    src.contains("cfg(debug_assertions)")
}

/// Pure classifier: takes the project's `src-tauri/Cargo.toml` text and the
/// concatenated plugin-source text and returns the active pattern.
fn classify_setup_from_inputs(cargo_text: &str, src_text: &str) -> SetupPattern {
    if !source_mentions_plugin(src_text) {
        return SetupPattern::None;
    }

    let optional = extract_cargo_dep(cargo_text, "tauri-plugin-connector")
        .map(|(_, opt)| opt)
        .unwrap_or(false);
    let connector_features = features_declaring_dep(cargo_text, "tauri-plugin-connector");
    let cfg_feature = connector_features
        .iter()
        .any(|feature| source_has_feature_gate_for(src_text, feature));
    let any_cfg_feature = !source_feature_gates(src_text).is_empty();
    let cfg_debug = source_has_debug_assertions_gate(src_text);

    let feature_gated = optional && !connector_features.is_empty() && cfg_feature;
    let legacy = !optional && connector_features.is_empty() && cfg_debug && !any_cfg_feature;

    match (feature_gated, legacy) {
        (true, false) => SetupPattern::FeatureGated,
        (false, true) => SetupPattern::Legacy,
        _ => SetupPattern::Mixed,
    }
}

/// Disk-backed convenience wrapper around `classify_setup_from_inputs`.
fn classify_setup(root: &Path) -> SetupPattern {
    let cargo_text =
        fs::read_to_string(root.join("src-tauri").join("Cargo.toml")).unwrap_or_default();
    let src_text = read_plugin_source(root);
    classify_setup_from_inputs(&cargo_text, &src_text)
}

// ----- section: plugin setup -----------------------------------------------

fn check_plugin_setup(root: &Path, pattern: SetupPattern) -> Section {
    let mut checks = vec![
        check_cargo_dependency(root),
        check_plugin_registration(root),
        check_capabilities(root, pattern),
        check_with_global_tauri(root),
        check_snapdom(root),
        check_mcp_json(root),
    ];

    // Feature-gated pattern requires two extra signals beyond the existing
    // checks: a `[features]` block declaring the cargo feature, and a runtime
    // `app.add_capability(include_str!(...))` call that registers the dev
    // capability. We also run these under `Mixed` to give actionable feedback
    // about what's missing.
    if matches!(pattern, SetupPattern::FeatureGated | SetupPattern::Mixed) {
        checks.push(check_features_block(root));
        checks.push(check_runtime_add_capability(root));
        checks.push(check_dev_script_feature(root));
    }

    // Legacy users get a non-blocking nudge to migrate.
    if pattern == SetupPattern::Legacy {
        checks.push(legacy_migration_warn());
    }

    Section {
        name: "Plugin Setup",
        checks,
    }
}

/// `src-tauri/Cargo.toml` should declare a cargo feature that
/// activates the optional `tauri-plugin-connector` dependency. Only emitted
/// under the FeatureGated/Mixed patterns.
fn check_features_block(root: &Path) -> Check {
    let path = root.join("src-tauri").join("Cargo.toml");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::warn(
                "Cannot read Cargo.toml for [features] check",
                format!("ensure {display} is readable"),
            )
        }
    };

    let features = features_declaring_dep(&text, "tauri-plugin-connector");
    if !features.is_empty() {
        Check::ok(format!(
            "[features] {} activates tauri-plugin-connector",
            features.join(", ")
        ))
        .with_detail(display)
    } else {
        Check::fail(
            "[features] block missing a feature for `tauri-plugin-connector`",
            format!(
                "add a [features] block to src-tauri/Cargo.toml so the plugin dep is opt-in:\n{}",
                indent_snippet(SNIPPET_FEATURES_BLOCK)
            ),
        )
    }
}

/// The dev capability JSON should be registered at runtime via
/// `app.add_capability(include_str!("../capabilities-dev/<file>.json"))`. We
/// look for the three substrings (`add_capability(`, `include_str!`,
/// `capabilities-dev`) anywhere in the source — the words project keeps the
/// `include_str!` in a module-level const, so requiring adjacency would miss it.
fn check_runtime_add_capability(root: &Path) -> Check {
    let src = read_plugin_source(root);
    if src.is_empty() {
        return Check::warn(
            "Cannot read lib.rs/main.rs for runtime add_capability check",
            "ensure src-tauri/src/lib.rs or main.rs is readable",
        );
    }
    if has_runtime_add_capability(&src) {
        Check::ok(
            "Capability loaded at runtime via app.add_capability(include_str!(\"../capabilities-dev/...\"))",
        )
    } else if tauri_conf_enables_connector_feature(root)
        && capability_in_dir(root, "capabilities", "connector:default")
    {
        Check::ok(
            "Default capability is loadable because tauri.conf.json build.features enables the connector feature",
        )
    } else {
        Check::warn(
            "Runtime app.add_capability(include_str!(\"../capabilities-dev/...\")) not detected",
            format!(
                "register the dev capability inside `setup(|app| {{ ... }})` so plain `tauri build` does not need to load it:\n{}",
                indent_snippet(SNIPPET_RUNTIME_ADD_CAPABILITY)
            ),
        )
    }
}

/// Three-substring heuristic: a project that contains all three substrings
/// in unrelated contexts could produce a false-positive Ok on this Warn-only
/// check. We accept that risk because the check is non-blocking and the
/// alternative (parsing the whole source for an exact `add_capability(include_str!("../capabilities-dev/..."))`
/// call) is brittle against the words project's module-level-const layout
/// where `include_str!` is bound to a `const DEV_CONNECTOR_CAPABILITY` and
/// then passed by name to `add_capability(DEV_CONNECTOR_CAPABILITY)`.
fn has_runtime_add_capability(src: &str) -> bool {
    src.contains("add_capability(")
        && src.contains("include_str!")
        && src.contains("capabilities-dev")
}

/// Feature-gated installs need a dev command that actually enables the cargo
/// feature; otherwise the dependency is optional but never compiled in.
fn check_dev_script_feature(root: &Path) -> Check {
    let path = root.join("package.json");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::warn(
                "package.json not found for dev script check",
                format!(
                    "add a dev script in your frontend package.json so the plugin feature is enabled:\n{}",
                    indent_snippet(SNIPPET_DEV_SCRIPT)
                ),
            )
        }
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Check::warn(
            "package.json is not valid JSON for dev script check",
            format!("fix the JSON in {display}:\n  $ jq . {display}"),
        );
    };

    let features = connector_feature_names(root);
    if package_scripts_enable_any_feature(&value, &features) {
        Check::ok(format!(
            "package.json dev script enables connector feature ({})",
            features.join(", ")
        ))
        .with_detail(display)
    } else if let Some(feature) = tauri_conf_enabled_connector_feature(root) {
        Check::ok(format!(
            "tauri.conf.json build.features enables connector feature ({feature})"
        ))
    } else {
        Check::warn(
            "dev command does not enable the connector cargo feature",
            format!(
                "ensure the command you use to launch Tauri dev passes the cargo feature:\n{}",
                indent_snippet(SNIPPET_DEV_SCRIPT)
            ),
        )
    }
}

fn package_scripts_enable_feature(value: &Value, feature_name: &str) -> bool {
    let Some(scripts) = value.get("scripts").and_then(|v| v.as_object()) else {
        return false;
    };
    scripts.values().filter_map(|v| v.as_str()).any(|cmd| {
        let mentions_tauri_dev = cmd.contains("tauri dev") || cmd.contains("cargo tauri dev");
        let has_long = cmd.contains(&format!("--features {feature_name}"))
            || cmd.contains(&format!("--features={feature_name}"));
        let has_short = cmd.contains(&format!("-F {feature_name}"))
            || cmd.contains(&format!("-F={feature_name}"));
        mentions_tauri_dev && (has_long || has_short)
    })
}

fn package_scripts_enable_any_feature(value: &Value, feature_names: &[String]) -> bool {
    feature_names
        .iter()
        .any(|feature| package_scripts_enable_feature(value, feature))
}

/// Non-blocking nudge for projects still on the legacy `cfg(debug_assertions)`
/// pattern. Emitted after the per-check helpers so the migration tip appears
/// at the bottom of the Plugin Setup section.
fn legacy_migration_warn() -> Check {
    Check::warn(
        "Using legacy debug_assertions gate — consider migrating to --features dev-connector",
        "the feature-gated pattern keeps the plugin dep (and its xcap / aws-sdk-s3 transitive deps) out of release builds. Migration:\n  1. src-tauri/Cargo.toml dep: tauri-plugin-connector = { version = \"0.11\", optional = true }\n  2. add to Cargo.toml:\n     [features]\n     dev-connector = [\"dep:tauri-plugin-connector\"]\n  3. replace `#[cfg(debug_assertions)]` with `#[cfg(feature = \"dev-connector\")]` in lib.rs/main.rs\n  4. move `connector:default` permission to src-tauri/capabilities-dev/dev-connector.json\n  5. register at runtime in setup():\n       #[cfg(feature = \"dev-connector\")]\n       app.add_capability(include_str!(\"../capabilities-dev/dev-connector.json\"))?;\n  6. add to package.json: \"tauri:dev\": \"tauri dev --features dev-connector\"\nSee README \"Quick Start\" / skill/SETUP.md for the full guide.",
    )
}

/// `src-tauri/Cargo.toml` must depend on `tauri-plugin-connector`.
fn check_cargo_dependency(root: &Path) -> Check {
    let path = root.join("src-tauri").join("Cargo.toml");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::fail(
                format!("Cargo dependency missing ({display} unreadable)"),
                format!("ensure {display} exists and is readable:\n  $ ls -l {display}"),
            )
        }
    };

    if let Some((version, optional)) = extract_cargo_dep(&text, "tauri-plugin-connector") {
        let label = if optional {
            format!(
                "Cargo dependency: tauri-plugin-connector = \"{version}\" (optional, feature-gated)"
            )
        } else {
            format!("Cargo dependency: tauri-plugin-connector = \"{version}\"")
        };
        Check::ok(label).with_detail(display)
    } else {
        let v = cargo_version_hint();
        Check::fail(
            "Cargo dependency `tauri-plugin-connector` is missing",
            format!(
                "add it to {display}:\n  $ cd src-tauri && cargo add tauri-plugin-connector@{v}\nor append under [dependencies]:\n  tauri-plugin-connector = \"{v}\""
            ),
        )
    }
}

/// Backwards-compatible thin wrapper over [`extract_cargo_dep`] that returns
/// only the version string (drops the `optional` flag). Production callers
/// use `extract_cargo_dep` directly; this wrapper exists so the legacy unit
/// tests keep compiling without rewrites.
#[cfg(test)]
fn extract_cargo_version(text: &str, name: &str) -> Option<String> {
    extract_cargo_dep(text, name).map(|(v, _)| v)
}

/// Return the value of `key = "..."` inside an inline table string.
fn extract_quoted_field(inline: &str, key: &str) -> Option<String> {
    let needle = format!("{key} =");
    let idx = inline.find(&needle)?;
    let tail = &inline[idx + needle.len()..];
    let tail = tail.trim_start();
    let tail = tail.strip_prefix('"')?;
    let end = tail.find('"')?;
    Some(tail[..end].to_string())
}

/// Return the value of `key = true|false` inside an inline table string.
/// Single-line form only — multi-line inline tables (closing `}` on a separate
/// line) are not parsed; the Cargo convention is single-line inline tables or
/// dotted-key tables (`[dep.tauri-plugin-connector]`), so this is fine in
/// practice for the dependency forms doctor cares about.
fn extract_bool_field(inline: &str, key: &str) -> Option<bool> {
    let needle = format!("{key} =");
    let idx = inline.find(&needle)?;
    let tail = inline[idx + needle.len()..].trim_start();
    if tail.starts_with("true") {
        Some(true)
    } else if tail.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

/// Like `extract_cargo_version`, but also returns whether the dependency is
/// marked `optional = true` (only meaningful for the inline-table form).
/// Returns `Some((version, optional))` when the dep is found in any
/// dependencies-flavored table; `None` otherwise.
fn extract_cargo_dep(text: &str, name: &str) -> Option<(String, bool)> {
    let mut in_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line.contains("dependencies");
            continue;
        }
        if !in_section {
            continue;
        }

        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();

        // Inline table: { version = "...", optional = true, ... }
        if rest.starts_with('{') {
            let version =
                extract_quoted_field(rest, "version").unwrap_or_else(|| "path|git".to_string());
            let optional = extract_bool_field(rest, "optional").unwrap_or(false);
            return Some((version, optional));
        }

        // Plain string: "x.y" — never optional in this form.
        if let Some(inner) = rest.strip_prefix('"') {
            if let Some(end) = inner.find('"') {
                return Some((inner[..end].to_string(), false));
            }
        }
    }
    None
}

/// True if `[features]` declares `feature_name` with at least one entry that
/// references `dep_name` — accepts both the canonical `"dep:<name>"` form and
/// the legacy bare `"<name>"` form. Single-line array form only (multi-line
/// arrays are not parsed, but the standard Cargo idiom is single-line).
///
/// The section-header detection tolerates trailing inline comments
/// (`[features] # generated`) but does NOT match nested sub-tables like
/// `[features.foo]`, since those define `features.foo`, not `features`.
#[cfg(test)]
fn features_declares_dep(text: &str, feature_name: &str, dep_name: &str) -> bool {
    features_declaring_dep(text, dep_name)
        .iter()
        .any(|name| name == feature_name)
}

fn features_declaring_dep(text: &str, dep_name: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_features = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            // Strip anything after the first `]` so trailing inline comments
            // (`[features] # comment`) still match. `find(']')` is safe because
            // `line.starts_with('[')` already guarantees there is a `[`.
            if let Some(end) = line.find(']') {
                let header = line[..=end].trim();
                in_features = header == "[features]";
                continue;
            }
            // No closing `]` on this line — treat as not a section header.
            continue;
        }
        if !in_features {
            continue;
        }

        let Some((feature_name, rest)) = line.split_once('=') else {
            continue;
        };
        let feature_name = feature_name.trim();
        let rest = rest.trim_start();
        if !rest.starts_with('[') {
            continue;
        }
        let needle_dep = format!("\"dep:{dep_name}\"");
        let needle_bare = format!("\"{dep_name}\"");
        if rest.contains(&needle_dep) || rest.contains(&needle_bare) {
            names.push(feature_name.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

fn connector_feature_names(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join("src-tauri").join("Cargo.toml"))
        .map(|text| features_declaring_dep(&text, "tauri-plugin-connector"))
        .unwrap_or_default()
}

fn tauri_conf_enabled_connector_feature(root: &Path) -> Option<String> {
    let features = connector_feature_names(root);
    if features.is_empty() {
        return None;
    }
    let path = root.join("src-tauri").join("tauri.conf.json");
    let text = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    build_features(&value)
        .into_iter()
        .find(|feature| features.iter().any(|known| known == feature))
}

fn tauri_conf_enables_connector_feature(root: &Path) -> bool {
    tauri_conf_enabled_connector_feature(root).is_some()
}

fn build_features(value: &Value) -> Vec<String> {
    let Some(features) = value.get("build").and_then(|v| v.get("features")) else {
        return Vec::new();
    };
    if let Some(feature) = features.as_str() {
        return vec![feature.to_string()];
    }
    features
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Plugin must be registered via `tauri_plugin_connector::init()` or
/// `ConnectorBuilder::new()...build()` in lib.rs or main.rs. Accepts either
/// a Cargo feature gate that activates the connector dependency or the legacy
/// `cfg(debug_assertions)` gate; surfaces which one was matched.
fn check_plugin_registration(root: &Path) -> Check {
    let candidates = [
        root.join("src-tauri").join("src").join("lib.rs"),
        root.join("src-tauri").join("src").join("main.rs"),
    ];

    for path in &candidates {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if !source_mentions_plugin(&text) {
            continue;
        }
        let connector_features = connector_feature_names(root);
        let feature_gates = source_feature_gates(&text);
        let feature_gate = connector_features
            .iter()
            .find(|feature| feature_gates.iter().any(|gate| gate == *feature))
            .or_else(|| feature_gates.first());
        let gate = if let Some(feature) = feature_gate {
            Some(format!("cfg(feature = \"{feature}\")"))
        } else if source_has_debug_assertions_gate(&text) {
            Some("cfg(debug_assertions)".to_string())
        } else {
            None
        };
        let label = match gate {
            Some(ref g) => format!("Plugin registered in {} ({g})", rel(root, path)),
            None => format!(
                "Plugin registered in {} (no cfg gate detected)",
                rel(root, path)
            ),
        };
        return if gate.is_some() {
            Check::ok(label)
        } else {
            Check::warn(
                label,
                format!(
                    "wrap the plugin registration in a cfg gate so release builds skip it:\n{}",
                    indent_snippet(SNIPPET_PLUGIN_REGISTER)
                ),
            )
        };
    }

    Check::fail(
        "Plugin not registered",
        format!(
            "register the plugin in src-tauri/src/lib.rs (before `.invoke_handler(...)`):\n{}",
            indent_snippet(SNIPPET_PLUGIN_REGISTER)
        ),
    )
}

/// Scan `src-tauri/capabilities/` and (when applicable) `capabilities-dev/`
/// for any `*.json` listing `"connector:default"` in its `permissions` array.
/// Pattern controls which directories are required:
/// - `FeatureGated`/`Mixed`: `capabilities-dev/dev-connector.json` is the
///   canonical home; we still accept matches in `capabilities/` for migration.
/// - `Legacy`/`None`: `capabilities/` is canonical.
fn check_capabilities(root: &Path, pattern: SetupPattern) -> Check {
    let dirs: &[(&str, _)] = &[
        ("capabilities", root.join("src-tauri").join("capabilities")),
        (
            "capabilities-dev",
            root.join("src-tauri").join("capabilities-dev"),
        ),
    ];

    let mut checked_any = false;
    let mut found_in: Option<(&str, PathBuf)> = None;
    for (label, dir) in dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_json = path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("json"))
                .unwrap_or(false);
            if !is_json {
                continue;
            }
            checked_any = true;
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if permissions_contain(&value, "connector:default") {
                found_in = Some((label, path));
                break;
            }
        }
        if found_in.is_some() {
            break;
        }
    }

    match found_in {
        Some((dir_label, p)) => {
            let label_text = format!("Permission \"connector:default\" in {}", rel(root, &p));
            // Surface a Warn when the location does not match the active pattern.
            match (pattern, dir_label) {
                (SetupPattern::FeatureGated, "capabilities")
                    if tauri_conf_enables_connector_feature(root) =>
                {
                    Check::ok(label_text).with_detail(
                        "tauri.conf.json build.features enables the connector feature for this app",
                    )
                }
                (SetupPattern::FeatureGated, "capabilities") => Check::warn(
                    format!("{label_text} (expected under capabilities-dev/)"),
                    format!(
                        "feature-gated setups should keep the capability outside the default tauri-build glob — move it to src-tauri/capabilities-dev/dev-connector.json:\n{}",
                        indent_snippet(SNIPPET_CAPABILITY_DEV)
                    ),
                ),
                (SetupPattern::Legacy, "capabilities-dev") => Check::warn(
                    format!("{label_text} (expected under capabilities/)"),
                    "legacy setups load capabilities from src-tauri/capabilities/ — either move the JSON back, or migrate to the feature-gated pattern (see README \"Quick Start\")",
                ),
                _ => Check::ok(label_text),
            }
        }
        None if !checked_any => match pattern {
            SetupPattern::FeatureGated | SetupPattern::Mixed => Check::fail(
                "No capability JSON files found (expected capabilities-dev/dev-connector.json)",
                format!(
                    "create src-tauri/capabilities-dev/dev-connector.json:\n{}",
                    indent_snippet(SNIPPET_CAPABILITY_DEV)
                ),
            ),
            _ => Check::fail(
                "No capability JSON files in src-tauri/capabilities/",
                format!(
                    "create src-tauri/capabilities/default.json:\n{}",
                    indent_snippet(SNIPPET_CAPABILITY)
                ),
            ),
        },
        None => match pattern {
            SetupPattern::FeatureGated | SetupPattern::Mixed => Check::fail(
                "Permission `connector:default` missing in capabilities-dev/",
                format!(
                    "create or update src-tauri/capabilities-dev/dev-connector.json:\n{}",
                    indent_snippet(SNIPPET_CAPABILITY_DEV)
                ),
            ),
            _ => Check::fail(
                "Permission `connector:default` missing",
                format!(
                    "add \"connector:default\" to the `permissions` array in src-tauri/capabilities/default.json:\n{}",
                    indent_snippet(SNIPPET_CAPABILITY)
                ),
            ),
        },
    }
}

fn capability_in_dir(root: &Path, dir_name: &str, permission: &str) -> bool {
    let dir = root.join("src-tauri").join(dir_name);
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        let is_json = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("json"))
            .unwrap_or(false);
        if !is_json {
            return false;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return false;
        };
        serde_json::from_str::<Value>(&text)
            .map(|value| permissions_contain(&value, permission))
            .unwrap_or(false)
    })
}

/// Walk a Tauri capability JSON and check whether `needle` appears in any
/// `permissions` array (supports both string and `{identifier: "..."}` items).
fn permissions_contain(value: &Value, needle: &str) -> bool {
    let Some(arr) = value.get("permissions").and_then(|v| v.as_array()) else {
        return false;
    };
    arr.iter().any(|p| match p {
        Value::String(s) => s == needle,
        Value::Object(obj) => obj
            .get("identifier")
            .and_then(|v| v.as_str())
            .map(|s| s == needle)
            .unwrap_or(false),
        _ => false,
    })
}

/// `src-tauri/tauri.conf.json` must have `app.withGlobalTauri: true`.
fn check_with_global_tauri(root: &Path) -> Check {
    let path = root.join("src-tauri").join("tauri.conf.json");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::fail(
                "tauri.conf.json unreadable",
                format!("make sure {display} exists and is valid JSON:\n  $ ls -l {display}"),
            )
        }
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Check::fail(
            "tauri.conf.json is not valid JSON",
            format!("fix the JSON in {display}:\n  $ jq . {display}"),
        );
    };

    let with_global = value
        .pointer("/app/withGlobalTauri")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if with_global {
        Check::ok("app.withGlobalTauri: true").with_detail(display)
    } else {
        Check::fail(
            "app.withGlobalTauri is not true",
            format!(
                "set `\"withGlobalTauri\": true` under `\"app\"` in {display} (required for the eval+event fallback):\n{}",
                indent_snippet(SNIPPET_WITH_GLOBAL_TAURI)
            ),
        )
    }
}

/// Look for `@zumer/snapdom` in the nearest `package.json`.
fn check_snapdom(root: &Path) -> Check {
    let path = root.join("package.json");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::warn(
                "package.json not found at project root",
                "if your frontend lives in a subdirectory, install @zumer/snapdom there:\n  $ cd <frontend-dir>\n  $ npm install @zumer/snapdom\n  $ # or: bun add @zumer/snapdom",
            )
        }
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Check::warn(
            "package.json is not valid JSON",
            format!("fix the JSON in {display}:\n  $ jq . {display}"),
        );
    };

    let has = package_has_dep(&value, "@zumer/snapdom");
    if has {
        Check::ok("Frontend dependency: @zumer/snapdom").with_detail(display)
    } else {
        Check::fail(
            "Frontend dependency `@zumer/snapdom` is missing",
            "install the screenshot fallback library:\n  $ npm install @zumer/snapdom\n  $ # or: bun add @zumer/snapdom\n  $ # or: pnpm add @zumer/snapdom",
        )
    }
}

fn package_has_dep(value: &Value, name: &str) -> bool {
    const KEYS: &[&str] = &[
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ];
    KEYS.iter().any(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_object())
            .map(|obj| obj.contains_key(name))
            .unwrap_or(false)
    })
}

/// `.mcp.json` at the project root should register `tauri-connector`.
fn check_mcp_json(root: &Path) -> Check {
    let path = root.join(".mcp.json");
    let display = rel(root, &path);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            return Check::fail(
                ".mcp.json missing",
                format!(
                    "create {display} at the project root:\n{}",
                    indent_snippet(SNIPPET_MCP_JSON)
                ),
            );
        }
    };

    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Check::fail(
            ".mcp.json is not valid JSON",
            format!("fix the JSON in {display}:\n  $ jq . {display}"),
        );
    };

    check_mcp_entry(value.pointer("/mcpServers/tauri-connector"), &display)
}

fn check_mcp_entry(entry: Option<&Value>, display: &str) -> Check {
    match entry {
        Some(Value::Object(obj)) => {
            let url = obj.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if !url.is_empty() {
                return check_mcp_url(display, url);
            }
            if obj.get("command").and_then(|v| v.as_str()).is_some() {
                return Check::warn(
                    ".mcp.json uses standalone command transport",
                    format!(
                        "the embedded plugin now exposes Streamable HTTP directly; prefer this url in {display}:\n{}",
                        indent_snippet(SNIPPET_MCP_JSON)
                    ),
                );
            }
            Check::fail(
                ".mcp.json `tauri-connector` entry has no `url` or `command`",
                format!(
                    "set the embedded MCP Streamable HTTP url in {display}:\n{}",
                    indent_snippet(SNIPPET_MCP_JSON)
                ),
            )
        }
        _ => Check::fail(
            ".mcp.json has no `tauri-connector` entry",
            format!(
                "add a `tauri-connector` entry under `mcpServers` in {display}:\n  \"tauri-connector\": {{ \"url\": \"http://127.0.0.1:9556/mcp\" }}"
            ),
        ),
    }
}

fn check_mcp_url(display: &str, url: &str) -> Check {
    if url.ends_with("/sse") {
        return Check::warn(
            ".mcp.json uses legacy /sse transport",
            format!(
                "prefer the Streamable HTTP endpoint in {display}:\n  \"tauri-connector\": {{ \"url\": \"{}\" }}",
                url.trim_end_matches("/sse").to_string() + "/mcp"
            ),
        );
    }
    if !url.ends_with("/mcp") {
        return Check::fail(
            ".mcp.json URL does not target /mcp",
            format!(
                "set the embedded MCP Streamable HTTP endpoint in {display}:\n  \"tauri-connector\": {{ \"url\": \"http://127.0.0.1:9556/mcp\" }}"
            ),
        );
    }
    if !(url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:")) {
        return Check::warn(
            ".mcp.json URL is not loopback",
            "use a loopback URL so MCP does not expose local app control outside this machine:\n  \"tauri-connector\": { \"url\": \"http://127.0.0.1:9556/mcp\" }",
        );
    }
    Check::ok(format!(".mcp.json registers tauri-connector ({url})")).with_detail(display)
}

// ----- section: runtime ----------------------------------------------------

struct PidInfo {
    pid: Option<u64>,
    ws_port: Option<u16>,
    mcp_port: Option<u16>,
    bridge_port: Option<u16>,
    app_name: Option<String>,
    app_id: Option<String>,
    log_dir: Option<PathBuf>,
}

fn find_pid_file(root: &Path) -> Option<PathBuf> {
    // The plugin writes the PID file next to the dev binary, so it moves between
    // `target/` and `target/debug|release/`. Scan common spots relative to the project.
    let candidates = [
        root.join("src-tauri")
            .join("target")
            .join("debug")
            .join(".connector.json"),
        root.join("src-tauri")
            .join("target")
            .join(".connector.json"),
        root.join("target").join("debug").join(".connector.json"),
        root.join("target").join(".connector.json"),
        root.join("src-tauri")
            .join("target")
            .join("release")
            .join(".connector.json"),
        root.join("target").join("release").join(".connector.json"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn read_pid_file(path: &Path) -> Option<PidInfo> {
    let text = fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some(PidInfo {
        pid: value.get("pid").and_then(|v| v.as_u64()),
        ws_port: value
            .get("ws_port")
            .and_then(|v| v.as_u64())
            .and_then(|n| u16::try_from(n).ok()),
        mcp_port: value
            .get("mcp_port")
            .and_then(|v| v.as_u64())
            .and_then(|n| u16::try_from(n).ok()),
        bridge_port: value
            .get("bridge_port")
            .and_then(|v| v.as_u64())
            .and_then(|n| u16::try_from(n).ok()),
        app_name: value
            .get("app_name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        app_id: value
            .get("app_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        log_dir: value
            .get("log_dir")
            .and_then(|v| v.as_str())
            .map(PathBuf::from),
    })
}

async fn check_runtime(project: Option<&PathBuf>, pid: Option<(PathBuf, PidInfo)>) -> Section {
    let mut checks = Vec::new();

    // PID file presence
    let (ws_port, mcp_port) = match (&pid, project) {
        (Some((path, info)), Some(root)) => {
            let mut detail = format!(
                "app: {}",
                info.app_name.clone().unwrap_or_else(|| "?".into())
            );
            if let Some(id) = &info.app_id {
                detail.push_str(&format!(" ({id})"));
            }
            if let Some(pid) = info.pid {
                detail.push_str(&format!(", pid {pid}"));
            }
            checks.push(Check::ok(format!("PID file: {}", rel(root, path))).with_detail(detail));
            (
                info.ws_port.unwrap_or(DEFAULT_WS_PORT),
                info.mcp_port.unwrap_or(DEFAULT_MCP_PORT),
            )
        }
        (Some((path, info)), None) => {
            checks.push(
                Check::ok(format!("PID file: {}", path.display())).with_detail(format!(
                    "app: {}",
                    info.app_name.clone().unwrap_or_else(|| "?".into())
                )),
            );
            (
                info.ws_port.unwrap_or(DEFAULT_WS_PORT),
                info.mcp_port.unwrap_or(DEFAULT_MCP_PORT),
            )
        }
        (None, _) => {
            checks.push(Check::warn(
                "PID file (.connector.json) not found",
                "start the Tauri app in dev mode — the plugin writes the PID file at startup:\n  $ bun run tauri dev\n  $ # or: cargo tauri dev",
            ));
            (DEFAULT_WS_PORT, DEFAULT_MCP_PORT)
        }
    };

    if let Some((_, info)) = &pid {
        checks.push(check_pid_alive(info));
        checks.push(check_pid_metadata(info));
        checks.push(check_log_dir(info));
    }

    // WebSocket reachable
    match probe_ws(PROBE_HOST, ws_port).await {
        Ok(()) => {
            checks.push(Check::ok(format!(
                "WebSocket ws://{PROBE_HOST}:{ws_port} reachable"
            )));
            match probe_ws_features(PROBE_HOST, ws_port).await {
                Ok(features) => {
                    checks.push(
                        Check::ok("Bridge status command reachable").with_detail(format!(
                            "{} webview bridge client(s), pending {}",
                            features.bridge_clients, features.pending_evals
                        )),
                    );
                    if features.bridge_clients == 0 {
                        checks.push(Check::warn(
                            "No webview bridge clients connected",
                            "open the Tauri window and confirm the connector plugin is registered before the app is built; if this persists, verify app.withGlobalTauri=true and the 9300-9400 bridge port range is not blocked",
                        ));
                    }
                    checks.push(Check::ok(
                        "Runtime, artifact, and debug WebSocket commands are available",
                    ));
                }
                Err(e) => checks.push(
                    Check::fail(
                        "Runtime/artifact/debug WebSocket commands unavailable",
                        "update the app to tauri-plugin-connector 0.11+, restart the Tauri process, then re-run doctor. These commands power `runtime`, `artifacts`, `debug snapshot`, and `act`.",
                    )
                    .with_detail(e),
                ),
            }
        }
        Err(e) => checks.push(
            Check::fail(
                format!("WebSocket ws://{PROBE_HOST}:{ws_port} unreachable"),
                "start the Tauri app so the plugin binds its WebSocket listener:\n  $ bun run tauri dev\n  $ # or: cargo tauri dev",
            )
            .with_detail(e),
        ),
    }

    // MCP Streamable HTTP reachable and protocol-correct.
    match probe_mcp_streamable(PROBE_HOST, mcp_port).await {
        Ok(probe) => checks.push(
            Check::ok(format!(
                "MCP Streamable HTTP http://{PROBE_HOST}:{mcp_port}/mcp protocol OK"
            ))
            .with_detail(format!(
                "protocol {}, session {}, initialize 200, notification 202, ping 200, GET /mcp 405, DELETE 204",
                probe.protocol_version, probe.session_id
            )),
        ),
        Err(e) => checks.push(
            Check::fail(
                format!("MCP Streamable HTTP http://{PROBE_HOST}:{mcp_port}/mcp protocol failed"),
                format!(
                    "start the Tauri app in dev mode and ensure `.mcp.json` points at the Streamable HTTP endpoint. If custom ports are set via ConnectorBuilder.mcp_port_range(...), update .mcp.json to match port {mcp_port}:\n  \"tauri-connector\": {{ \"url\": \"http://127.0.0.1:{mcp_port}/mcp\" }}\n  $ bun run tauri dev"
                ),
            )
            .with_detail(e),
        ),
    }

    Section {
        name: "Runtime",
        checks,
    }
}

fn check_pid_alive(info: &PidInfo) -> Check {
    let Some(pid) = info.pid else {
        return Check::warn(
            ".connector.json has no pid",
            "restart the Tauri app with tauri-plugin-connector 0.11+ so the runtime PID file includes process metadata",
        );
    };
    if process_alive(pid) {
        Check::ok(format!("Runtime process {pid} is alive"))
    } else {
        Check::fail(
            format!("Runtime process {pid} is not alive"),
            "remove the stale `.connector.json`, restart the Tauri app, then re-run doctor:\n  $ rm -f src-tauri/target/.connector.json src-tauri/target/debug/.connector.json\n  $ bun run tauri dev",
        )
    }
}

fn process_alive(pid: u64) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn check_pid_metadata(info: &PidInfo) -> Check {
    let mut missing = Vec::new();
    if info.ws_port.is_none() {
        missing.push("ws_port");
    }
    if info.mcp_port.is_none() {
        missing.push("mcp_port");
    }
    if info.bridge_port.is_none() {
        missing.push("bridge_port");
    }
    if info.app_name.as_deref().unwrap_or("").is_empty() {
        missing.push("app_name");
    }
    if info.app_id.as_deref().unwrap_or("").is_empty() {
        missing.push("app_id");
    }
    if info.log_dir.is_none() {
        missing.push("log_dir");
    }

    if missing.is_empty() {
        Check::ok(".connector.json exposes runtime ports, app identity, and log_dir")
    } else {
        Check::warn(
            format!(".connector.json missing {}", missing.join(", ")),
            "restart the app with the current tauri-plugin-connector. Older PID files cannot support precise doctor/runtime/artifact checks.",
        )
    }
}

fn check_log_dir(info: &PidInfo) -> Check {
    let Some(log_dir) = info.log_dir.as_ref() else {
        return Check::warn(
            "Runtime log_dir not reported",
            "restart the app with tauri-plugin-connector 0.11+ so doctor can verify console/ipc/events/runtime logs and artifacts",
        );
    };
    if !log_dir.is_dir() {
        return Check::fail(
            format!("Runtime log_dir does not exist ({})", log_dir.display()),
            "the PID file is stale or the app cannot create its connector data directory. Restart the app and check filesystem permissions for the app data directory.",
        );
    }
    let mut missing = Vec::new();
    for file in ["console.log", "ipc.log", "events.log", "runtime.log"] {
        if !log_dir.join(file).is_file() {
            missing.push(file);
        }
    }
    if missing.is_empty() {
        Check::ok("Connector JSONL logs initialized").with_detail(log_dir.display().to_string())
    } else {
        Check::warn(
            format!("Connector log files missing: {}", missing.join(", ")),
            "restart the app with the current plugin. The connector should initialize console.log, ipc.log, events.log, and runtime.log under log_dir.",
        )
        .with_detail(log_dir.display().to_string())
    }
}

async fn probe_ws(host: &str, port: u16) -> Result<(), String> {
    let mut client = ConnectorClient::new();
    let connect = client.connect(host, port);
    tokio::time::timeout(Duration::from_secs(2), connect)
        .await
        .map_err(|_| "connect timed out after 2s".to_string())??;

    let ping = client.send_with_timeout(json!({ "type": "ping" }), 2_000);
    let _ = ping.await.map_err(|e| format!("ping failed: {e}"))?;
    client.disconnect().await;
    Ok(())
}

struct WsFeatureProbe {
    bridge_clients: usize,
    pending_evals: u64,
}

async fn probe_ws_features(host: &str, port: u16) -> Result<WsFeatureProbe, String> {
    let mut client = ConnectorClient::new();
    tokio::time::timeout(Duration::from_secs(2), client.connect(host, port))
        .await
        .map_err(|_| "connect timed out after 2s".to_string())??;

    let bridge = client
        .send_with_timeout(json!({ "type": "bridge_status" }), 2_000)
        .await
        .map_err(|e| format!("bridge_status failed: {e}"))?;
    client
        .send_with_timeout(json!({ "type": "runtime_get_captured", "limit": 1 }), 2_000)
        .await
        .map_err(|e| format!("runtime_get_captured failed: {e}"))?;
    client
        .send_with_timeout(json!({ "type": "artifact_list", "limit": 1 }), 2_000)
        .await
        .map_err(|e| format!("artifact_list failed: {e}"))?;
    client
        .send_with_timeout(
            json!({
                "type": "debug_snapshot",
                "include_dom": false,
                "include_screenshot": false,
                "include_logs": false,
                "include_ipc": false,
                "include_events": false,
                "include_runtime": false
            }),
            2_000,
        )
        .await
        .map_err(|e| format!("debug_snapshot failed: {e}"))?;
    client.disconnect().await;

    let bridge_clients = bridge
        .get("clients")
        .and_then(|v| v.as_array())
        .map(Vec::len)
        .unwrap_or(0);
    let pending_evals = bridge
        .get("pending")
        .or_else(|| bridge.get("pendingEvals"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Ok(WsFeatureProbe {
        bridge_clients,
        pending_evals,
    })
}

struct McpProbe {
    protocol_version: String,
    session_id: String,
}

async fn probe_mcp_streamable(host: &str, port: u16) -> Result<McpProbe, String> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {
                "name": "tauri-connector-doctor",
                "version": CURRENT_VERSION
            }
        }
    })
    .to_string();
    let init = raw_http_request(
        host,
        port,
        "POST",
        "/mcp",
        &[
            ("Accept", "application/json, text/event-stream"),
            ("Content-Type", "application/json"),
        ],
        Some(&body),
    )
    .await?;
    if init.status_code() != Some(200) {
        return Err(format!(
            "expected initialize HTTP 200, got {}",
            init.status_line
        ));
    }

    let session_id = init
        .header("mcp-session-id")
        .ok_or_else(|| "initialize response missing MCP-Session-Id header".to_string())?;
    let header_protocol = init.header("mcp-protocol-version");

    let response: Value = serde_json::from_str(&init.body)
        .map_err(|e| format!("initialize response body was not JSON: {e}"))?;
    let protocol = response
        .pointer("/result/protocolVersion")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "initialize response missing result.protocolVersion".to_string())?;
    if !matches!(protocol, "2025-11-25" | "2025-06-18" | "2025-03-26") {
        return Err(format!("unsupported protocolVersion returned: {protocol}"));
    }
    if let Some(header_protocol) = header_protocol.as_deref() {
        if header_protocol != protocol {
            return Err(format!(
                "MCP-Protocol-Version header {header_protocol} did not match initialize result {protocol}"
            ));
        }
    }

    let initialized_body = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    })
    .to_string();
    let initialized = raw_http_request(
        host,
        port,
        "POST",
        "/mcp",
        &[
            ("Accept", "application/json, text/event-stream"),
            ("Content-Type", "application/json"),
            ("MCP-Session-Id", &session_id),
            ("MCP-Protocol-Version", protocol),
        ],
        Some(&initialized_body),
    )
    .await?;
    if initialized.status_code() != Some(202) || !initialized.body.trim().is_empty() {
        return Err(format!(
            "expected initialized notification 202 with empty body, got {} body {:?}",
            initialized.status_line, initialized.body
        ));
    }

    let ping_body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "ping"
    })
    .to_string();
    let ping = raw_http_request(
        host,
        port,
        "POST",
        "/mcp",
        &[
            ("Accept", "application/json, text/event-stream"),
            ("Content-Type", "application/json"),
            ("MCP-Session-Id", &session_id),
            ("MCP-Protocol-Version", protocol),
        ],
        Some(&ping_body),
    )
    .await?;
    if ping.status_code() != Some(200) {
        return Err(format!("expected ping HTTP 200, got {}", ping.status_line));
    }
    let ping_json: Value =
        serde_json::from_str(&ping.body).map_err(|e| format!("ping response was not JSON: {e}"))?;
    if ping_json.pointer("/result").is_none() {
        return Err("ping response missing JSON-RPC result".to_string());
    }

    let get = raw_http_request(
        host,
        port,
        "GET",
        "/mcp",
        &[("Accept", "text/event-stream")],
        None,
    )
    .await?;
    if get.status_code() != Some(405) || !get.header_contains("allow", "POST") {
        return Err(format!(
            "expected GET /mcp 405 with Allow: POST, DELETE; got {}",
            get.status_line
        ));
    }

    let delete = raw_http_request(
        host,
        port,
        "DELETE",
        "/mcp",
        &[("MCP-Session-Id", &session_id)],
        None,
    )
    .await?;
    if delete.status_code() != Some(204) {
        return Err(format!(
            "expected DELETE /mcp HTTP 204, got {}",
            delete.status_line
        ));
    }

    Ok(McpProbe {
        protocol_version: protocol.to_string(),
        session_id,
    })
}

struct RawHttpResponse {
    status_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl RawHttpResponse {
    fn status_code(&self) -> Option<u16> {
        self.status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    fn header_contains(&self, name: &str, needle: &str) -> bool {
        self.header(name)
            .map(|value| {
                value
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
            })
            .unwrap_or(false)
    }
}

async fn raw_http_request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<RawHttpResponse, String> {
    let body = body.unwrap_or("");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n");
    for (key, value) in headers {
        request.push_str(&format!("{key}: {value}\r\n"));
    }
    if !body.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);

    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await
    .map_err(|_| "connect timed out after 2s".to_string())?
    .map_err(|e| format!("tcp connect failed: {e}"))?;

    tokio::time::timeout(Duration::from_secs(2), stream.write_all(request.as_bytes()))
        .await
        .map_err(|_| "HTTP write timed out after 2s".to_string())?
        .map_err(|e| format!("HTTP write failed: {e}"))?;

    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut bytes))
        .await
        .map_err(|_| "HTTP read timed out after 2s".to_string())?
        .map_err(|e| format!("HTTP read failed: {e}"))?;

    let raw = String::from_utf8(bytes).map_err(|e| format!("HTTP response was not UTF-8: {e}"))?;
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "HTTP response missing header/body separator".to_string())?;
    let mut lines = head.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| "HTTP response missing status line".to_string())?
        .to_string();
    let headers = lines
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    Ok(RawHttpResponse {
        status_line,
        headers,
        body: body.to_string(),
    })
}

// ----- section: integration ------------------------------------------------

fn check_integration(root: &Path) -> Section {
    let mut checks = Vec::new();

    let hook_script = root
        .join(".claude")
        .join("hooks")
        .join("tauri-connector-detect.sh");
    let settings = root.join(".claude").join("settings.local.json");

    let script_exists = hook_script.is_file();
    let settings_has_hook = fs::read_to_string(&settings)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .as_ref()
        .map(settings_has_connector_hook)
        .unwrap_or(false);

    match (script_exists, settings_has_hook) {
        (true, true) => checks.push(
            Check::ok("Claude Code auto-detect hook installed")
                .with_detail(rel(root, &hook_script)),
        ),
        (true, false) => checks.push(Check::warn(
            "Hook script present but not wired into settings.local.json",
            "finish wiring the hook:\n  $ tauri-connector hook install",
        )),
        (false, true) => checks.push(Check::warn(
            "Hook entry in settings.local.json but script is missing",
            "regenerate the script:\n  $ tauri-connector hook install",
        )),
        (false, false) => checks.push(Check::warn(
            "Claude Code auto-detect hook not installed (optional)",
            "enable per-prompt detection:\n  $ tauri-connector hook install",
        )),
    }

    let stale_docs = skills::stale_local_skill_docs();
    if stale_docs.is_empty() {
        checks.push(Check::ok(
            "Local tauri-connector skill docs match the bundled CLI skill",
        ));
    } else {
        let details = stale_docs
            .iter()
            .map(|(path, reason)| format!("{} ({reason})", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        checks.push(
            Check::warn(
                "Local tauri-connector skill docs are stale",
                "refresh each stale file from the CLI-bundled docs listed above:\n  $ tauri-connector skills list\n  $ tauri-connector skills get tauri-connector > <skill-dir>/SKILL.md\n  $ tauri-connector skills get mcp-tools > <skill-dir>/references/mcp-tools.md\n  $ tauri-connector skills path mcp-tools",
            )
            .with_detail(details),
        );
    }

    Section {
        name: "Integration",
        checks,
    }
}

fn settings_has_connector_hook(settings: &Value) -> bool {
    let Some(arr) = settings
        .pointer("/hooks/UserPromptSubmit")
        .and_then(|v| v.as_array())
    else {
        return false;
    };
    arr.iter().any(|entry| {
        entry
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.contains("tauri-connector-detect"))
    })
}

// ----- rendering -----------------------------------------------------------

fn print_text(sections: &[Section]) {
    println!("tauri-connector doctor v{CURRENT_VERSION}");
    println!("{}", "=".repeat(29 + CURRENT_VERSION.len()));
    println!();

    for section in sections {
        println!("{}", section.name);
        for c in &section.checks {
            println!("  {} {}", c.status.symbol(), c.label);
            if let Some(detail) = &c.detail {
                for line in detail.lines() {
                    println!("      {line}");
                }
            }
            if let Some(fix) = &c.fix {
                for (i, line) in fix.lines().enumerate() {
                    if i == 0 {
                        println!("      Fix: {line}");
                    } else {
                        println!("           {line}");
                    }
                }
            }
        }
        println!();
    }

    let (ok, fail, warn) = tally(sections);
    let summary = format!("Summary: {ok} ok · {warn} warn · {fail} fail");
    println!("{summary}");
    if fail > 0 {
        println!("Run the `Fix` commands above and re-run `tauri-connector doctor`.");
    } else if warn > 0 {
        println!("Warnings are non-blocking; address them when convenient.");
    } else {
        println!("All checks passed — you're good to go.");
    }
}

fn print_json(sections: &[Section], pattern: SetupPattern) {
    let (ok, fail, warn) = tally(sections);
    let payload = json!({
        "cli_version": CURRENT_VERSION,
        "setup_pattern": pattern.as_str(),
        "summary": { "ok": ok, "fail": fail, "warn": warn, "passed": fail == 0 },
        "fixes": fix_suggestions(sections),
        "sections": sections.iter().map(|s| json!({
            "name": s.name,
            "checks": s.checks.iter().map(|c| json!({
                "label": c.label,
                "status": c.status.as_str(),
                "detail": c.detail,
                "fix": c.fix,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("Failed to serialize doctor JSON: {e}"),
    }
}

fn fix_suggestions(sections: &[Section]) -> Vec<Value> {
    sections
        .iter()
        .flat_map(|section| {
            section.checks.iter().filter_map(move |check| {
                check.fix.as_ref().map(|fix| {
                    json!({
                        "section": section.name,
                        "label": check.label,
                        "status": check.status.as_str(),
                        "fix": fix,
                    })
                })
            })
        })
        .collect()
}

fn tally(sections: &[Section]) -> (usize, usize, usize) {
    let mut ok = 0;
    let mut fail = 0;
    let mut warn = 0;
    for s in sections {
        for c in &s.checks {
            match c.status {
                Status::Ok => ok += 1,
                Status::Fail => fail += 1,
                Status::Warn => warn += 1,
            }
        }
    }
    (ok, fail, warn)
}

fn any_fail(sections: &[Section]) -> bool {
    sections
        .iter()
        .any(|s| s.checks.iter().any(|c| c.status == Status::Fail))
}

// ----- small helpers -------------------------------------------------------

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

// ----- tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_version_plain_string() {
        let toml = r#"
[dependencies]
serde = "1"
tauri-plugin-connector = "0.8"
reqwest = "0.12"
"#;
        assert_eq!(
            extract_cargo_version(toml, "tauri-plugin-connector"),
            Some("0.8".into())
        );
    }

    #[test]
    fn cargo_version_inline_table() {
        let toml = r#"
[dependencies]
tauri-plugin-connector = { version = "0.8.0", features = ["foo"] }
"#;
        assert_eq!(
            extract_cargo_version(toml, "tauri-plugin-connector"),
            Some("0.8.0".into())
        );
    }

    #[test]
    fn cargo_version_absent() {
        let toml = r#"
[dependencies]
serde = "1"
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), None);
    }

    #[test]
    fn cargo_version_commented_out_is_absent() {
        let toml = r#"
[dependencies]
# tauri-plugin-connector = "0.8"
serde = "1"
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), None);
    }

    #[test]
    fn cargo_version_only_inside_dependencies_tables() {
        // Appearances outside a `dependencies`-flavored section should be ignored.
        let toml = r#"
[package]
tauri-plugin-connector = "shouldnotmatch"

[features]
default = []
"#;
        assert_eq!(extract_cargo_version(toml, "tauri-plugin-connector"), None);
    }

    #[test]
    fn cargo_version_dev_dependencies() {
        let toml = r#"
[dev-dependencies]
tauri-plugin-connector = "0.8"
"#;
        assert_eq!(
            extract_cargo_version(toml, "tauri-plugin-connector"),
            Some("0.8".into())
        );
    }

    #[test]
    fn permissions_string_form() {
        let caps: Value =
            serde_json::from_str(r#"{ "permissions": ["connector:default", "fs:default"] }"#)
                .unwrap();
        assert!(permissions_contain(&caps, "connector:default"));
        assert!(!permissions_contain(&caps, "missing:perm"));
    }

    #[test]
    fn permissions_object_form() {
        let caps: Value = serde_json::from_str(
            r#"{ "permissions": [{ "identifier": "connector:default" }, "fs:default"] }"#,
        )
        .unwrap();
        assert!(permissions_contain(&caps, "connector:default"));
    }

    #[test]
    fn package_deps_found_across_sections() {
        let pkg: Value =
            serde_json::from_str(r#"{ "devDependencies": { "@zumer/snapdom": "^1.0.0" } }"#)
                .unwrap();
        assert!(package_has_dep(&pkg, "@zumer/snapdom"));
        assert!(!package_has_dep(&pkg, "missing-pkg"));
    }

    #[test]
    fn package_scripts_detect_dev_connector_feature() {
        let pkg: Value = serde_json::from_str(
            r#"{ "scripts": { "tauri:dev": "tauri dev --features dev-connector" } }"#,
        )
        .unwrap();
        assert!(package_scripts_enable_feature(&pkg, "dev-connector"));
    }

    #[test]
    fn package_scripts_accept_short_feature_flag() {
        let pkg: Value =
            serde_json::from_str(r#"{ "scripts": { "dev": "cargo tauri dev -F dev-connector" } }"#)
                .unwrap();
        assert!(package_scripts_enable_feature(&pkg, "dev-connector"));
    }

    #[test]
    fn package_scripts_reject_missing_feature_flag() {
        let pkg: Value =
            serde_json::from_str(r#"{ "scripts": { "tauri:dev": "tauri dev" } }"#).unwrap();
        assert!(!package_scripts_enable_feature(&pkg, "dev-connector"));
    }

    #[test]
    fn mcp_url_must_target_streamable_mcp() {
        let bad_path = check_mcp_url(".mcp.json", "http://127.0.0.1:9556/message");
        assert_eq!(bad_path.status, Status::Fail);
        assert!(bad_path.fix.unwrap().contains("/mcp"));

        let legacy = check_mcp_url(".mcp.json", "http://127.0.0.1:9556/sse");
        assert_eq!(legacy.status, Status::Warn);
        assert!(legacy.fix.unwrap().contains("9556/mcp"));

        let ok = check_mcp_url(".mcp.json", "http://127.0.0.1:9556/mcp");
        assert_eq!(ok.status, Status::Ok);
    }

    #[test]
    fn mcp_command_transport_gets_fix_suggestion() {
        let value: Value = serde_json::from_str(
            r#"{ "command": "tauri-connector-mcp", "args": ["--port", "9555"] }"#,
        )
        .unwrap();
        let check = check_mcp_entry(Some(&value), ".mcp.json");
        assert_eq!(check.status, Status::Warn);
        assert!(check.fix.unwrap().contains("127.0.0.1:9556/mcp"));
    }

    #[test]
    fn read_pid_file_parses_new_runtime_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "tauri-connector-doctor-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".connector.json");
        std::fs::write(
            &path,
            r#"{
              "pid": 123,
              "ws_port": 9555,
              "mcp_port": 9556,
              "bridge_port": 9300,
              "app_name": "Example",
              "app_id": "com.example",
              "log_dir": "/tmp/example-connector"
            }"#,
        )
        .unwrap();
        let info = read_pid_file(&path).unwrap();
        assert_eq!(info.pid, Some(123));
        assert_eq!(info.ws_port, Some(9555));
        assert_eq!(info.mcp_port, Some(9556));
        assert_eq!(info.bridge_port, Some(9300));
        assert_eq!(info.app_name.as_deref(), Some("Example"));
        assert_eq!(info.app_id.as_deref(), Some("com.example"));
        assert_eq!(
            info.log_dir.as_deref(),
            Some(std::path::Path::new("/tmp/example-connector"))
        );
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn settings_detects_connector_hook() {
        let v: Value = serde_json::from_str(
            r#"{ "hooks": { "UserPromptSubmit": [
                { "command": "bash .claude/hooks/tauri-connector-detect.sh" }
            ] } }"#,
        )
        .unwrap();
        assert!(settings_has_connector_hook(&v));
    }

    #[test]
    fn cargo_version_hint_is_major_minor() {
        let v = cargo_version_hint();
        // Expect "major.minor" from CARGO_PKG_VERSION.
        assert_eq!(v.matches('.').count(), 1, "got {v}");
        assert!(v.chars().next().unwrap().is_ascii_digit(), "got {v}");
    }

    #[test]
    fn indent_snippet_prefixes_two_spaces() {
        assert_eq!(indent_snippet("a\nb"), "  a\n  b");
    }

    #[test]
    fn json_fix_suggestions_collects_warn_and_fail_fixes() {
        let sections = vec![Section {
            name: "Test",
            checks: vec![
                Check::ok("ok"),
                Check::warn("warn", "do warn"),
                Check::fail("fail", "do fail"),
            ],
        }];
        let fixes = fix_suggestions(&sections);
        assert_eq!(fixes.len(), 2);
        assert_eq!(fixes[0]["status"], "warn");
        assert_eq!(fixes[0]["fix"], "do warn");
        assert_eq!(fixes[1]["status"], "fail");
        assert_eq!(fixes[1]["fix"], "do fail");
    }

    #[test]
    fn settings_ignores_unrelated_hooks() {
        let v: Value = serde_json::from_str(
            r#"{ "hooks": { "UserPromptSubmit": [
                { "command": "bash .claude/hooks/other.sh" }
            ] } }"#,
        )
        .unwrap();
        assert!(!settings_has_connector_hook(&v));
    }

    #[test]
    fn extract_cargo_dep_inline_table_optional_true() {
        let toml = r#"
[dependencies]
tauri-plugin-connector = { version = "0.9", optional = true }
"#;
        assert_eq!(
            extract_cargo_dep(toml, "tauri-plugin-connector"),
            Some(("0.9".into(), true))
        );
    }

    #[test]
    fn extract_cargo_dep_plain_string_is_not_optional() {
        let toml = r#"
[dependencies]
tauri-plugin-connector = "0.9"
"#;
        assert_eq!(
            extract_cargo_dep(toml, "tauri-plugin-connector"),
            Some(("0.9".into(), false))
        );
    }

    #[test]
    fn extract_cargo_dep_inline_table_without_optional_flag() {
        let toml = r#"
[dependencies]
tauri-plugin-connector = { version = "0.9", features = ["foo"] }
"#;
        assert_eq!(
            extract_cargo_dep(toml, "tauri-plugin-connector"),
            Some(("0.9".into(), false))
        );
    }

    #[test]
    fn extract_cargo_dep_target_specific_table() {
        // The WordBrain layout puts the optional dep under a target-cfg table.
        let toml = r#"
[target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies]
tauri-plugin-connector = { version = "0.9", optional = true }
"#;
        assert_eq!(
            extract_cargo_dep(toml, "tauri-plugin-connector"),
            Some(("0.9".into(), true))
        );
    }

    #[test]
    fn extract_cargo_dep_absent() {
        let toml = r#"
[dependencies]
serde = "1"
"#;
        assert_eq!(extract_cargo_dep(toml, "tauri-plugin-connector"), None);
    }

    #[test]
    fn extract_bool_field_true_and_false() {
        assert_eq!(
            extract_bool_field("{ optional = true }", "optional"),
            Some(true)
        );
        assert_eq!(
            extract_bool_field("{ optional = false }", "optional"),
            Some(false)
        );
        assert_eq!(extract_bool_field("{ version = \"1\" }", "optional"), None);
    }

    #[test]
    fn features_declares_dep_form() {
        let toml = r#"
[features]
default = []
dev-connector = ["dep:tauri-plugin-connector"]
"#;
        assert!(features_declares_dep(
            toml,
            "dev-connector",
            "tauri-plugin-connector"
        ));
    }

    #[test]
    fn features_declares_bare_form() {
        let toml = r#"
[features]
dev-connector = ["tauri-plugin-connector"]
"#;
        assert!(features_declares_dep(
            toml,
            "dev-connector",
            "tauri-plugin-connector"
        ));
    }

    #[test]
    fn features_declaring_dep_returns_custom_feature_names() {
        let toml = r#"
[features]
default = []
connector = ["tauri-plugin-connector"]
dev-connector = ["dep:tauri-plugin-connector"]
unrelated = ["serde"]
"#;
        assert_eq!(
            features_declaring_dep(toml, "tauri-plugin-connector"),
            vec!["connector".to_string(), "dev-connector".to_string()]
        );
    }

    #[test]
    fn features_missing_returns_false() {
        let toml = r#"
[dependencies]
serde = "1"
"#;
        assert!(!features_declares_dep(
            toml,
            "dev-connector",
            "tauri-plugin-connector"
        ));
    }

    #[test]
    fn features_different_dep_returns_false() {
        let toml = r#"
[features]
some-feat = ["dep:other-crate"]
"#;
        assert!(!features_declares_dep(
            toml,
            "dev-connector",
            "tauri-plugin-connector"
        ));
    }

    #[test]
    fn features_only_inside_features_section() {
        // A `dev-connector` line in a different section must not match.
        let toml = r#"
[dependencies]
dev-connector = ["dep:tauri-plugin-connector"]
"#;
        assert!(!features_declares_dep(
            toml,
            "dev-connector",
            "tauri-plugin-connector"
        ));
    }

    #[test]
    fn features_section_tolerates_trailing_comment() {
        // `[features] # generated` should still match the [features] header.
        let toml = r#"
[features] # auto-generated by tooling
default = []
dev-connector = ["dep:tauri-plugin-connector"]
"#;
        assert!(features_declares_dep(
            toml,
            "dev-connector",
            "tauri-plugin-connector"
        ));
    }

    #[test]
    fn features_does_not_match_subtable_header() {
        // `[features.foo]` defines `features.foo`, not `[features]`, so any
        // entries inside it must not be picked up as members of `[features]`.
        let toml = r#"
[features.foo]
dev-connector = ["dep:tauri-plugin-connector"]
"#;
        assert!(!features_declares_dep(
            toml,
            "dev-connector",
            "tauri-plugin-connector"
        ));
    }

    #[test]
    fn source_feature_gates_extracts_custom_feature_names() {
        let src = r#"
#[cfg(feature="connector")]
builder = builder.plugin(tauri_plugin_connector::init());
#[cfg(feature = "dev-connector")]
const DEV_CONNECTOR: &str = "";
"#;
        assert_eq!(
            source_feature_gates(src),
            vec!["connector".to_string(), "dev-connector".to_string()]
        );
        assert!(source_has_feature_gate_for(src, "connector"));
    }

    #[test]
    fn build_features_accepts_array_and_string_forms() {
        let array: Value =
            serde_json::from_str(r#"{ "build": { "features": ["connector"] } }"#).unwrap();
        assert_eq!(build_features(&array), vec!["connector".to_string()]);

        let string: Value =
            serde_json::from_str(r#"{ "build": { "features": "connector" } }"#).unwrap();
        assert_eq!(build_features(&string), vec!["connector".to_string()]);
    }

    // ----- classify_setup_from_inputs ---------------------------------------

    const FEATURE_GATED_CARGO: &str = r#"
[dependencies]
[target.'cfg(not(any(target_os = "android", target_os = "ios")))'.dependencies]
tauri-plugin-connector = { version = "0.9", optional = true }

[features]
default = []
dev-connector = ["dep:tauri-plugin-connector"]
"#;

    const FEATURE_GATED_SRC: &str = r#"
#[cfg(feature = "dev-connector")]
const DEV_CONNECTOR_CAPABILITY: &str =
    include_str!("../capabilities-dev/dev-connector.json");

pub fn run() {
    let mut builder = tauri::Builder::default();
    #[cfg(feature = "dev-connector")]
    {
        builder = builder.plugin(tauri_plugin_connector::init());
    }
}
"#;

    const LEGACY_CARGO: &str = r#"
[dependencies]
tauri-plugin-connector = "0.9"
"#;

    const LEGACY_SRC: &str = r#"
pub fn run() {
    let mut builder = tauri::Builder::default();
    #[cfg(debug_assertions)]
    {
        builder = builder.plugin(tauri_plugin_connector::init());
    }
}
"#;

    #[test]
    fn classify_feature_gated_layout() {
        assert_eq!(
            classify_setup_from_inputs(FEATURE_GATED_CARGO, FEATURE_GATED_SRC),
            SetupPattern::FeatureGated
        );
    }

    #[test]
    fn classify_feature_gated_layout_with_custom_feature_name() {
        let cargo = r#"
[dependencies]
tauri-plugin-connector = { version = "0.11", optional = true }

[features]
connector = ["tauri-plugin-connector"]
"#;
        let src = r#"
pub fn run() {
    let mut builder = tauri::Builder::default();
    #[cfg(feature = "connector")]
    {
        builder = builder.plugin(tauri_plugin_connector::init());
    }
}
"#;
        assert_eq!(
            classify_setup_from_inputs(cargo, src),
            SetupPattern::FeatureGated
        );
    }

    #[test]
    fn classify_legacy_layout() {
        assert_eq!(
            classify_setup_from_inputs(LEGACY_CARGO, LEGACY_SRC),
            SetupPattern::Legacy
        );
    }

    #[test]
    fn classify_mixed_capability_moved_but_dep_not_optional() {
        // src uses cfg(feature = "dev-connector"), Cargo dep is plain string.
        assert_eq!(
            classify_setup_from_inputs(LEGACY_CARGO, FEATURE_GATED_SRC),
            SetupPattern::Mixed
        );
    }

    #[test]
    fn classify_mixed_optional_dep_but_no_features_block() {
        let cargo = r#"
[dependencies]
tauri-plugin-connector = { version = "0.9", optional = true }
"#;
        assert_eq!(
            classify_setup_from_inputs(cargo, FEATURE_GATED_SRC),
            SetupPattern::Mixed
        );
    }

    #[test]
    fn classify_mixed_both_gates_present() {
        // Hybrid src with both cfg gates — explicitly Mixed.
        let src = r#"
#[cfg(debug_assertions)]
#[cfg(feature = "dev-connector")]
{
    builder = builder.plugin(tauri_plugin_connector::init());
}
"#;
        assert_eq!(
            classify_setup_from_inputs(LEGACY_CARGO, src),
            SetupPattern::Mixed
        );
    }

    #[test]
    fn classify_none_when_plugin_not_referenced() {
        let src = r#"
pub fn run() {
    let mut builder = tauri::Builder::default();
}
"#;
        assert_eq!(
            classify_setup_from_inputs(LEGACY_CARGO, src),
            SetupPattern::None
        );
    }

    #[test]
    fn classify_none_for_empty_inputs() {
        assert_eq!(classify_setup_from_inputs("", ""), SetupPattern::None);
    }

    #[test]
    fn setup_pattern_as_str_round_trip() {
        assert_eq!(SetupPattern::FeatureGated.as_str(), "feature-gated");
        assert_eq!(SetupPattern::Legacy.as_str(), "legacy");
        assert_eq!(SetupPattern::Mixed.as_str(), "mixed");
        assert_eq!(SetupPattern::None.as_str(), "none");
    }
}
