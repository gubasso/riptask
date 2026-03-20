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
    Ok(new_path)
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
