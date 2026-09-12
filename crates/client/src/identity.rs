//! Endpoint identity verification; local files are discovery hints, never authority.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppIdentity {
    pub app_instance_id: String,
    pub app_id: String,
    pub pid: u32,
    pub started_at: u64,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub workspace_path: Option<PathBuf>,
    pub inspection_protocol_version: u64,
    #[serde(default)]
    pub workflow_protocol_version: Option<u64>,
}

impl AppIdentity {
    pub fn parse(value: Value) -> Result<Self, String> {
        let identity: Self = serde_json::from_value(value)
            .map_err(|e| format!("identity_unavailable: invalid endpoint identity: {e}"))?;
        if identity.app_instance_id.is_empty() || identity.app_id.is_empty() || identity.pid == 0 {
            return Err("identity_unavailable: incomplete endpoint identity".into());
        }
        Ok(identity)
    }
}

/// Canonical filesystem containment uses path segments, never textual prefixes.
pub fn workspace_matches(cwd: &Path, workspace: &Path) -> bool {
    match (cwd.canonicalize(), workspace.canonicalize()) {
        (Ok(cwd), Ok(workspace)) => cwd.starts_with(workspace),
        _ => false,
    }
}

pub fn instance_from_handle(handle: &str) -> Option<&str> {
    handle
        .split_once(":picker:")
        .or_else(|| handle.split_once(":capture:"))
        .or_else(|| handle.split_once(":artifact:"))
        .map(|(instance, _)| instance)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_workspace_boundaries_and_symlinks() {
        let root =
            std::env::temp_dir().join(format!("connector-identity-{}", uuid::Uuid::new_v4()));
        let app = root.join("app");
        let apple = root.join("apple");
        std::fs::create_dir_all(app.join("child")).unwrap();
        std::fs::create_dir_all(&apple).unwrap();
        assert!(workspace_matches(&app.join("child"), &app));
        assert!(!workspace_matches(&apple, &app));
        assert!(!workspace_matches(&app, &root.join("missing")));
        #[cfg(unix)]
        {
            let alias = root.join("alias");
            std::os::unix::fs::symlink(&app, &alias).unwrap();
            assert!(workspace_matches(&alias.join("child"), &app));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
