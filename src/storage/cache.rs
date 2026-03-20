use crate::adapters::remote::RemoteIssueRecord;
use crate::domain::id_map::IdMap;
use crate::domain::remote_state::RemoteState;
use crate::domain::remote_state::remote_state_key;
use crate::paths::AppPaths;
use crate::services::remote_mapping::remote_state_entry;
use anyhow::{Context, Result};
use std::fs;

pub fn load_remote_state(paths: &AppPaths) -> Result<RemoteState> {
    load_json(paths.remote_state_path().as_str())
}

pub fn save_remote_state(paths: &AppPaths, state: &RemoteState) -> Result<()> {
    save_json(paths.remote_state_path().as_str(), state)
}

pub fn seed_remote_state_entry(
    paths: &AppPaths,
    provider_name: &str,
    repo: &str,
    record: &RemoteIssueRecord,
) -> Result<()> {
    let mut state = load_remote_state(paths)?;
    state.insert(
        remote_state_key(provider_name, repo, record.issue_id),
        remote_state_entry(record),
    );
    save_remote_state(paths, &state)
}

pub fn load_id_map(paths: &AppPaths) -> Result<IdMap> {
    load_json(paths.id_map_path().as_str())
}

pub fn save_id_map(paths: &AppPaths, map: &IdMap) -> Result<()> {
    save_json(paths.id_map_path().as_str(), map)
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
            super::load_remote_state(&paths)
                .expect("remote state")
                .is_empty()
        );
    }
}
