//! Build-embedded allowlist. Hashes identify content; they are not authentication.
use serde::Serialize;

pub const RUNTIME_VERSION: &str = "1";
pub const SEMANTIC_VERSION: &str = "1";
pub const PROTOCOL_VERSION: u32 = 1;
pub const BOOTSTRAP: &str = include_str!("bootstrap.js");
pub const INSTALLER: &str = include_str!("installer.js");
pub const SEMANTIC: &str = include_str!("../semantic/core.js");
pub const WORKFLOW: &str = include_str!("../workflow/page.js");
pub const PICKER: &str = include_str!("../picker/page.js");
pub const GEOMETRY: &str = include_str!("../screenshot/geometry.js");
pub const CAPTURE: &str = include_str!("../capture/page.js");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleManifest {
    pub module_id: &'static str,
    pub version: &'static str,
    pub content_hash: String,
    pub dependencies: Vec<&'static str>,
    pub install_source: &'static str,
    pub scope: &'static str,
    pub cleanup: &'static str,
}

/// FNV-1a is a reproducible content consistency marker, never a trust proof.
pub fn content_hash(source: &[u8]) -> String {
    let hash = source.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    format!("fnv1a64:{hash:016x}")
}

pub fn modules() -> Vec<ModuleManifest> {
    [
        ("semantic", SEMANTIC, vec![]),
        ("workflow", WORKFLOW, vec!["semantic"]),
        ("picker", PICKER, vec!["semantic"]),
        ("ipcCapture", CAPTURE, vec![]),
        ("geometry", GEOMETRY, vec!["semantic"]),
    ]
    .into_iter()
    .map(|(module_id, source, dependencies)| ModuleManifest {
        module_id,
        version: "1",
        content_hash: content_hash(source.as_bytes()),
        dependencies,
        install_source: "build_embedded",
        scope: "document",
        cleanup: "dispose_before_replacement_or_pagehide",
    })
    .collect()
}

pub fn bundle_hash() -> String {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| {
        content_hash(
            [
                BOOTSTRAP, INSTALLER, SEMANTIC, WORKFLOW, PICKER, CAPTURE, GEOMETRY,
            ]
            .join("\n")
            .as_bytes(),
        )
    })
    .clone()
}

pub fn validate(modules: &[ModuleManifest]) -> Result<(), &'static str> {
    let mut installed = std::collections::HashSet::new();
    for module in modules {
        if module.install_source != "build_embedded"
            || !module
                .dependencies
                .iter()
                .all(|dependency| installed.contains(dependency))
            || !installed.insert(module.module_id)
        {
            return Err("invalid_module_graph");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_manifest_is_topologically_sorted() {
        assert!(validate(&modules()).is_ok());
    }
    #[test]
    fn cycles_duplicates_and_remote_sources_are_rejected() {
        let mut cycle = modules();
        cycle[0].dependencies.push("workflow");
        assert!(validate(&cycle).is_err());
        let mut duplicate = modules();
        duplicate.push(duplicate[0].clone());
        assert!(validate(&duplicate).is_err());
        let mut remote = modules();
        remote[0].install_source = "https://untrusted.invalid/script";
        assert!(validate(&remote).is_err());
    }
}
