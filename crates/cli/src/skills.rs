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
        name: "debug-playbook",
        display_path: "references/debug-playbook.md",
        content: include_str!("../skill/references/debug-playbook.md"),
    },
    SkillDoc {
        name: "code-review-playbook",
        display_path: "references/code-review-playbook.md",
        content: include_str!("../skill/references/code-review-playbook.md"),
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

pub fn stale_local_skill_docs() -> Vec<(PathBuf, String)> {
    local_skill_dirs()
        .into_iter()
        .filter(|dir| dir.exists())
        .flat_map(|dir| {
            SKILL_DOCS.iter().filter_map(move |doc| {
                let path = dir.join(doc.display_path);
                match std::fs::read_to_string(&path) {
                    Ok(content) if content != doc.content => {
                        Some((path, "content differs".to_string()))
                    }
                    Ok(_) => None,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => Some((path, format!("could not read: {e}"))),
                }
            })
        })
        .collect()
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
}
