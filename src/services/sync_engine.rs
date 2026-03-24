use crate::adapters::backend::{BackendIssueRecord, BackendProvider, DeleteOutcome};
use crate::config::Config;
use crate::domain::backend_state::{BackendStateEntry, backend_state_key};
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
    pub deleted: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StatusSummary {
    pub pushes: Vec<String>,
    pub creates: Vec<String>,
    pub deletes: Vec<String>,
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
        let deleted_keys = cache::load_deleted_keys(self.paths).map_err(RiptskError::Other)?;
        let records = provider.list_issues(repo).await?;
        let mut summary = PullSummary::default();
        let mut seen_ids = HashSet::new();

        for record in records {
            let key = backend_state_key(provider_name(backend), repo, record.issue_id);
            seen_ids.insert(record.issue_id);
            if deleted_keys.contains(&key) {
                continue;
            }
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
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
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
            issue_store::delete_issue_files(self.paths, &issue.frontmatter.id)?;
            backend_state.remove(&backend_state_key(provider_name(backend), repo, issue_id));
            summary.deleted.push(issue.frontmatter.id.clone());
        }

        cache::save_backend_state(self.paths, &backend_state).map_err(RiptskError::Other)?;
        Ok(summary)
    }

    pub async fn push<P: BackendProvider + ?Sized>(
        &self,
        provider: &P,
        backend: &BackendConfig,
    ) -> Result<PushSummary, RiptskError> {
        let issue_paths = issue_store::list_issues(self.paths)?;
        self.push_inner(provider, backend, &issue_paths, true).await
    }

    pub async fn push_issues<P: BackendProvider + ?Sized>(
        &self,
        provider: &P,
        backend: &BackendConfig,
        issue_paths: &[camino::Utf8PathBuf],
    ) -> Result<PushSummary, RiptskError> {
        self.push_inner(provider, backend, issue_paths, false).await
    }

    pub fn status(&self) -> Result<StatusSummary, RiptskError> {
        let backend_state = cache::load_backend_state(self.paths).map_err(RiptskError::Other)?;
        let deleted_keys = cache::load_deleted_keys(self.paths).map_err(RiptskError::Other)?;
        let mut summary = StatusSummary::default();

        for path in issue_store::list_issues(self.paths)? {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
            if issue.frontmatter.conflict.is_some() {
                summary.conflicts.push(issue.frontmatter.id.clone());
                continue;
            }

            let Some(project_backend) = self.backend_for_project(&issue.frontmatter.project) else {
                continue;
            };

            let backend_key = issue_backend_key(&issue, project_backend);
            let Some(backend_key) = backend_key else {
                summary.creates.push(issue.frontmatter.id.clone());
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

        let mut deleted = deleted_keys.into_iter().collect::<Vec<_>>();
        deleted.sort();
        summary.deletes = deleted;
        Ok(summary)
    }

    async fn push_inner<P: BackendProvider + ?Sized>(
        &self,
        provider: &P,
        backend: &BackendConfig,
        issue_paths: &[camino::Utf8PathBuf],
        include_deletes: bool,
    ) -> Result<PushSummary, RiptskError> {
        let repo = backend.repo.as_deref().ok_or_else(|| {
            RiptskError::Config(format!("backend {} is missing repo", backend.name))
        })?;
        let mut backend_state =
            cache::load_backend_state(self.paths).map_err(RiptskError::Other)?;
        let mut deleted_keys = cache::load_deleted_keys(self.paths).map_err(RiptskError::Other)?;
        let mut summary = PushSummary::default();

        for path in issue_paths {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
            if issue.frontmatter.project != backend.name || issue.frontmatter.remote_deleted {
                continue;
            }
            if issue.frontmatter.conflict.is_some() {
                summary.skipped.push(issue.frontmatter.id.clone());
                continue;
            }

            let upsert = issue_to_upsert(&issue);
            let issue_id = issue_backend_issue_id(&issue, backend);
            if let Some(issue_id) = issue_id {
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
                sync_lock_state(provider, repo, issue_id, &issue, &record).await?;
                if let Some(state) = upsert.state.clone() {
                    record.state = state;
                }
                record.state_reason = upsert.state_reason.clone();
                record.discussion_locked = issue.frontmatter.discussion_locked;
                record.locked = issue.frontmatter.locked;
                record.lock_reason = issue.frontmatter.lock_reason.clone();
                record.updated_at = current_timestamp();

                let mut updated = issue;
                update_issue_from_backend(&mut updated, &record, backend);
                frontmatter::save_issue(path.as_std_path(), &updated)
                    .map_err(RiptskError::Other)?;
                backend_state.insert(key, backend_state_entry(&record));
                summary.updated.push(updated.frontmatter.id.clone());
                continue;
            }

            let mut record = provider.create_issue(repo, &upsert).await?;
            provider
                .sync_labels(repo, record.issue_id, &upsert.labels)
                .await?;
            if issue.frontmatter.status == crate::domain::issue::IssueState::Done {
                provider.close_issue(repo, record.issue_id).await?;
                record.state = "closed".into();
            } else {
                provider.reopen_issue(repo, record.issue_id).await?;
                record.state = "open".into();
            }
            sync_lock_state(provider, repo, record.issue_id, &issue, &record).await?;
            record.state_reason = upsert.state_reason.clone();
            record.discussion_locked = issue.frontmatter.discussion_locked;
            record.locked = issue.frontmatter.locked;
            record.lock_reason = issue.frontmatter.lock_reason.clone();
            record.updated_at = current_timestamp();

            let mut created = issue;
            update_issue_from_backend(&mut created, &record, backend);
            frontmatter::save_issue(path.as_std_path(), &created).map_err(RiptskError::Other)?;
            backend_state.insert(
                backend_state_key(provider_name(backend), repo, record.issue_id),
                backend_state_entry(&record),
            );
            summary.created.push(created.frontmatter.id.clone());
        }

        if include_deletes {
            let mut pending = deleted_keys.iter().cloned().collect::<Vec<_>>();
            pending.sort();
            for key in pending {
                let Some((key_provider, key_repo, issue_id)) = parse_backend_state_key(&key) else {
                    continue;
                };
                if key_provider != provider_name(backend) || key_repo != repo {
                    continue;
                }
                let outcome = provider.delete_issue(repo, issue_id).await?;
                if outcome == DeleteOutcome::HardDeleted {
                    deleted_keys.remove(&key);
                }
                backend_state.remove(&key);
                summary.deleted.push(key);
            }
        }

        cache::save_backend_state(self.paths, &backend_state).map_err(RiptskError::Other)?;
        cache::save_deleted_keys(self.paths, &deleted_keys).map_err(RiptskError::Other)?;
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

    fn backend_for_project(&self, project: &str) -> Option<&BackendConfig> {
        self.config.backends.iter().find(|backend| {
            backend.name == project && matches!(backend.backend, Backend::Github | Backend::Gitlab)
        })
    }

    fn write_conflict(
        &self,
        backend: &BackendConfig,
        record: &BackendIssueRecord,
        issue: &mut IssueDocument,
        issue_path: &camino::Utf8PathBuf,
        cached: Option<&BackendStateEntry>,
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

async fn sync_lock_state<P: BackendProvider + ?Sized>(
    provider: &P,
    repo: &str,
    issue_id: u64,
    issue: &IssueDocument,
    record: &BackendIssueRecord,
) -> Result<(), RiptskError> {
    let desired_locked = issue
        .frontmatter
        .locked
        .or(issue.frontmatter.discussion_locked)
        .unwrap_or(false);
    let current_locked = record.locked.or(record.discussion_locked).unwrap_or(false);
    if desired_locked == current_locked {
        return Ok(());
    }
    if desired_locked {
        provider
            .lock_issue(repo, issue_id, issue.frontmatter.lock_reason.as_deref())
            .await
    } else {
        provider.unlock_issue(repo, issue_id).await
    }
}

fn provider_name(backend: &BackendConfig) -> &'static str {
    match backend.backend {
        Backend::Github => "github",
        Backend::Gitlab => "gitlab",
        Backend::Local => "local",
    }
}

fn issue_backend_issue_id(issue: &IssueDocument, backend: &BackendConfig) -> Option<u64> {
    match backend.backend {
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
    }
}

fn issue_backend_key(issue: &IssueDocument, backend: &BackendConfig) -> Option<String> {
    let repo = backend.repo.as_deref()?;
    let issue_id = issue_backend_issue_id(issue, backend)?;
    Some(backend_state_key(provider_name(backend), repo, issue_id))
}

fn parse_backend_state_key(key: &str) -> Option<(&str, &str, u64)> {
    let (provider, rest) = key.split_once(':')?;
    let (repo, issue_id) = rest.rsplit_once(':')?;
    Some((provider, repo, issue_id.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::backend::{
        BackendIssueUpsert, BackendPrRecord, DeleteOutcome, MergeMethod, PrChecksStatus,
    };
    use crate::config::default_config;
    use crate::domain::issue::{
        ConflictMeta, GithubIssueMeta, IssueFrontmatter, IssueState, Priority,
    };
    use async_trait::async_trait;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[derive(Debug, Default)]
    struct FakeState {
        listed: Vec<BackendIssueRecord>,
        create_responses: VecDeque<BackendIssueRecord>,
        update_responses: HashMap<u64, BackendIssueRecord>,
        created: Vec<(String, BackendIssueUpsert)>,
        updated: Vec<(String, u64, BackendIssueUpsert)>,
        deleted: Vec<(String, u64)>,
        closed: Vec<(String, u64)>,
        reopened: Vec<(String, u64)>,
        locked: Vec<(String, u64, Option<String>)>,
        unlocked: Vec<(String, u64)>,
        synced_labels: Vec<(String, u64, Vec<String>)>,
        delete_outcome: DeleteOutcome,
    }

    #[derive(Clone, Default)]
    struct FakeBackendProvider {
        state: Arc<Mutex<FakeState>>,
    }

    impl FakeBackendProvider {
        fn with_listed(records: Vec<BackendIssueRecord>) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeState {
                    listed: records,
                    delete_outcome: DeleteOutcome::HardDeleted,
                    ..FakeState::default()
                })),
            }
        }

        fn with_create_response(self, record: BackendIssueRecord) -> Self {
            self.state
                .lock()
                .expect("lock")
                .create_responses
                .push_back(record);
            self
        }

        fn with_update_response(self, issue_id: u64, record: BackendIssueRecord) -> Self {
            self.state
                .lock()
                .expect("lock")
                .update_responses
                .insert(issue_id, record);
            self
        }

        fn with_delete_outcome(self, outcome: DeleteOutcome) -> Self {
            self.state.lock().expect("lock").delete_outcome = outcome;
            self
        }
    }

    #[async_trait]
    impl BackendProvider for FakeBackendProvider {
        async fn list_issues(&self, _repo: &str) -> Result<Vec<BackendIssueRecord>, RiptskError> {
            Ok(self.state.lock().expect("lock").listed.clone())
        }

        async fn create_issue(
            &self,
            repo: &str,
            issue: &BackendIssueUpsert,
        ) -> Result<BackendIssueRecord, RiptskError> {
            let mut state = self.state.lock().expect("lock");
            state.created.push((repo.to_owned(), issue.clone()));
            state
                .create_responses
                .pop_front()
                .ok_or_else(|| RiptskError::General("missing create response".into()))
        }

        async fn update_issue(
            &self,
            repo: &str,
            issue_id: u64,
            issue: &BackendIssueUpsert,
        ) -> Result<BackendIssueRecord, RiptskError> {
            let mut state = self.state.lock().expect("lock");
            state
                .updated
                .push((repo.to_owned(), issue_id, issue.clone()));
            state
                .update_responses
                .get(&issue_id)
                .cloned()
                .ok_or_else(|| {
                    RiptskError::General(format!("missing update response for {issue_id}"))
                })
        }

        async fn close_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
            self.state
                .lock()
                .expect("lock")
                .closed
                .push((repo.to_owned(), issue_id));
            Ok(())
        }

        async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
            self.state
                .lock()
                .expect("lock")
                .reopened
                .push((repo.to_owned(), issue_id));
            Ok(())
        }

        async fn delete_issue(
            &self,
            repo: &str,
            issue_id: u64,
        ) -> Result<DeleteOutcome, RiptskError> {
            let mut state = self.state.lock().expect("lock");
            state.deleted.push((repo.to_owned(), issue_id));
            Ok(state.delete_outcome)
        }

        async fn lock_issue(
            &self,
            repo: &str,
            issue_id: u64,
            reason: Option<&str>,
        ) -> Result<(), RiptskError> {
            self.state.lock().expect("lock").locked.push((
                repo.to_owned(),
                issue_id,
                reason.map(ToOwned::to_owned),
            ));
            Ok(())
        }

        async fn unlock_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
            self.state
                .lock()
                .expect("lock")
                .unlocked
                .push((repo.to_owned(), issue_id));
            Ok(())
        }

        async fn sync_labels(
            &self,
            repo: &str,
            issue_id: u64,
            labels: &[String],
        ) -> Result<(), RiptskError> {
            self.state.lock().expect("lock").synced_labels.push((
                repo.to_owned(),
                issue_id,
                labels.to_vec(),
            ));
            Ok(())
        }

        async fn create_pr(
            &self,
            _repo: &str,
            _head: &str,
            _base: &str,
            _title: &str,
            _body: &str,
        ) -> Result<BackendPrRecord, RiptskError> {
            Err(RiptskError::General("unused in test".into()))
        }

        async fn get_pr(&self, _repo: &str, _number: u64) -> Result<BackendPrRecord, RiptskError> {
            Err(RiptskError::General("unused in test".into()))
        }

        async fn update_pr(
            &self,
            _repo: &str,
            _number: u64,
            _title: &str,
            _body: &str,
        ) -> Result<BackendPrRecord, RiptskError> {
            Err(RiptskError::General("unused in test".into()))
        }

        async fn find_pr_by_branch(
            &self,
            _repo: &str,
            _head: &str,
            _base: &str,
        ) -> Result<Option<BackendPrRecord>, RiptskError> {
            Err(RiptskError::General("unused in test".into()))
        }

        async fn merge_pr(
            &self,
            _repo: &str,
            _number: u64,
            _method: MergeMethod,
            _commit_title: Option<&str>,
            _commit_message: Option<&str>,
        ) -> Result<(), RiptskError> {
            Ok(())
        }

        async fn enable_auto_merge(
            &self,
            _repo: &str,
            _number: u64,
            _method: MergeMethod,
        ) -> Result<(), RiptskError> {
            Ok(())
        }

        async fn get_pr_checks_status(
            &self,
            _repo: &str,
            _number: u64,
        ) -> Result<PrChecksStatus, RiptskError> {
            Ok(PrChecksStatus::None)
        }

        async fn create_branch(
            &self,
            _repo: &str,
            _branch_name: &str,
            _base_ref: &str,
            _issue_id: u64,
        ) -> Result<(), RiptskError> {
            Err(RiptskError::General("unused in test".into()))
        }

        async fn default_branch(&self, _repo: &str) -> Result<String, RiptskError> {
            Err(RiptskError::General("unused in test".into()))
        }

        async fn delete_branch(&self, _repo: &str, _branch_name: &str) -> Result<(), RiptskError> {
            Err(RiptskError::General("unused in test".into()))
        }
    }

    #[test]
    fn pull_creates_local_issues_from_remote() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let provider = FakeBackendProvider::with_listed(vec![record(
            42,
            "Remote issue",
            "open",
            "2026-03-20T10:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false))
            .expect("pull");

        assert_eq!(summary.created, vec!["GH-OWN-REP--42"]);
        let saved = load_issue(&paths, "GH-OWN-REP--42");
        assert_eq!(
            saved
                .frontmatter
                .github
                .as_ref()
                .and_then(|meta| meta.issue_id),
            Some(42)
        );
    }

    #[test]
    fn pull_updates_local_issues_when_remote_is_newer() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Old title", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.local_updated_at = "2026-03-20T10:00:00Z".into();
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        let provider = FakeBackendProvider::with_listed(vec![record(
            42,
            "New title",
            "open",
            "2026-03-20T12:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false))
            .expect("pull");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        assert_eq!(
            load_issue(&paths, "GH-OWN-REP--42").frontmatter.title,
            "New title"
        );
    }

    #[test]
    fn pull_detects_conflicts_when_both_sides_changed() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Old title", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.local_updated_at = "2026-03-20T11:00:00Z".into();
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        let provider = FakeBackendProvider::with_listed(vec![record(
            42,
            "Remote title",
            "open",
            "2026-03-20T12:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false))
            .expect("pull");

        assert_eq!(summary.conflicts, vec!["GH-OWN-REP--42"]);
        assert!(paths.issues_dir().join("GH-OWN-REP--42.REMOTE.md").exists());
    }

    #[test]
    fn pull_deletes_local_files_when_remote_issue_disappears() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let document = backend_to_local(&original, &backend);
        save_issue(&paths, &document);
        save_remote_issue(&paths, "GH-OWN-REP--42");
        seed_backend_state(&paths, &backend, &original);

        let provider = FakeBackendProvider::with_listed(Vec::new());
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false))
            .expect("pull");

        assert_eq!(summary.deleted, vec!["GH-OWN-REP--42"]);
        assert!(!paths.issues_dir().join("GH-OWN-REP--42.md").exists());
        assert!(!paths.issues_dir().join("GH-OWN-REP--42.REMOTE.md").exists());
        assert!(cache::load_backend_state(&paths).expect("state").is_empty());
    }

    #[test]
    fn pull_skips_issues_marked_deleted() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let key = backend_state_key("github", "owner/repo", 42);
        cache::mark_deleted(&paths, &key).expect("mark deleted");
        let provider = FakeBackendProvider::with_listed(vec![record(
            42,
            "Remote issue",
            "open",
            "2026-03-20T10:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false))
            .expect("pull");

        assert!(summary.created.is_empty());
        assert!(!paths.issues_dir().join("GH-OWN-REP--42.md").exists());
    }

    #[test]
    fn push_updates_remote_when_local_is_newer() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.local_updated_at = "2026-03-20T12:00:00Z".into();
        document.frontmatter.title = "Local title".into();
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        let provider = FakeBackendProvider::default().with_update_response(
            42,
            record(42, "Local title", "open", "2026-03-20T12:30:00Z"),
        );
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.push(&provider, &backend))
            .expect("push");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        assert_eq!(provider.state.lock().expect("lock").updated.len(), 1);
    }

    #[test]
    fn push_creates_remote_issue_for_local_only_issue() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let document = local_issue("LO-LOC--1", &backend.name, "Local only");
        save_issue(&paths, &document);

        let provider = FakeBackendProvider::default().with_create_response(record(
            9,
            "Local only",
            "open",
            "2026-03-20T13:00:00Z",
        ));
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.push(&provider, &backend))
            .expect("push");

        assert_eq!(summary.created, vec!["LO-LOC--1"]);
        let saved = load_issue(&paths, "LO-LOC--1");
        assert_eq!(
            saved
                .frontmatter
                .github
                .as_ref()
                .and_then(|meta| meta.issue_id),
            Some(9)
        );
        assert_eq!(provider.state.lock().expect("lock").created.len(), 1);
    }

    #[test]
    fn push_processes_pending_deletes() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let key = backend_state_key("github", "owner/repo", 42);
        cache::mark_deleted(&paths, &key).expect("mark deleted");
        let mut state = HashMap::new();
        state.insert(
            key.clone(),
            backend_state_entry(&record(42, "Remote issue", "open", "2026-03-20T10:00:00Z")),
        );
        cache::save_backend_state(&paths, &state).expect("save state");

        let provider = FakeBackendProvider::default();
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.push(&provider, &backend))
            .expect("push");

        assert_eq!(summary.deleted, vec![key.clone()]);
        let remaining = cache::load_deleted_keys(&paths).expect("deleted keys");
        assert!(!remaining.contains(&key));
        assert!(cache::load_backend_state(&paths).expect("state").is_empty());
        assert_eq!(
            provider.state.lock().expect("lock").deleted,
            vec![("owner/repo".into(), 42)]
        );
    }

    #[test]
    fn push_keeps_tombstone_when_delete_soft_closes() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let key = backend_state_key("github", "owner/repo", 42);
        cache::mark_deleted(&paths, &key).expect("mark deleted");
        let mut state = HashMap::new();
        state.insert(
            key.clone(),
            backend_state_entry(&record(42, "Remote issue", "open", "2026-03-20T10:00:00Z")),
        );
        cache::save_backend_state(&paths, &state).expect("save state");

        let provider =
            FakeBackendProvider::default().with_delete_outcome(DeleteOutcome::SoftClosed);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.push(&provider, &backend))
            .expect("push");

        assert_eq!(summary.deleted, vec![key.clone()]);
        let remaining = cache::load_deleted_keys(&paths).expect("deleted keys");
        assert!(remaining.contains(&key));
    }

    #[test]
    fn push_skips_conflicted_issues() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let mut document = local_issue("LO-LOC--1", &backend.name, "Conflicted");
        document.frontmatter.conflict = Some(ConflictMeta {
            detected_at: "2026-03-20T12:00:00Z".into(),
            remote_file: "LO-LOC--1.REMOTE.md".into(),
            remote_updated_at: "2026-03-20T11:00:00Z".into(),
            local_updated_at: "2026-03-20T12:00:00Z".into(),
            last_synced_at: "2026-03-20T10:00:00Z".into(),
        });
        save_issue(&paths, &document);

        let provider = FakeBackendProvider::default();
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.push(&provider, &backend))
            .expect("push");

        assert_eq!(summary.skipped, vec!["LO-LOC--1"]);
        assert!(provider.state.lock().expect("lock").created.is_empty());
    }

    #[test]
    fn push_calls_lock_when_local_is_locked() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.local_updated_at = "2026-03-20T12:00:00Z".into();
        document.frontmatter.locked = Some(true);
        document.frontmatter.lock_reason = Some("resolved".into());
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        let provider = FakeBackendProvider::default().with_update_response(
            42,
            record(42, "Remote issue", "open", "2026-03-20T12:30:00Z"),
        );
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.push(&provider, &backend))
            .expect("push");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        let state = provider.state.lock().expect("lock");
        assert_eq!(state.locked.len(), 1);
        assert_eq!(state.locked[0].1, 42);
        assert_eq!(state.locked[0].2, Some("resolved".into()));
    }

    #[test]
    fn push_calls_unlock_when_local_is_unlocked() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let mut original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        original.locked = Some(true);
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.local_updated_at = "2026-03-20T12:00:00Z".into();
        document.frontmatter.locked = Some(false);
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        let mut update_response = record(42, "Remote issue", "open", "2026-03-20T12:30:00Z");
        update_response.locked = Some(true);
        let provider = FakeBackendProvider::default().with_update_response(42, update_response);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.push(&provider, &backend))
            .expect("push");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        let state = provider.state.lock().expect("lock");
        assert_eq!(state.unlocked.len(), 1);
        assert_eq!(state.unlocked[0].1, 42);
    }

    #[test]
    fn status_reports_creates_pushes_deletes_and_conflicts() {
        let (paths, config, backend) = test_context();
        let mut conflicted = local_issue("LO-CON--1", &backend.name, "Conflicted");
        conflicted.frontmatter.conflict = Some(ConflictMeta {
            detected_at: "2026-03-20T12:00:00Z".into(),
            remote_file: "LO-CON--1.REMOTE.md".into(),
            remote_updated_at: "2026-03-20T11:00:00Z".into(),
            local_updated_at: "2026-03-20T12:00:00Z".into(),
            last_synced_at: "2026-03-20T10:00:00Z".into(),
        });
        save_issue(&paths, &conflicted);

        let create = local_issue("LO-NEW--1", &backend.name, "Create me");
        save_issue(&paths, &create);

        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let mut push = backend_to_local(&original, &backend);
        push.frontmatter.local_updated_at = "2026-03-20T12:00:00Z".into();
        save_issue(&paths, &push);
        seed_backend_state(&paths, &backend, &original);

        let delete_key = backend_state_key("github", "owner/repo", 77);
        cache::mark_deleted(&paths, &delete_key).expect("mark deleted");

        let summary = SyncEngine::new(&paths, &config).status().expect("status");

        assert_eq!(summary.conflicts, vec!["LO-CON--1"]);
        assert_eq!(summary.creates, vec!["LO-NEW--1"]);
        assert_eq!(summary.pushes, vec!["GH-OWN-REP--42"]);
        assert_eq!(summary.deletes, vec![delete_key]);
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    fn test_context() -> (AppPaths, Config, BackendConfig) {
        let temp = tempdir().expect("temp dir");
        let root = temp.path().to_path_buf();
        std::mem::forget(temp);
        let repo = root.join("repo");
        let cache_root = root.join("cache");
        let paths = AppPaths {
            riptsk_repo: repo.to_string_lossy().as_ref().into(),
            cache_root: cache_root.to_string_lossy().as_ref().into(),
        };
        paths.ensure_repo_dirs().expect("repo dirs");
        let backend = BackendConfig {
            name: "remote-project".into(),
            backend: Backend::Github,
            host: None,
            repo: Some("owner/repo".into()),
            default_board: Some("personal".into()),
            default_org: None,
            path: None,
        };
        let mut config = default_config();
        config.backends = vec![backend.clone()];
        (paths, config, backend)
    }

    fn record(issue_id: u64, title: &str, state: &str, updated_at: &str) -> BackendIssueRecord {
        BackendIssueRecord {
            issue_id,
            node_id: Some(format!("node-{issue_id}")),
            title: title.into(),
            state: state.into(),
            state_reason: None,
            labels: vec!["status::todo".into()],
            assignees: Vec::new(),
            milestone: None,
            milestone_id: None,
            body: Some(format!("{title} body")),
            url: format!("https://example.invalid/issues/{issue_id}"),
            updated_at: updated_at.into(),
            due_date: None,
            weight: None,
            confidential: None,
            discussion_locked: None,
            issue_type: None,
            locked: None,
            lock_reason: None,
            comments: Vec::new(),
            linked_mrs: Vec::new(),
        }
    }

    fn local_issue(id: &str, project: &str, title: &str) -> IssueDocument {
        IssueDocument {
            frontmatter: IssueFrontmatter {
                id: id.into(),
                title: title.into(),
                status: IssueState::Todo,
                board: "personal".into(),
                project: project.into(),
                org: None,
                priority: Some(Priority::Medium),
                labels: vec!["bug".into()],
                assignees: Vec::new(),
                milestone: None,
                state_reason: None,
                cycle: None,
                order: Some(1),
                gitlab: None,
                github: None,
                local_updated_at: "2026-03-20T12:00:00Z".into(),
                due: None,
                weight: None,
                confidential: None,
                discussion_locked: None,
                issue_type: None,
                locked: None,
                lock_reason: None,
                recurring: None,
                remote_deleted: false,
                conflict: None,
                conflict_role: None,
                conflict_parent: None,
                id_slug: Some("slug".into()),
                branch: None,
                pr_url: None,
                pr_number: None,
            },
            body: format!("{title} body"),
            remote_section: None,
        }
    }

    fn save_issue(paths: &AppPaths, issue: &IssueDocument) {
        frontmatter::save_issue(
            paths
                .issues_dir()
                .join(format!("{}.md", issue.frontmatter.id))
                .as_std_path(),
            issue,
        )
        .expect("save issue");
    }

    fn save_remote_issue(paths: &AppPaths, id: &str) {
        let remote = IssueDocument {
            frontmatter: IssueFrontmatter {
                id: id.into(),
                title: "Remote".into(),
                status: IssueState::Todo,
                board: "personal".into(),
                project: "remote-project".into(),
                org: None,
                priority: Some(Priority::Medium),
                labels: Vec::new(),
                assignees: Vec::new(),
                milestone: None,
                state_reason: None,
                cycle: None,
                order: Some(1),
                gitlab: None,
                github: Some(GithubIssueMeta {
                    repo: "owner/repo".into(),
                    issue_id: Some(42),
                    node_id: Some("node-42".into()),
                    milestone_id: None,
                    url: Some("https://example.invalid/issues/42".into()),
                    updated_at: "2026-03-20T10:00:00Z".into(),
                    last_pushed_state: Some(IssueState::Todo),
                }),
                local_updated_at: "2026-03-20T10:00:00Z".into(),
                due: None,
                weight: None,
                confidential: None,
                discussion_locked: None,
                issue_type: None,
                locked: None,
                lock_reason: None,
                recurring: None,
                remote_deleted: false,
                conflict: None,
                conflict_role: Some("remote".into()),
                conflict_parent: Some(id.into()),
                id_slug: Some("slug".into()),
                branch: None,
                pr_url: None,
                pr_number: None,
            },
            body: "remote".into(),
            remote_section: None,
        };
        frontmatter::save_issue(
            paths
                .issues_dir()
                .join(format!("{id}.REMOTE.md"))
                .as_std_path(),
            &remote,
        )
        .expect("save remote issue");
    }

    fn load_issue(paths: &AppPaths, id: &str) -> IssueDocument {
        frontmatter::load_issue(paths.issues_dir().join(format!("{id}.md")).as_std_path())
            .expect("load issue")
    }

    fn seed_backend_state(paths: &AppPaths, backend: &BackendConfig, record: &BackendIssueRecord) {
        let mut state = HashMap::new();
        state.insert(
            backend_state_key(
                provider_name(backend),
                backend.repo.as_deref().expect("repo"),
                record.issue_id,
            ),
            backend_state_entry(record),
        );
        cache::save_backend_state(paths, &state).expect("save state");
    }
}
