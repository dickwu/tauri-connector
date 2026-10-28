//! Version drift warnings for `status` and `doctor` (0.17.6): several
//! `tauri-connector` binaries on PATH at different versions (an old
//! `~/.cargo/bin` copy shadowing Homebrew), a CLI older or newer than the
//! connected app's plugin, and a Homebrew tap that is behind the plugin.

use std::path::{Path, PathBuf};

pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One `tauri-connector` executable found on PATH.
#[derive(Debug, Clone, PartialEq)]
pub struct PathBinary {
    pub path: PathBuf,
    pub version: Option<String>,
}

/// A version warning: a one-line message and the command that fixes it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VersionWarning {
    pub code: &'static str,
    pub message: String,
    pub fix: String,
}

fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split(['.', '-', '+']);
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

/// Every `tauri-connector` on PATH, in PATH order, deduplicated by real
/// path, with the version each reports for `--version`.
pub fn path_binaries() -> Vec<PathBinary> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(windows) {
            "tauri-connector.exe"
        } else {
            "tauri-connector"
        });
        if !candidate.is_file() {
            continue;
        }
        let real = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if seen.contains(&real) {
            continue;
        }
        seen.push(real.clone());
        let version = if std::env::current_exe()
            .ok()
            .and_then(|exe| std::fs::canonicalize(exe).ok())
            .is_some_and(|exe| exe == real)
        {
            Some(CLI_VERSION.to_string())
        } else {
            binary_version(&candidate)
        };
        out.push(PathBinary {
            path: candidate,
            version,
        });
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// `tauri-connector X.Y.Z` from `<bin> --version`.
fn binary_version(bin: &Path) -> Option<String> {
    let output = std::process::Command::new(bin)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .find(|word| parse(word).is_some())
        .map(|word| word.trim_start_matches('v').to_string())
}

/// Version of the formula in a local `dickwu/homebrew-tap` checkout, when
/// Homebrew is installed (no network, no `brew` subprocess).
pub fn local_tap_version() -> Option<String> {
    let mut repos = Vec::new();
    if let Some(repo) = std::env::var_os("HOMEBREW_REPOSITORY") {
        repos.push(PathBuf::from(repo));
    }
    repos.push(PathBuf::from("/opt/homebrew"));
    repos.push(PathBuf::from("/usr/local/Homebrew"));
    repos.push(PathBuf::from("/home/linuxbrew/.linuxbrew/Homebrew"));
    repos.into_iter().find_map(|repo| {
        let formula = repo.join("Library/Taps/dickwu/homebrew-tap/Formula/tauri-connector.rb");
        formula_version(&std::fs::read_to_string(formula).ok()?)
    })
}

/// `version "X.Y.Z"`, else the `/vX.Y.Z/` in the formula's download URL.
fn formula_version(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("version ") {
            let v = rest.trim().trim_matches('"');
            if parse(v).is_some() {
                return Some(v.to_string());
            }
        }
    }
    text.split(['/', '"'])
        .find(|part| part.starts_with('v') && parse(part).is_some())
        .map(|part| part.trim_start_matches('v').to_string())
}

fn is_homebrew_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/Cellar/") || text.contains("/homebrew/") || text.contains("/linuxbrew/")
}

/// `(Some(a), Some(b))` only when `keep(a, b)`; otherwise `(None, None)`.
trait FilterPair<T> {
    fn filter_pair(self, keep: impl Fn(&T, &T) -> bool) -> (Option<T>, Option<T>);
}

impl<T> FilterPair<T> for (Option<T>, Option<T>) {
    fn filter_pair(self, keep: impl Fn(&T, &T) -> bool) -> (Option<T>, Option<T>) {
        match self {
            (Some(a), Some(b)) if keep(&a, &b) => (Some(a), Some(b)),
            _ => (None, None),
        }
    }
}

