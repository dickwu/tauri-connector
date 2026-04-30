//! Self-update via GitHub releases.

use serde::Deserialize;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES_URL: &str = "https://api.github.com/repos/dickwu/tauri-connector/releases/latest";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Map current platform to the release asset name suffix.
fn platform_target() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        (os, arch) => Err(format!("Unsupported platform: {os}/{arch}")),
    }
}

/// Parse a version string like "0.4.0" into (major, minor, patch).
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.strip_prefix('v').unwrap_or(v);
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}

/// Returns true if `latest` is newer than `current`.
fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// Run the update check and optionally install.
pub async fn run(check_only: bool) -> Result<(), String> {
    let target = platform_target()?;

    eprintln!("Current version: {CURRENT_VERSION}");
    eprintln!("Checking for updates...");

    let client = reqwest::Client::builder()
        .user_agent("tauri-connector-cli")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let release: Release = client
        .get(RELEASES_URL)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch releases: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse release: {e}"))?;

    let latest = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    eprintln!("Latest version:  {latest}");

    if !is_newer(CURRENT_VERSION, latest) {
        eprintln!("Already up to date.");
        return Ok(());
    }

    eprintln!("New version available: {CURRENT_VERSION} -> {latest}");

    if check_only {
        return Ok(());
    }

    // Find the right asset for this platform
    let asset_name = format!("tauri-connector-{target}");
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| format!("No release asset found for {target}"))?;

    eprintln!("Downloading {}...", asset.name);

    let bytes = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Failed to read download: {e}"))?;

    let size_kb = bytes.len() / 1024;
    eprintln!("Downloaded {size_kb}KB");

    // Replace the current binary
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Cannot locate current binary: {e}"))?;

    // On Unix: rename current to .old, write new, set executable, remove .old
    // On Windows: rename current to .old.exe, write new
    let backup = current_exe.with_extension("old");
    if backup.exists() {
        std::fs::remove_file(&backup).map_err(|e| format!("Failed to remove old backup: {e}"))?;
    }

    std::fs::rename(&current_exe, &backup)
        .map_err(|e| format!("Failed to backup current binary: {e}"))?;

    std::fs::write(&current_exe, &bytes).map_err(|e| {
        // Restore backup on failure
        let _ = std::fs::rename(&backup, &current_exe);
        format!("Failed to write new binary: {e}")
    })?;

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&current_exe, perms)
            .map_err(|e| format!("Failed to set permissions: {e}"))?;
    }

    // Clean up backup
    let _ = std::fs::remove_file(&backup);

    eprintln!("Updated to v{latest}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!(is_newer("0.3.1", "0.4.0"));
        assert!(is_newer("0.4.0", "0.4.1"));
        assert!(is_newer("0.4.0", "1.0.0"));
        assert!(!is_newer("0.4.0", "0.4.0"));
        assert!(!is_newer("0.4.1", "0.4.0"));
        assert!(!is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn test_parse_version() {
        assert_eq!(parse_version("0.4.0"), Some((0, 4, 0)));
        assert_eq!(parse_version("v0.4.0"), Some((0, 4, 0)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("bad"), None);
    }

    #[test]
    fn test_platform_target() {
        // Just verify it doesn't panic on the current platform
        let result = platform_target();
        assert!(result.is_ok());
    }
}
