use crate::domain::work_clone::WorkCloneMarker;
use crate::error::RiptaskError;
use anyhow::Context;
use std::fs;
use std::path::{Path, PathBuf};

pub fn marker_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".git").join("tsk-work-clone.json")
}

pub fn load_marker(repo_root: &Path) -> Result<Option<WorkCloneMarker>, RiptaskError> {
    let path = marker_path(repo_root);
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content)
            .map(Some)
            .with_context(|| format!("failed to parse {}", path.display()))
            .map_err(RiptaskError::Other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn save_marker(repo_root: &Path, marker: &WorkCloneMarker) -> Result<(), RiptaskError> {
    let path = marker_path(repo_root);
    let content =
        serde_json::to_string_pretty(marker).map_err(|error| RiptaskError::Other(error.into()))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{content}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{load_marker, save_marker};
    use crate::domain::work_clone::WorkCloneMarker;
    use tempfile::tempdir;

    #[test]
    fn load_marker_returns_none_when_missing() {
        let temp = tempdir().expect("temp dir");

        let marker = load_marker(temp.path()).expect("load marker");

        assert!(marker.is_none());
    }

    #[test]
    fn marker_round_trip() {
        let temp = tempdir().expect("temp dir");
        std::fs::create_dir_all(temp.path().join(".git")).expect("git dir");
        let marker = WorkCloneMarker {
            main_repo_path: "/tmp/main".into(),
            branch: "42-test-branch".into(),
            issue_id: "42".into(),
            remote_url: "git@example.com:org/repo.git".into(),
        };

        save_marker(temp.path(), &marker).expect("save marker");
        let loaded = load_marker(temp.path())
            .expect("load marker")
            .expect("marker present");

        assert_eq!(loaded.main_repo_path, marker.main_repo_path);
        assert_eq!(loaded.branch, marker.branch);
        assert_eq!(loaded.issue_id, marker.issue_id);
        assert_eq!(loaded.remote_url, marker.remote_url);
    }
}
