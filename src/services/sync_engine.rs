use crate::adapters::remote::RemoteProvider;
use crate::config::{Config, RemoteConfig, RemoteType};
use crate::domain::issue::{ConflictMeta, IssueDocument};
use crate::domain::remote_state::remote_state_key;
use crate::error::TskError;
use crate::paths::AppPaths;
use crate::services::issue_ids;
use crate::services::remote_mapping::{
    current_timestamp, issue_to_upsert, remote_state_entry, remote_to_local,
    update_issue_from_remote,
};
use crate::storage::{cache, frontmatter, issue_store};
use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct PullSummary {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub conflicts: Vec<String>,
    pub deleted: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PushSummary {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StatusSummary {
    pub pushes: Vec<String>,
    pub conflicts: Vec<String>,
}

pub struct SyncEngine<'a> {
    pub paths: &'a AppPaths,
    pub config: &'a Config,
}

impl<'a> SyncEngine<'a> {
    pub fn new(paths: &'a AppPaths, config: &'a Config) -> Self {
        Self { paths, config }
    }

    pub async fn pull<P: RemoteProvider + ?Sized>(
        &self,
        provider: &P,
        remote: &RemoteConfig,
        force: bool,
    ) -> Result<PullSummary, TskError> {
        let repo = remote
            .repo
            .as_deref()
            .ok_or_else(|| TskError::Config(format!("remote {} is missing repo", remote.name)))?;
        let mut remote_state = cache::load_remote_state(self.paths).map_err(TskError::Other)?;
        let records = provider.list_issues(repo).await?;
        let mut summary = PullSummary::default();
        let mut seen_ids = HashSet::new();

        for record in records {
            seen_ids.insert(record.issue_id);
            let key = remote_state_key(provider_name(remote), repo, record.issue_id);
            let cached = remote_state.get(&key).cloned();
            let local_path = self.find_local_issue(remote, record.issue_id)?;

            if let Some(path) = local_path {
                let mut issue =
                    frontmatter::load_issue(path.as_std_path()).map_err(TskError::Other)?;
                let local_changed = cached
                    .as_ref()
                    .is_some_and(|entry| issue.frontmatter.local_updated_at > entry.updated_at);
                let remote_changed = cached
                    .as_ref()
                    .is_none_or(|entry| record.updated_at > entry.updated_at);

                if !force && self.config.sync.conflict_detection && local_changed && remote_changed
                {
                    self.write_conflict(remote, &record, &mut issue, &path, cached.as_ref())?;
                    summary.conflicts.push(issue.frontmatter.id.clone());
                } else if remote_changed {
                    update_issue_from_remote(&mut issue, &record, remote);
                    issue.frontmatter.remote_deleted = false;
                    frontmatter::save_issue(path.as_std_path(), &issue).map_err(TskError::Other)?;
                    summary.updated.push(issue.frontmatter.id.clone());
                }
            } else {
                let document = remote_to_local(&record, remote);
                let path = self
                    .paths
                    .issues_dir()
                    .join(format!("{}.md", document.frontmatter.id));
                frontmatter::save_issue(path.as_std_path(), &document).map_err(TskError::Other)?;
                summary.created.push(document.frontmatter.id.clone());
            }

            remote_state.insert(key, remote_state_entry(&record));
        }

        for path in issue_store::list_issues(self.paths)? {
            let mut issue = frontmatter::load_issue(path.as_std_path()).map_err(TskError::Other)?;
            let meta = match remote.remote_type {
                RemoteType::Github => issue
                    .frontmatter
                    .github
                    .as_ref()
                    .and_then(|meta| (meta.repo == repo).then_some(meta.issue_id).flatten()),
                RemoteType::Gitlab => issue
                    .frontmatter
                    .gitlab
                    .as_ref()
                    .and_then(|meta| (meta.repo == repo).then_some(meta.issue_id).flatten()),
                RemoteType::Local => None,
            };
            let Some(issue_id) = meta else {
                continue;
            };
            if seen_ids.contains(&issue_id) {
                continue;
            }
            if !issue.frontmatter.remote_deleted {
                issue.frontmatter.remote_deleted = true;
                frontmatter::save_issue(path.as_std_path(), &issue).map_err(TskError::Other)?;
                summary.deleted.push(issue.frontmatter.id.clone());
            }
        }

        cache::save_remote_state(self.paths, &remote_state).map_err(TskError::Other)?;
        Ok(summary)
    }

    pub async fn push<P: RemoteProvider + ?Sized>(
        &self,
        provider: &P,
        remote: &RemoteConfig,
    ) -> Result<PushSummary, TskError> {
        let repo = remote
            .repo
            .as_deref()
            .ok_or_else(|| TskError::Config(format!("remote {} is missing repo", remote.name)))?;
        let mut remote_state = cache::load_remote_state(self.paths).map_err(TskError::Other)?;
        let mut summary = PushSummary::default();

        for path in issue_store::list_issues(self.paths)? {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(TskError::Other)?;
            if issue.frontmatter.project != remote.name || issue.frontmatter.remote_deleted {
                continue;
            }
            if issue.frontmatter.conflict.is_some() {
                summary.skipped.push(issue.frontmatter.id.clone());
                continue;
            }

            let upsert = issue_to_upsert(&issue);
            let issue_id = match remote.remote_type {
                RemoteType::Github => issue
                    .frontmatter
                    .github
                    .as_ref()
                    .and_then(|meta| meta.issue_id),
                RemoteType::Gitlab => issue
                    .frontmatter
                    .gitlab
                    .as_ref()
                    .and_then(|meta| meta.issue_id),
                RemoteType::Local => None,
            };
            let Some(issue_id) = issue_id else {
                summary.skipped.push(issue.frontmatter.id.clone());
                continue;
            };

            let key = remote_state_key(provider_name(remote), repo, issue_id);
            let cached = remote_state.get(&key);
            if cached
                .as_ref()
                .is_some_and(|entry| issue.frontmatter.local_updated_at <= entry.updated_at)
            {
                summary.skipped.push(issue.frontmatter.id.clone());
                continue;
            }

            let mut record = provider.update_issue(repo, issue_id, &upsert).await?;
            provider.sync_labels(repo, issue_id, &upsert.labels).await?;
            if issue.frontmatter.state == crate::domain::issue::IssueState::Done {
                provider.close_issue(repo, issue_id).await?;
                record.state = "closed".into();
            } else {
                provider.reopen_issue(repo, issue_id).await?;
                record.state = "open".into();
            }
            // Refresh timestamp to account for label/state mutations after update_issue
            record.updated_at = current_timestamp();

            let mut updated = issue;
            update_issue_from_remote(&mut updated, &record, remote);
            frontmatter::save_issue(path.as_std_path(), &updated).map_err(TskError::Other)?;
            remote_state.insert(key, remote_state_entry(&record));
            summary.updated.push(updated.frontmatter.id.clone());
        }

        cache::save_remote_state(self.paths, &remote_state).map_err(TskError::Other)?;
        Ok(summary)
    }

    pub fn status(&self) -> Result<StatusSummary, TskError> {
        let remote_state = cache::load_remote_state(self.paths).map_err(TskError::Other)?;
        let mut summary = StatusSummary::default();
        for path in issue_store::list_issues(self.paths)? {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(TskError::Other)?;
            if issue.frontmatter.conflict.is_some() {
                summary.conflicts.push(issue.frontmatter.id.clone());
                continue;
            }
            let remote_key = if let Some(meta) = issue.frontmatter.github.as_ref() {
                meta.issue_id
                    .map(|issue_id| remote_state_key("github", &meta.repo, issue_id))
            } else if let Some(meta) = issue.frontmatter.gitlab.as_ref() {
                meta.issue_id
                    .map(|issue_id| remote_state_key("gitlab", &meta.repo, issue_id))
            } else {
                None
            };
            let Some(remote_key) = remote_key else {
                continue;
            };
            let Some(entry) = remote_state.get(&remote_key) else {
                summary.pushes.push(issue.frontmatter.id.clone());
                continue;
            };
            if issue.frontmatter.local_updated_at > entry.updated_at {
                summary.pushes.push(issue.frontmatter.id.clone());
            }
        }
        Ok(summary)
    }

    fn find_local_issue(
        &self,
        remote: &RemoteConfig,
        issue_id: u64,
    ) -> Result<Option<camino::Utf8PathBuf>, TskError> {
        let exact_id = issue_ids::format_id(&issue_ids::derive_scope_from_remote(remote), issue_id);
        if let Ok(path) = issue_store::find_issue(self.paths, &exact_id) {
            return Ok(Some(path));
        }

        for path in issue_store::list_issues(self.paths)? {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(TskError::Other)?;
            let matches = match remote.remote_type {
                RemoteType::Github => issue.frontmatter.github.as_ref().is_some_and(|meta| {
                    meta.repo == remote.repo.clone().unwrap_or_default()
                        && meta.issue_id == Some(issue_id)
                }),
                RemoteType::Gitlab => issue.frontmatter.gitlab.as_ref().is_some_and(|meta| {
                    meta.repo == remote.repo.clone().unwrap_or_default()
                        && meta.issue_id == Some(issue_id)
                }),
                RemoteType::Local => false,
            };
            if matches {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    fn write_conflict(
        &self,
        remote: &RemoteConfig,
        record: &crate::adapters::remote::RemoteIssueRecord,
        issue: &mut IssueDocument,
        issue_path: &camino::Utf8PathBuf,
        cached: Option<&crate::domain::remote_state::RemoteStateEntry>,
    ) -> Result<(), TskError> {
        let mut remote_doc = remote_to_local(record, remote);
        remote_doc.frontmatter.conflict_role = Some("remote".into());
        remote_doc.frontmatter.conflict_parent = Some(issue.frontmatter.id.clone());
        let remote_path = self
            .paths
            .issues_dir()
            .join(format!("{}.REMOTE.md", issue.frontmatter.id));
        frontmatter::save_issue(remote_path.as_std_path(), &remote_doc).map_err(TskError::Other)?;
        issue.frontmatter.conflict = Some(ConflictMeta {
            detected_at: current_timestamp(),
            remote_file: remote_path.file_name().unwrap_or_default().to_string(),
            remote_updated_at: record.updated_at.clone(),
            local_updated_at: issue.frontmatter.local_updated_at.clone(),
            last_synced_at: cached
                .map(|entry| entry.updated_at.clone())
                .unwrap_or_default(),
        });
        issue.frontmatter.conflict_role = None;
        issue.frontmatter.conflict_parent = None;
        frontmatter::save_issue(issue_path.as_std_path(), issue).map_err(TskError::Other)?;
        Ok(())
    }
}

fn provider_name(remote: &RemoteConfig) -> &'static str {
    match remote.remote_type {
        RemoteType::Github => "github",
        RemoteType::Gitlab => "gitlab",
        RemoteType::Local => "local",
    }
}
