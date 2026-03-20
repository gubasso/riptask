use crate::adapters::backend::{BackendIssueRecord, BackendProvider};
use crate::config::Config;
use crate::domain::backend_state::backend_state_key;
use crate::domain::issue::{ConflictMeta, IssueDocument};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::backend_mapping::{
    backend_state_entry, backend_to_local, current_timestamp, issue_to_upsert,
    update_issue_from_backend,
};
use crate::services::issue_ids;
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

    pub async fn pull<P: BackendProvider + ?Sized>(
        &self,
        provider: &P,
        backend: &BackendConfig,
        force: bool,
    ) -> Result<PullSummary, RiptskError> {
        let repo = backend.repo.as_deref().ok_or_else(|| {
            RiptskError::Config(format!("backend {} is missing repo", backend.name))
        })?;
        let mut backend_state =
            cache::load_backend_state(self.paths).map_err(RiptskError::Other)?;
        let records = provider.list_issues(repo).await?;
        let mut summary = PullSummary::default();
        let mut seen_ids = HashSet::new();

        for record in records {
            seen_ids.insert(record.issue_id);
            let key = backend_state_key(provider_name(backend), repo, record.issue_id);
            let cached = backend_state.get(&key).cloned();
            let local_path = self.find_local_issue(backend, record.issue_id)?;

            if let Some(path) = local_path {
                let mut issue =
                    frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
                let local_changed = cached
                    .as_ref()
                    .is_some_and(|entry| issue.frontmatter.local_updated_at > entry.updated_at);
                let remote_changed = cached
                    .as_ref()
                    .is_none_or(|entry| record.updated_at > entry.updated_at);

                if !force && self.config.sync.conflict_detection && local_changed && remote_changed
                {
                    self.write_conflict(backend, &record, &mut issue, &path, cached.as_ref())?;
                    summary.conflicts.push(issue.frontmatter.id.clone());
                } else if remote_changed {
                    update_issue_from_backend(&mut issue, &record, backend);
                    issue.frontmatter.remote_deleted = false;
                    frontmatter::save_issue(path.as_std_path(), &issue)
                        .map_err(RiptskError::Other)?;
                    summary.updated.push(issue.frontmatter.id.clone());
                }
            } else {
                let document = backend_to_local(&record, backend);
                let path = self
                    .paths
                    .issues_dir()
                    .join(format!("{}.md", document.frontmatter.id));
                frontmatter::save_issue(path.as_std_path(), &document)
                    .map_err(RiptskError::Other)?;
                summary.created.push(document.frontmatter.id.clone());
            }

            backend_state.insert(key, backend_state_entry(&record));
        }

        for path in issue_store::list_issues(self.paths)? {
            let mut issue =
                frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
            let meta = match backend.backend {
                Backend::Github => issue
                    .frontmatter
                    .github
                    .as_ref()
                    .and_then(|meta| (meta.repo == repo).then_some(meta.issue_id).flatten()),
                Backend::Gitlab => issue
                    .frontmatter
                    .gitlab
                    .as_ref()
                    .and_then(|meta| (meta.repo == repo).then_some(meta.issue_id).flatten()),
                Backend::Local => None,
            };
            let Some(issue_id) = meta else {
                continue;
            };
            if seen_ids.contains(&issue_id) {
                continue;
            }
            if !issue.frontmatter.remote_deleted {
                issue.frontmatter.remote_deleted = true;
                frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
                summary.deleted.push(issue.frontmatter.id.clone());
            }
        }

        cache::save_backend_state(self.paths, &backend_state).map_err(RiptskError::Other)?;
        Ok(summary)
    }

    pub async fn push<P: BackendProvider + ?Sized>(
        &self,
        provider: &P,
        backend: &BackendConfig,
    ) -> Result<PushSummary, RiptskError> {
        let repo = backend.repo.as_deref().ok_or_else(|| {
            RiptskError::Config(format!("backend {} is missing repo", backend.name))
        })?;
        let mut backend_state =
            cache::load_backend_state(self.paths).map_err(RiptskError::Other)?;
        let mut summary = PushSummary::default();

        for path in issue_store::list_issues(self.paths)? {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
            if issue.frontmatter.project != backend.name || issue.frontmatter.remote_deleted {
                continue;
            }
            if issue.frontmatter.conflict.is_some() {
                summary.skipped.push(issue.frontmatter.id.clone());
                continue;
            }

            let upsert = issue_to_upsert(&issue);
            let issue_id = match backend.backend {
                Backend::Github => issue
                    .frontmatter
                    .github
                    .as_ref()
                    .and_then(|meta| meta.issue_id),
                Backend::Gitlab => issue
                    .frontmatter
                    .gitlab
                    .as_ref()
                    .and_then(|meta| meta.issue_id),
                Backend::Local => None,
            };
            let Some(issue_id) = issue_id else {
                summary.skipped.push(issue.frontmatter.id.clone());
                continue;
            };

            let key = backend_state_key(provider_name(backend), repo, issue_id);
            let cached = backend_state.get(&key);
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
            update_issue_from_backend(&mut updated, &record, backend);
            frontmatter::save_issue(path.as_std_path(), &updated).map_err(RiptskError::Other)?;
            backend_state.insert(key, backend_state_entry(&record));
            summary.updated.push(updated.frontmatter.id.clone());
        }

        cache::save_backend_state(self.paths, &backend_state).map_err(RiptskError::Other)?;
        Ok(summary)
    }

    pub fn status(&self) -> Result<StatusSummary, RiptskError> {
        let backend_state = cache::load_backend_state(self.paths).map_err(RiptskError::Other)?;
        let mut summary = StatusSummary::default();
        for path in issue_store::list_issues(self.paths)? {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
            if issue.frontmatter.conflict.is_some() {
                summary.conflicts.push(issue.frontmatter.id.clone());
                continue;
            }
            let backend_key = if let Some(meta) = issue.frontmatter.github.as_ref() {
                meta.issue_id
                    .map(|issue_id| backend_state_key("github", &meta.repo, issue_id))
            } else if let Some(meta) = issue.frontmatter.gitlab.as_ref() {
                meta.issue_id
                    .map(|issue_id| backend_state_key("gitlab", &meta.repo, issue_id))
            } else {
                None
            };
            let Some(backend_key) = backend_key else {
                continue;
            };
            let Some(entry) = backend_state.get(&backend_key) else {
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
        backend: &BackendConfig,
        issue_id: u64,
    ) -> Result<Option<camino::Utf8PathBuf>, RiptskError> {
        let exact_id =
            issue_ids::format_id(&issue_ids::derive_scope_from_backend(backend), issue_id);
        if let Ok(path) = issue_store::find_issue(self.paths, &exact_id) {
            return Ok(Some(path));
        }

        for path in issue_store::list_issues(self.paths)? {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
            let matches = match backend.backend {
                Backend::Github => issue.frontmatter.github.as_ref().is_some_and(|meta| {
                    meta.repo == backend.repo.clone().unwrap_or_default()
                        && meta.issue_id == Some(issue_id)
                }),
                Backend::Gitlab => issue.frontmatter.gitlab.as_ref().is_some_and(|meta| {
                    meta.repo == backend.repo.clone().unwrap_or_default()
                        && meta.issue_id == Some(issue_id)
                }),
                Backend::Local => false,
            };
            if matches {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    fn write_conflict(
        &self,
        backend: &BackendConfig,
        record: &BackendIssueRecord,
        issue: &mut IssueDocument,
        issue_path: &camino::Utf8PathBuf,
        cached: Option<&crate::domain::backend_state::BackendStateEntry>,
    ) -> Result<(), RiptskError> {
        let mut remote_doc = backend_to_local(record, backend);
        remote_doc.frontmatter.conflict_role = Some("remote".into());
        remote_doc.frontmatter.conflict_parent = Some(issue.frontmatter.id.clone());
        let remote_path = self
            .paths
            .issues_dir()
            .join(format!("{}.REMOTE.md", issue.frontmatter.id));
        frontmatter::save_issue(remote_path.as_std_path(), &remote_doc)
            .map_err(RiptskError::Other)?;
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
        frontmatter::save_issue(issue_path.as_std_path(), issue).map_err(RiptskError::Other)?;
        Ok(())
    }
}

fn provider_name(backend: &BackendConfig) -> &'static str {
    match backend.backend {
        Backend::Github => "github",
        Backend::Gitlab => "gitlab",
        Backend::Local => "local",
    }
}
