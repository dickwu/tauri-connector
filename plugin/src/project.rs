//! Which project an app instance belongs to (0.17.5+).
//!
//! Several apps share the default 9555/9556 ports, so the identity response,
//! the PID file and the MCP `initialize` metadata name the project. The CLI
//! and MCP server compare it with the working directory's project and with
//! `--project`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

/// Name set with [`crate::ConnectorBuilder::project_name`].
static EXPLICIT_NAME: OnceLock<String> = OnceLock::new();
static PROJECT: OnceLock<Project> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: String,
    /// Directory holding the Tauri project (the parent of `src-tauri/`).
    pub root: Option<PathBuf>,
    /// `[package] name` of the app crate.
    pub cargo_package: Option<String>,
    /// Where `name` came from: `builder`, `productName`, `cargoPackage`,
    /// `directory` or `identifier`.
    pub source: &'static str,
}

impl Project {
    pub fn to_json(&self) -> Value {
        json!({
            "projectName": self.name,
            "projectRoot": self.root.as_ref().map(|path| path.to_string_lossy().into_owned()),
            "cargoPackage": self.cargo_package,
            "projectNameSource": self.source,
        })
    }
}

pub(crate) fn set_explicit_name(name: &str) {
    let name = name.trim();
    if !name.is_empty() {
        let _ = EXPLICIT_NAME.set(name.to_string());
    }
}

/// The project for this process, computed once.
pub fn current(app: &tauri::AppHandle) -> &'static Project {
    PROJECT.get_or_init(|| {
        let config = app.config();
        let manifest = manifest_dir(
            std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from),
            std::env::current_exe().ok().as_deref(),
        );
        detect(
            EXPLICIT_NAME.get().map(String::as_str),
            config.product_name.as_deref(),
            &config.identifier,
            manifest.as_deref(),
        )
    })
}

pub(crate) fn detect(
    explicit: Option<&str>,
    product_name: Option<&str>,
    identifier: &str,
    manifest_dir: Option<&Path>,
) -> Project {
    let cargo_package = manifest_dir.and_then(|dir| package_name(&dir.join("Cargo.toml")));
    let root = manifest_dir.map(|dir| {
        let root = if dir.file_name().is_some_and(|name| name == "src-tauri") {
            dir.parent().unwrap_or(dir)
        } else {
            dir
        };
        root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
    });
    let nonempty = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let directory = root
        .as_ref()
        .and_then(|root| root.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let (name, source) = if let Some(name) = nonempty(explicit) {
        (name, "builder")
    } else if let Some(name) = nonempty(product_name) {
        (name, "productName")
    } else if let Some(name) = cargo_package.clone() {
        (name, "cargoPackage")
    } else if let Some(name) = directory {
        (name, "directory")
    } else {
        (identifier.to_string(), "identifier")
    };
    Project {
        name,
        root,
        cargo_package,
        source,
    }
}

/// The app crate's directory: `CARGO_MANIFEST_DIR` when the app runs under
/// `cargo run`/`tauri dev`, otherwise found from the `target/` directory the
/// executable was built into (`<root>/target` or `<root>/src-tauri/target`).
pub(crate) fn manifest_dir(env_manifest: Option<PathBuf>, exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = env_manifest.filter(|dir| dir.join("Cargo.toml").is_file()) {
        return Some(dir);
    }
    let target = exe?
        .ancestors()
        .skip(1)
        .find(|dir| dir.file_name().is_some_and(|name| name == "target"))?;
    let parent = target.parent()?;
    if package_name(&parent.join("Cargo.toml")).is_some() {
        return Some(parent.to_path_buf());
    }
    let src_tauri = parent.join("src-tauri");
    if src_tauri.join("Cargo.toml").is_file() {
        return Some(src_tauri);
    }
    parent
        .join("Cargo.toml")
        .is_file()
        .then(|| parent.to_path_buf())
}

/// `name` from the `[package]` table of a Cargo manifest.
pub(crate) fn package_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some(rest) = line.strip_prefix("name") else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tc-project-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn project_name_prefers_builder_then_product_name_then_package() {
        let root = temp("names");
        let src_tauri = root.join("src-tauri");
        std::fs::create_dir_all(&src_tauri).unwrap();
        std::fs::write(
            src_tauri.join("Cargo.toml"),
            "[package]\nname = \"r2-app\"\nversion = \"1.0.0\"\n\n[dependencies]\nname = \"x\"\n",
        )
        .unwrap();
        let canonical = root.canonicalize().unwrap();

        let project = detect(Some("R2 dev"), Some("R2"), "com.r2", Some(&src_tauri));
        assert_eq!(project.name, "R2 dev");
        assert_eq!(project.source, "builder");
        assert_eq!(project.root.as_deref(), Some(canonical.as_path()));
        assert_eq!(project.cargo_package.as_deref(), Some("r2-app"));

        let project = detect(None, Some("R2"), "com.r2", Some(&src_tauri));
        assert_eq!(
            (project.name.as_str(), project.source),
            ("R2", "productName")
        );
        let project = detect(Some("  "), Some(""), "com.r2", Some(&src_tauri));
        assert_eq!(
            (project.name.as_str(), project.source),
            ("r2-app", "cargoPackage")
        );
        let project = detect(None, None, "com.r2", None);
        assert_eq!(
            (project.name.as_str(), project.source),
            ("com.r2", "identifier")
        );
        assert_eq!(project.root, None);
        let json = project.to_json();
        assert_eq!(json["projectName"], "com.r2");
        assert!(json["projectRoot"].is_null());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_dir_comes_from_cargo_or_the_target_directory() {
        let root = temp("manifest");
        let src_tauri = root.join("src-tauri");
        std::fs::create_dir_all(src_tauri.join("target/debug")).unwrap();
        std::fs::write(src_tauri.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
        let exe = src_tauri.join("target/debug/app");
        assert_eq!(manifest_dir(None, Some(&exe)), Some(src_tauri.clone()));
        assert_eq!(
            manifest_dir(Some(src_tauri.clone()), None),
            Some(src_tauri.clone())
        );
        // A workspace target at the repo root still finds src-tauri.
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"src-tauri\"]\n",
        )
        .unwrap();
        let exe = root.join("target/debug/app");
        assert_eq!(manifest_dir(None, Some(&exe)), Some(src_tauri.clone()));
        // An installed binary outside any target dir has no project root.
        assert_eq!(
            manifest_dir(
                Some(root.join("missing")),
                Some(Path::new("/usr/local/bin/app"))
            ),
            None
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
