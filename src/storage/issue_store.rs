use crate::error::{RiptskError, StoreError};
use crate::paths::AppPaths;
use camino::Utf8PathBuf;
use std::fs;

pub fn issue_dir(paths: &AppPaths) -> Utf8PathBuf {
    paths.issues_dir()
}

pub fn find_issue(paths: &AppPaths, id: &str) -> Result<Utf8PathBuf, RiptskError> {
    let candidate = issue_dir(paths).join(format!("{id}.md"));
    if candidate.exists() {
        return Ok(candidate);
    }
    Err(StoreError::FileNotFound(id.to_owned()).into())
}

pub fn list_issues(paths: &AppPaths) -> Result<Vec<Utf8PathBuf>, RiptskError> {
    let mut issues = Vec::new();
    for entry in fs::read_dir(issue_dir(paths))? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .ends_with(".REMOTE.md")
            || path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .ends_with(".LOCAL.md")
        {
            continue;
        }
        issues.push(
            Utf8PathBuf::from_path_buf(path)
                .map_err(|_| RiptskError::General("issue path is not valid UTF-8".into()))?,
        );
    }
    issues.sort();
    Ok(issues)
}

pub fn list_all_issues(paths: &AppPaths) -> Result<Vec<Utf8PathBuf>, RiptskError> {
    let mut issues = Vec::new();
    for entry in fs::read_dir(issue_dir(paths))? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        issues.push(
            Utf8PathBuf::from_path_buf(path)
                .map_err(|_| RiptskError::General("issue path is not valid UTF-8".into()))?,
        );
    }
    issues.sort();
    Ok(issues)
}

pub fn rename_issue_file(
    paths: &AppPaths,
    old_id: &str,
    new_id: &str,
) -> Result<Utf8PathBuf, RiptskError> {
    let old_path = issue_dir(paths).join(format!("{old_id}.md"));
    let new_path = issue_dir(paths).join(format!("{new_id}.md"));
    if !old_path.exists() {
        return Err(StoreError::FileNotFound(old_id.to_owned()).into());
    }
    fs::rename(&old_path, &new_path)?;

    let old_remote = issue_dir(paths).join(format!("{old_id}.REMOTE.md"));
    let new_remote = issue_dir(paths).join(format!("{new_id}.REMOTE.md"));
    if old_remote.exists() {
        fs::rename(old_remote, new_remote)?;
    }
    let old_local = issue_dir(paths).join(format!("{old_id}.LOCAL.md"));
    let new_local = issue_dir(paths).join(format!("{new_id}.LOCAL.md"));
    if old_local.exists() {
        fs::rename(old_local, new_local)?;
    }
    Ok(new_path)
}

pub fn delete_issue_files(paths: &AppPaths, id: &str) -> Result<(), RiptskError> {
    let main_path = issue_dir(paths).join(format!("{id}.md"));
    if main_path.exists() {
        fs::remove_file(&main_path)?;
    }
    let remote_path = issue_dir(paths).join(format!("{id}.REMOTE.md"));
    if remote_path.exists() {
        fs::remove_file(&remote_path)?;
    }
    let local_path = issue_dir(paths).join(format!("{id}.LOCAL.md"));
    if local_path.exists() {
        fs::remove_file(&local_path)?;
    }
    Ok(())
}

pub fn local_backup_path(paths: &AppPaths, id: &str) -> Utf8PathBuf {
    issue_dir(paths).join(format!("{id}.LOCAL.md"))
}

pub fn remote_backup_path(paths: &AppPaths, id: &str) -> Utf8PathBuf {
    issue_dir(paths).join(format!("{id}.REMOTE.md"))
}

pub fn delete_conflict_backups(paths: &AppPaths, id: &str) -> Result<(), RiptskError> {
    let local_path = local_backup_path(paths, id);
    if local_path.exists() {
        fs::remove_file(local_path)?;
    }
    let remote_path = remote_backup_path(paths, id);
    if remote_path.exists() {
        fs::remove_file(remote_path)?;
    }
    Ok(())
}

pub fn rewrite_cross_references(
    paths: &AppPaths,
    old_id: &str,
    new_id: &str,
) -> Result<Vec<Utf8PathBuf>, RiptskError> {
    let mut updated = Vec::new();
    for path in list_all_issues(paths)? {
        let content = fs::read_to_string(&path)?;
        if !content.contains(old_id) {
            continue;
        }
        fs::write(&path, content.replace(old_id, new_id))?;
        updated.push(path);
    }
    Ok(updated)
}
