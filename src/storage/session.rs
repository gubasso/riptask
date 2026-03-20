use crate::domain::session::SessionState;
use crate::error::RiptskError;
use anyhow::Context;
use std::fs;
use std::path::Path;

pub fn load_session(path: &Path) -> Result<Option<SessionState>, RiptskError> {
    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str(&content)
            .map(Some)
            .with_context(|| format!("failed to parse {}", path.display()))
            .map_err(RiptskError::Other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn save_session(path: &Path, state: &SessionState) -> Result<(), RiptskError> {
    let content =
        serde_json::to_string_pretty(state).map_err(|error| RiptskError::Other(error.into()))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{content}\n"))?;
    Ok(())
}

pub fn clear_session(path: &Path) -> Result<(), RiptskError> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}
