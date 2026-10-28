//! Bundled skill documents embedded in the CLI binary.

use std::path::{Path, PathBuf};

pub struct SkillDoc {
    pub name: &'static str,
    pub display_path: &'static str,
    pub content: &'static str,
}

const SKILL_DOCS: &[SkillDoc] = &[
    SkillDoc {
        name: "tauri-connector",
        display_path: "SKILL.md",
        content: include_str!("../skill/SKILL.md"),
    },
    SkillDoc {
        name: "setup",
        display_path: "SETUP.md",
        content: include_str!("../skill/SETUP.md"),
    },
    SkillDoc {
        name: "cli-commands",
        display_path: "references/cli-commands.md",
        content: include_str!("../skill/references/cli-commands.md"),
    },
    SkillDoc {
        name: "mcp-tools",
        display_path: "references/mcp-tools.md",
        content: include_str!("../skill/references/mcp-tools.md"),
    },
    SkillDoc {
        name: "snapshot-budget",
        display_path: "references/snapshot-budget.md",
        content: include_str!("../skill/references/snapshot-budget.md"),
    },
    SkillDoc {
        name: "debug-playbook",
        display_path: "references/debug-playbook.md",
        content: include_str!("../skill/references/debug-playbook.md"),
    },
    SkillDoc {
        name: "code-review-playbook",
        display_path: "references/code-review-playbook.md",
        content: include_str!("../skill/references/code-review-playbook.md"),
    },
    SkillDoc {
        name: "upgrade-and-smoke",
        display_path: "references/upgrade-and-smoke.md",
        content: include_str!("../skill/references/upgrade-and-smoke.md"),
    },
];

pub fn get(name: &str) -> Option<&'static SkillDoc> {
    let normalized = normalize_name(name);
    SKILL_DOCS.iter().find(|doc| {
        normalize_name(doc.name) == normalized || normalize_name(doc.display_path) == normalized
    })
}

pub fn list() {
    for doc in SKILL_DOCS {
        println!("{}\t{}", doc.name, doc.display_path);
    }
}

pub fn print(name: &str) -> Result<(), String> {
    let doc = get(name).ok_or_else(|| unknown_skill(name))?;
    print!("{}", doc.content);
    Ok(())
}

pub fn materialize(name: &str) -> Result<PathBuf, String> {
    let doc = get(name).ok_or_else(|| unknown_skill(name))?;
    materialize_doc(doc)
}

fn local_skill_dirs() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home_dir() {
        for base in [
            ".codex/skills/tauri-connector",
            ".Codex/skills/tauri-connector",
            ".claude/skills/tauri-connector",
            ".agents/skills/tauri-connector",
        ] {
            paths.push(home.join(base));
        }
    }
    paths
}

/// One installed skill directory whose files differ from the bundled docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleSkillDir {
    pub dir: PathBuf,
    /// Bundled files that differ from (or cannot be read in) this copy.
    pub files: Vec<String>,
    /// `version:` from the copy's SKILL.md front matter, if any.
    pub version: Option<String>,
}

impl StaleSkillDir {
    /// True when the copy declares a newer version than this CLI bundles,
    /// so the CLI, not the copy, is out of date.
    pub fn newer_than_cli(&self) -> bool {
        self.version
            .as_deref()
            .and_then(parse_version)
            .zip(parse_version(env!("CARGO_PKG_VERSION")))
            .is_some_and(|(copy, cli)| copy > cli)
    }

    pub fn describe(&self) -> String {
        let version = self.version.as_deref().unwrap_or("unknown version");
        let relation = if self.newer_than_cli() {
            "newer than this CLI"
        } else {
            "differs from this CLI"
        };
        format!(
            "{} (SKILL.md {version}, {relation} {}; {} file(s): {})",
            self.dir.display(),
            env!("CARGO_PKG_VERSION"),
            self.files.len(),
            self.files.join(", ")
        )
    }
}

pub fn stale_local_skill_docs() -> Vec<StaleSkillDir> {
    stale_skill_dirs(local_skill_dirs())
}

fn stale_skill_dirs(dirs: Vec<PathBuf>) -> Vec<StaleSkillDir> {
    // ~/.codex and ~/.Codex are one directory on a case-insensitive file
    // system, and a symlinked copy is the same files: report each once.
    let mut seen = Vec::new();
    let mut stale = Vec::new();
    for dir in dirs {
        let Ok(canonical) = dir.canonicalize() else {
            continue;
        };
        if seen.contains(&canonical) {
            continue;
        }
        seen.push(canonical);
        let files: Vec<String> = SKILL_DOCS
            .iter()
            .filter(
                |doc| match std::fs::read_to_string(dir.join(doc.display_path)) {
                    Ok(content) => content != doc.content,
                    Err(e) => e.kind() != std::io::ErrorKind::NotFound,
                },
            )
            .map(|doc| doc.display_path.to_string())
            .collect();
        if files.is_empty() {
            continue;
        }
        let version = std::fs::read_to_string(dir.join("SKILL.md"))
            .ok()
            .and_then(|text| front_matter_version(&text));
        stale.push(StaleSkillDir {
            dir,
            files,
            version,
        });
    }
    stale
}

