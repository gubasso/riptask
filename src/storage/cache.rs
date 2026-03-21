use crate::adapters::backend::BackendIssueRecord;
use crate::domain::backend_state::BackendState;
use crate::domain::backend_state::backend_state_key;
use crate::domain::id_map::IdMap;
use crate::paths::AppPaths;
use crate::services::backend_mapping::backend_state_entry;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;

pub fn load_backend_state(paths: &AppPaths) -> Result<BackendState> {
    load_json(paths.backend_state_path().as_str())
}

pub fn save_backend_state(paths: &AppPaths, state: &BackendState) -> Result<()> {
    save_json(paths.backend_state_path().as_str(), state)
}

pub fn seed_backend_state_entry(
    paths: &AppPaths,
    provider_name: &str,
    repo: &str,
    record: &BackendIssueRecord,
) -> Result<()> {
    let mut state = load_backend_state(paths)?;
    state.insert(
        backend_state_key(provider_name, repo, record.issue_id),
        backend_state_entry(record),
    );
    save_backend_state(paths, &state)
}

pub fn load_id_map(paths: &AppPaths) -> Result<IdMap> {
    load_json(paths.id_map_path().as_str())
}

pub fn save_id_map(paths: &AppPaths, map: &IdMap) -> Result<()> {
    save_json(paths.id_map_path().as_str(), map)
}

pub fn load_deleted_keys(paths: &AppPaths) -> Result<HashSet<String>> {
    load_json(paths.deleted_keys_path().as_str())
}

pub fn save_deleted_keys(paths: &AppPaths, keys: &HashSet<String>) -> Result<()> {
    save_json(paths.deleted_keys_path().as_str(), keys)
}

pub fn mark_deleted(paths: &AppPaths, key: &str) -> Result<()> {
    let mut keys = load_deleted_keys(paths)?;
    keys.insert(key.to_owned());
    save_deleted_keys(paths, &keys)
}

pub fn unmark_deleted(paths: &AppPaths, key: &str) -> Result<()> {
    let mut keys = load_deleted_keys(paths)?;
    keys.remove(key);
    save_deleted_keys(paths, &keys)
}

fn load_json<T>(path: &str) -> Result<T>
where
    T: for<'de> serde::Deserialize<'de> + Default,
{
    match fs::read_to_string(path) {
        Ok(content) => {
            serde_json::from_str(&content).with_context(|| format!("failed to parse {path}"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error.into()),
    }
}

fn save_json<T>(path: &str, value: &T) -> Result<()>
where
    T: serde::Serialize,
{
    let content = serde_json::to_string_pretty(value).context("failed to serialize cache file")?;
    let dest = std::path::Path::new(path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create cache dir for {path}"))?;
    }
    let dir = dest.parent().unwrap_or(std::path::Path::new("."));
    let mut file =
        tempfile::NamedTempFile::new_in(dir).context("failed to create temp cache file")?;
    use std::io::Write;
    file.write_all(format!("{content}\n").as_bytes())
        .context("failed to write temp cache file")?;
    file.persist(dest)
        .map_err(|error| anyhow::Error::from(error.error))
        .with_context(|| format!("failed to persist cache to {path}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::paths::AppPaths;
    use tempfile::tempdir;

    #[test]
    fn missing_cache_files_default_to_empty_maps() {
        let temp = tempdir().expect("temp dir");
        let paths = AppPaths {
            riptsk_repo: temp.path().join("repo").to_string_lossy().as_ref().into(),
            cache_root: temp.path().join("cache").to_string_lossy().as_ref().into(),
        };
        assert!(super::load_id_map(&paths).expect("id map").is_empty());
        assert!(
            super::load_backend_state(&paths)
                .expect("backend state")
                .is_empty()
        );
        assert!(
            super::load_deleted_keys(&paths)
                .expect("deleted keys")
                .is_empty()
        );
    }
}