/// All warnings for this CLI, the binaries on PATH, the connected plugin
/// (when known) and the local tap version (when Homebrew is installed).
pub fn warnings(
    binaries: &[PathBinary],
    plugin_version: Option<&str>,
    tap_version: Option<&str>,
) -> Vec<VersionWarning> {
    let mut out = Vec::new();
    let cli = parse(CLI_VERSION);
    let versions: Vec<&str> = binaries
        .iter()
        .filter_map(|b| b.version.as_deref())
        .collect();
    let distinct = versions
        .iter()
        .filter_map(|v| parse(v))
        .collect::<std::collections::BTreeSet<_>>();
    if distinct.len() > 1 {
        let list = binaries
            .iter()
            .map(|b| {
                format!(
                    "{} ({})",
                    b.path.display(),
                    b.version.as_deref().unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let stale = binaries
            .iter()
            .filter(|b| b.version.as_deref().and_then(parse) < distinct.iter().next_back().copied())
            .map(|b| b.path.display().to_string())
            .collect::<Vec<_>>();
        out.push(VersionWarning {
            code: "multiple_cli_versions",
            message: format!(
                "{} tauri-connector binaries on PATH at different versions; the first one wins: {list}",
                binaries.len()
            ),
            fix: format!(
                "remove or upgrade the older copies ({}), e.g. `cargo uninstall connector-cli` for ~/.cargo/bin, then `hash -r`",
                stale.join(", ")
            ),
        });
    }
    if let (Some(plugin), Some(cli)) =
        (plugin_version.and_then(parse), cli).filter_pair(|p, c| p != c)
    {
        let plugin_text = plugin_version.unwrap_or_default();
        let fix = if plugin > cli {
            let brew = binaries.first().is_some_and(|b| is_homebrew_path(&b.path))
                || std::env::current_exe().is_ok_and(|exe| is_homebrew_path(&exe));
            if brew {
                "upgrade the CLI: `brew update && brew upgrade tauri-connector` (`brew update` refreshes the tap first)".to_string()
            } else {
                "upgrade the CLI: `brew update && brew upgrade tauri-connector`, or `cargo install connector-cli --locked --force`".to_string()
            }
        } else {
            format!(
                "update the app's plugin: `cargo update -p tauri-plugin-connector --precise {CLI_VERSION}` in src-tauri, then restart the app"
            )
        };
        out.push(VersionWarning {
            code: "cli_plugin_version_mismatch",
            message: format!(
                "CLI {CLI_VERSION} is {} the connected app's plugin {plugin_text}",
                if plugin > cli {
                    "older than"
                } else {
                    "newer than"
                }
            ),
            fix,
        });
    }
    let newest_known = [plugin_version.and_then(parse), cli]
        .into_iter()
        .flatten()
        .max();
    if let (Some(_), Some(newest)) =
        (tap_version.and_then(parse), newest_known).filter_pair(|t, n| t < n)
    {
        out.push(VersionWarning {
            code: "homebrew_tap_stale",
            message: format!(
                "the local dickwu/homebrew-tap formula is at {}, behind {}.{}.{}",
                tap_version.unwrap_or_default(),
                newest.0,
                newest.1,
                newest.2
            ),
            fix: "run `brew update` before `brew upgrade tauri-connector`".into(),
        });
    }
    out
}

/// The warnings for this machine and an optional connected plugin.
pub fn current_warnings(plugin_version: Option<&str>) -> Vec<VersionWarning> {
    warnings(
        &path_binaries(),
        plugin_version,
        local_tap_version().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(path: &str, version: &str) -> PathBinary {
        PathBinary {
            path: PathBuf::from(path),
            version: Some(version.into()),
        }
    }

    #[test]
    fn several_cli_versions_on_path_are_named_with_the_stale_copy() {
        let found = warnings(
            &[
                bin("/Users/me/.cargo/bin/tauri-connector", "0.16.0"),
                bin("/opt/homebrew/bin/tauri-connector", CLI_VERSION),
            ],
            None,
            None,
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].code, "multiple_cli_versions");
        assert!(found[0]
            .message
            .contains(".cargo/bin/tauri-connector (0.16.0)"));
        assert!(found[0]
            .fix
            .contains("/Users/me/.cargo/bin/tauri-connector"));
        assert!(!found[0].fix.contains("/opt/homebrew/bin"));
        // The same version twice (a symlink farm) is not drift.
        let same = [
            bin("/a/tauri-connector", CLI_VERSION),
            bin("/b/tauri-connector", CLI_VERSION),
        ];
        assert!(warnings(&same, None, None).is_empty());
    }

    #[test]
    fn a_plugin_version_mismatch_names_the_side_to_upgrade() {
        let older_cli = warnings(
            &[bin("/opt/homebrew/bin/tauri-connector", CLI_VERSION)],
            Some("99.0.0"),
            None,
        );
        assert_eq!(older_cli[0].code, "cli_plugin_version_mismatch");
        assert!(older_cli[0].message.contains("older than"));
        assert!(older_cli[0].fix.contains("brew update && brew upgrade"));
        let older_plugin = warnings(&[], Some("0.1.0"), None);
        assert!(older_plugin[0].message.contains("newer than"));
        assert!(older_plugin[0]
            .fix
            .contains("cargo update -p tauri-plugin-connector"));
        assert!(warnings(&[], Some(CLI_VERSION), None).is_empty());
        assert!(warnings(&[], None, None).is_empty());
    }

    #[test]
    fn a_stale_tap_hints_brew_update() {
        let stale = warnings(&[], None, Some("0.1.0"));
        assert_eq!(stale[0].code, "homebrew_tap_stale");
        assert!(stale[0].fix.contains("brew update"));
        assert!(warnings(&[], None, Some(CLI_VERSION)).is_empty());
        assert_eq!(
            formula_version("class TauriConnector < Formula\n  version \"0.17.5\"\n"),
            Some("0.17.5".into())
        );
        assert_eq!(
            formula_version("  url \"https://github.com/dickwu/tauri-connector/releases/download/v0.17.4/x.tar.gz\""),
            Some("0.17.4".into())
        );
    }
}
