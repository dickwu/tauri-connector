//! Private CLI handle-to-instance hints. The live endpoint must still authenticate and match.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Debug, Serialize, Deserialize)]
pub struct Binding {
    pub app_instance_id: String,
    pub host: String,
    pub port: u16,
}
fn root() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".cache")))
        .ok_or("private handle registry unavailable")?;
    let path = base.join("tauri-connector/instance-bindings");
    if !path.exists() {
        std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
    }
    let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("unsafe handle registry".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("handle registry permissions must be private".into());
        }
    }
    #[cfg(not(unix))]
    return Err(
        "private handle registry unavailable on this platform; supply --app-instance-id".into(),
    );
    #[cfg(unix)]
    Ok(path)
}
fn file(handle: &str) -> Result<PathBuf, String> {
    if handle.is_empty() || handle.len() > 256 {
        return Err("invalid handle".into());
    }
    let encoded = crate::commands::sha256_hex(handle.as_bytes());
    Ok(root()?.join(encoded))
}
pub fn load(handle: &str) -> Result<Option<Binding>, String> {
    let path = file(handle)?;
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
                return Err("unsafe handle binding".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err("handle binding permissions must be private".into());
                }
            }
        }
    }
    serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map(Some)
        .map_err(|e| e.to_string())
}
pub fn save(handle: &str, binding: &Binding) -> Result<(), String> {
    use std::io::Write;
    let path = file(handle)?;
    if let Some(previous) = load(handle)? {
        if previous.app_instance_id != binding.app_instance_id {
            return Err("handle already belongs to another app instance".into());
        }
        return Ok(());
    }
    if std::fs::read_dir(root()?)
        .map_err(|e| e.to_string())?
        .take(4096)
        .count()
        >= 4096
    {
        return Err(
            "private handle registry capacity reached; retain --app-instance-id for this run"
                .into(),
        );
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&path).map_err(|e| e.to_string())?;
    output
        .write_all(&serde_json::to_vec(binding).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    output.sync_all().map_err(|e| e.to_string())
}