fn front_matter_version(text: &str) -> Option<String> {
    let body = text.strip_prefix("---")?;
    let front = &body[..body.find("\n---")?];
    front.lines().find_map(|line| {
        line.trim()
            .strip_prefix("version:")
            .map(|v| v.trim().trim_matches('"').to_string())
    })
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.split(['-', '+']).next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

fn materialize_doc(doc: &SkillDoc) -> Result<PathBuf, String> {
    let path = materialized_root().join(doc.display_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let write = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != doc.content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => return Err(format!("failed to read {}: {e}", path.display())),
    };
    if write {
        std::fs::write(&path, doc.content)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    }
    Ok(path)
}

fn materialized_root() -> PathBuf {
    if let Ok(cache_home) = std::env::var("XDG_CACHE_HOME") {
        return Path::new(&cache_home)
            .join("tauri-connector")
            .join("skills")
            .join(env!("CARGO_PKG_VERSION"));
    }
    if let Some(home) = home_dir() {
        return home
            .join(".cache")
            .join("tauri-connector")
            .join("skills")
            .join(env!("CARGO_PKG_VERSION"));
    }
    std::env::temp_dir()
        .join("tauri-connector")
        .join("skills")
        .join(env!("CARGO_PKG_VERSION"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn normalize_name(name: &str) -> String {
    name.trim()
        .trim_start_matches("./")
        .trim_start_matches("skill/")
        .trim_start_matches("references/")
        .trim_end_matches(".md")
        .trim_end_matches("/SKILL")
        .trim_end_matches("/SETUP")
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn unknown_skill(name: &str) -> String {
    let names = SKILL_DOCS
        .iter()
        .map(|doc| doc.name)
        .collect::<Vec<_>>()
        .join(", ");
    format!("unknown skill doc '{name}'. Available: {names}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aliases() {
        assert_eq!(get("tauri-connector").unwrap().display_path, "SKILL.md");
        assert_eq!(get("SKILL.md").unwrap().name, "tauri-connector");
        assert_eq!(get("references/mcp-tools.md").unwrap().name, "mcp-tools");
    }

    #[test]
    fn embedded_mcp_reference_mentions_agent_oriented_schema() {
        let mcp = get("mcp-tools").unwrap().content;
        for needle in [
            "webview_locator",
            "annotate",
            "loadState",
            "`fn`",
            "`state`",
        ] {
            assert!(mcp.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn embedded_docs_match_workspace_skill_docs_when_available() {
        let workspace_skill = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skill");
        if !workspace_skill.exists() {
            return;
        }

        for doc in SKILL_DOCS {
            let path = workspace_skill.join(doc.display_path);
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
            assert_eq!(
                content, doc.content,
                "stale embedded doc {}",
                doc.display_path
            );
        }
    }

    #[test]
    fn stale_skill_dirs_are_reported_once_per_directory_with_their_version() {
        let root = std::env::temp_dir().join(format!("tc-skill-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let old = root.join("old");
        let current = root.join("current");
        let newer = root.join("newer");
        for dir in [&old, &current, &newer] {
            for doc in SKILL_DOCS {
                let path = dir.join(doc.display_path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, doc.content).unwrap();
            }
        }
        let cli = env!("CARGO_PKG_VERSION");
        let skill = get("SKILL.md").unwrap().content;
        std::fs::write(
            old.join("SKILL.md"),
            skill.replace(&format!("version: {cli}"), "version: 0.1.0"),
        )
        .unwrap();
        std::fs::write(old.join("references/mcp-tools.md"), "old").unwrap();
        std::fs::write(
            newer.join("SKILL.md"),
            skill.replace(&format!("version: {cli}"), "version: 999.0.0"),
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&old, root.join("alias")).unwrap();

        let mut dirs = vec![old.clone(), current, newer.clone(), root.join("missing")];
        #[cfg(unix)]
        dirs.push(root.join("alias"));
        let stale = stale_skill_dirs(dirs);
        assert_eq!(stale.len(), 2, "{stale:?}");
        assert_eq!(stale[0].dir, old);
        assert_eq!(stale[0].version.as_deref(), Some("0.1.0"));
        assert_eq!(stale[0].files, vec!["SKILL.md", "references/mcp-tools.md"]);
        assert!(!stale[0].newer_than_cli());
        assert_eq!(stale[1].dir, newer);
        assert!(stale[1].newer_than_cli());
        assert!(stale[1].describe().contains("newer than this CLI"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
