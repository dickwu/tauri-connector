//! Project names for app instances (plugins 0.17.5+).
//!
//! Several Tauri apps share the default 9555/9556 ports. The plugin reports
//! a project name and root; these helpers compare them with `--project`
//! (`TAURI_CONNECTOR_PROJECT`) and with the working directory's project.

use std::path::{Path, PathBuf};

use crate::discovery::ConnectorInstance;
use crate::identity::AppIdentity;

/// Environment variable naming the project to connect to (`--project`).
pub const PROJECT_ENV: &str = "TAURI_CONNECTOR_PROJECT";

/// Shown when an app's plugin predates project names (before 0.17.5).
pub const UNKNOWN_PROJECT: &str = "unknown project";

/// What an app says about its project, from its identity or PID file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectFacts<'a> {
    pub name: Option<&'a str>,
    pub root: Option<&'a Path>,
    pub cargo_package: Option<&'a str>,
    pub app_name: Option<&'a str>,
    pub app_id: Option<&'a str>,
}

impl<'a> ProjectFacts<'a> {
    /// Facts from a live identity, filled in from the PID file when the
    /// identity lacks them.
    pub fn of(identity: Option<&'a AppIdentity>, instance: Option<&'a ConnectorInstance>) -> Self {
        Self {
            name: identity
                .and_then(|i| i.project_name.as_deref())
                .or_else(|| instance.and_then(|i| i.project_name.as_deref())),
            root: identity
                .and_then(|i| i.project_root.as_deref())
                .or_else(|| instance.and_then(|i| i.project_root.as_deref())),
            cargo_package: identity
                .and_then(|i| i.cargo_package.as_deref())
                .or_else(|| instance.and_then(|i| i.cargo_package.as_deref())),
            app_name: identity
                .and_then(|i| i.app_name.as_deref())
                .or_else(|| instance.and_then(|i| i.app_name.as_deref())),
            app_id: identity
                .map(|i| i.app_id.as_str())
                .or_else(|| instance.and_then(|i| i.app_id.as_deref())),
        }
    }

    /// The project name, or `unknown project` for plugins before 0.17.5.
    pub fn label(&self) -> String {
        self.name.unwrap_or(UNKNOWN_PROJECT).to_string()
    }

    /// Whether `selector` names this project: its project name, Cargo
    /// package, app name, app id or root directory name (case-insensitive),
    /// or a path to its root.
    pub fn matches(&self, selector: &str) -> bool {
        let selector = selector.trim();
        if selector.is_empty() {
            return false;
        }
        let root_name = self
            .root
            .and_then(Path::file_name)
            .and_then(|name| name.to_str());
        let named = [
            self.name,
            self.cargo_package,
            self.app_name,
            self.app_id,
            root_name,
        ]
        .into_iter()
        .flatten()
        .any(|value| value.eq_ignore_ascii_case(selector));
        named
            || self.root.is_some_and(|root| {
                let path = Path::new(selector);
                path.is_absolute() && same_dir(path, root)
            })
    }
}

/// The Tauri project the working directory belongs to: the nearest ancestor
/// holding `src-tauri/`, or holding `tauri.conf.json` (a `src-tauri`
/// directory counts as its parent).
pub fn cwd_project_root(cwd: &Path) -> Option<PathBuf> {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    for dir in cwd.ancestors().take(12) {
        if dir.join("src-tauri").is_dir() {
            return Some(dir.to_path_buf());
        }
        if dir.join("tauri.conf.json").is_file() {
            return Some(match dir.file_name() {
                Some(name) if name == "src-tauri" => dir.parent().unwrap_or(dir).to_path_buf(),
                _ => dir.to_path_buf(),
            });
        }
    }
    None
}

/// True when both roots are known and neither contains the other: the app
/// belongs to a different project than the working directory.
pub fn is_other_project(cwd_root: Option<&Path>, app_root: Option<&Path>) -> bool {
    let (Some(cwd_root), Some(app_root)) = (cwd_root, app_root) else {
        return false;
    };
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let (cwd_root, app_root) = (canonical(cwd_root), canonical(app_root));
    !(cwd_root.starts_with(&app_root) || app_root.starts_with(&cwd_root))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tc-project-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn selectors_match_names_packages_ids_and_roots() {
        let root = temp("r2");
        let facts = ProjectFacts {
            name: Some("R2"),
            root: Some(&root),
            cargo_package: Some("r2-desktop"),
            app_name: Some("R2"),
            app_id: Some("com.example.r2"),
        };
        for selector in [
            "r2",
            "R2",
            "r2-desktop",
            "com.example.r2",
            root.to_str().unwrap(),
        ] {
            assert!(facts.matches(selector), "{selector}");
        }
        let dir_name = root.file_name().unwrap().to_str().unwrap().to_string();
        assert!(facts.matches(&dir_name));
        assert!(!facts.matches("gfit"));
        assert!(!facts.matches(""));
        assert_eq!(facts.label(), "R2");
        assert_eq!(ProjectFacts::default().label(), UNKNOWN_PROJECT);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn working_directory_project_is_found_and_compared() {
        let base = temp("cwd");
        let r2 = base.join("r2");
        let gfit = base.join("gfit");
        std::fs::create_dir_all(r2.join("src-tauri/src")).unwrap();
        std::fs::create_dir_all(gfit.join("src-tauri")).unwrap();
        std::fs::write(gfit.join("src-tauri/tauri.conf.json"), "{}").unwrap();
        assert_eq!(
            cwd_project_root(&r2.join("src-tauri/src")),
            Some(r2.clone())
        );
        assert_eq!(cwd_project_root(&r2), Some(r2.clone()));
        assert_eq!(
            cwd_project_root(&gfit.join("src-tauri")),
            Some(gfit.clone())
        );
        assert!(is_other_project(Some(&r2), Some(&gfit)));
        assert!(!is_other_project(Some(&r2), Some(&r2.join("src-tauri"))));
        assert!(
            !is_other_project(Some(&r2), None),
            "older plugins are not refused"
        );
        assert!(!is_other_project(None, Some(&gfit)));
        std::fs::remove_dir_all(base).unwrap();
    }
}
