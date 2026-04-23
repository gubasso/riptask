use crate::adapters::backend::{BackendIssueRecord, DeleteOutcome, IssueTracker};
use crate::adapters::git::{CliGit, GitBackend};
use crate::config::Config;
use crate::domain::backend_state::backend_state_key;
use crate::domain::issue::IssueDocument;
use crate::error::RiptskError;
use crate::models::{BackendKind, RepoProject};
use crate::paths::AppPaths;
use crate::services::backend_mapping;
use crate::services::backend_mapping::{
    backend_state_entry, backend_to_local, copy_local_only_fields, current_timestamp,
    issue_to_upsert, update_issue_from_backend,
};
use crate::services::issue_ids;
use crate::storage::{cache, frontmatter, issue_store};
use std::collections::HashSet;
use std::fs;

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

    pub async fn pull<P: IssueTracker + ?Sized>(
        &self,
        provider: &P,
        repo_project: &RepoProject,
        force: bool,
        filter_ids: Option<&HashSet<String>>,
        force_ids: Option<&HashSet<String>>,
    ) -> Result<PullSummary, RiptskError> {
        let repo = sync_repo(repo_project)?;
        let mut backend_state =
            cache::load_backend_state(self.paths).map_err(RiptskError::Other)?;
        let deleted_keys = cache::load_deleted_keys(self.paths).map_err(RiptskError::Other)?;
        let records = provider.list_issues(&repo).await?;
        let mut summary = PullSummary::default();
        let mut seen_ids = HashSet::new();

        for record in records {
            let record_id =
                issue_ids::format_id(&issue_ids::effective_key(repo_project), record.issue_id);
            if filter_ids.is_some_and(|ids| !ids.contains(&record_id)) {
                continue;
            }
            let key = backend_state_key(provider_name(repo_project), &repo, record.issue_id);
            seen_ids.insert(record.issue_id);
            if deleted_keys.contains(&key) {
                continue;
            }
            let cached = backend_state.get(&key).cloned();
            let local_path = self.find_local_issue(repo_project, record.issue_id)?;

            if let Some(path) = local_path {
                let mut issue = match frontmatter::try_load_issue(path.as_std_path()) {
                    frontmatter::IssueLoadResult::Ok(issue) => issue,
                    frontmatter::IssueLoadResult::Conflict { id, .. } => {
                        summary.conflicts.push(id);
                        continue;
                    }
                    frontmatter::IssueLoadResult::Err(error) => {
                        return Err(RiptskError::Other(error));
                    }
                };
                let local_changed = cached
                    .as_ref()
                    .is_some_and(|entry| issue.frontmatter.local_updated_at > entry.updated_at);
                let remote_changed = cached
                    .as_ref()
                    .is_none_or(|entry| record.updated_at > entry.updated_at);

                let force_this = force || force_ids.is_some_and(|ids| ids.contains(&record_id));
                if !force_this
                    && self.config.sync.conflict_detection
                    && local_changed
                    && remote_changed
                {
                    self.write_conflict(repo_project, &record, &issue, &path)?;
                    summary.conflicts.push(issue.frontmatter.id.clone());
                } else if remote_changed {
                    update_issue_from_backend(&mut issue, &record, repo_project);
                    frontmatter::save_issue(path.as_std_path(), &issue)
                        .map_err(RiptskError::Other)?;
                    summary.updated.push(issue.frontmatter.id.clone());
                }
            } else {
                let document = backend_to_local(&record, repo_project);
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

        if filter_ids.is_none() {
            for path in issue_store::list_issues(self.paths)? {
                let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                    frontmatter::IssueLoadResult::Ok(issue) => issue,
                    frontmatter::IssueLoadResult::Conflict { .. } => continue,
                    frontmatter::IssueLoadResult::Err(error) => {
                        return Err(RiptskError::Other(error));
                    }
                };
                // Limit the deletion sweep to issues that belong to THIS
                // RepoProject. Without this guard, a Jira shared-project pull
                // (filtered by `repo_project_label`) would delete every other
                // RepoProject's local issues because they share `project_key`
                // but never appear in the label-filtered `seen_ids` set.
                if issue.frontmatter.project != repo_project.name {
                    continue;
                }
                let meta =
                    match repo_project.tasks_backend.kind {
                        BackendKind::Github => issue.frontmatter.github.as_ref().and_then(|meta| {
                            (meta.repo == repo).then_some(meta.issue_id).flatten()
                        }),
                        BackendKind::Gitlab => issue.frontmatter.gitlab.as_ref().and_then(|meta| {
                            (meta.repo == repo).then_some(meta.issue_id).flatten()
                        }),
                        BackendKind::Jira => issue.frontmatter.jira.as_ref().and_then(|meta| {
                            let expected_key =
                                crate::adapters::jira::JiraProvider::project_key(&repo);
                            (meta.project_key == expected_key)
                                .then_some(meta.issue_id)
                                .flatten()
                        }),
                        BackendKind::Local => None,
                    };
                let Some(issue_id) = meta else {
                    continue;
                };
                if seen_ids.contains(&issue_id) {
                    continue;
                }
                issue_store::delete_issue_files(self.paths, &issue.frontmatter.id)?;
                backend_state.remove(&backend_state_key(
                    provider_name(repo_project),
                    &repo,
                    issue_id,
                ));
                summary.deleted.push(issue.frontmatter.id.clone());
            }
        }

        cache::save_backend_state(self.paths, &backend_state).map_err(RiptskError::Other)?;
        Ok(summary)
    }

    pub async fn push<P: IssueTracker + ?Sized>(
        &self,
        provider: &P,
        repo_project: &RepoProject,
    ) -> Result<PushSummary, RiptskError> {
        let issue_paths = issue_store::list_issues(self.paths)?;
        self.push_inner(provider, repo_project, &issue_paths, true)
            .await
    }

    pub async fn push_issues<P: IssueTracker + ?Sized>(
        &self,
        provider: &P,
        repo_project: &RepoProject,
        issue_paths: &[camino::Utf8PathBuf],
    ) -> Result<PushSummary, RiptskError> {
        self.push_inner(provider, repo_project, issue_paths, false)
            .await
    }

    pub fn status(&self) -> Result<StatusSummary, RiptskError> {
        let backend_state = cache::load_backend_state(self.paths).map_err(RiptskError::Other)?;
        let deleted_keys = cache::load_deleted_keys(self.paths).map_err(RiptskError::Other)?;
        let mut summary = StatusSummary::default();

        for path in issue_store::list_issues(self.paths)? {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { id, .. } => {
                    summary.conflicts.push(id);
                    continue;
                }
                frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
            };

            let Some(repo_project) = self.project_for_name(&issue.frontmatter.project) else {
                continue;
            };

            let backend_key = issue_backend_key(&issue, repo_project);
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

    async fn push_inner<P: IssueTracker + ?Sized>(
        &self,
        provider: &P,
        repo_project: &RepoProject,
        issue_paths: &[camino::Utf8PathBuf],
        include_deletes: bool,
    ) -> Result<PushSummary, RiptskError> {
        let repo = sync_repo(repo_project)?;
        let mut backend_state =
            cache::load_backend_state(self.paths).map_err(RiptskError::Other)?;
        let mut deleted_keys = cache::load_deleted_keys(self.paths).map_err(RiptskError::Other)?;
        let mut summary = PushSummary::default();

        for path in issue_paths {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { id, .. } => {
                    summary.skipped.push(id);
                    continue;
                }
                frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
            };
            if issue.frontmatter.project != repo_project.name || issue.frontmatter.remote_deleted {
                continue;
            }

            let upsert = issue_to_upsert(&issue, repo_project);
            let outbound_labels = effective_outbound_labels(&upsert.labels, repo_project);
            let issue_id = issue_backend_issue_id(&issue, repo_project);
            if let Some(issue_id) = issue_id {
                let key = backend_state_key(provider_name(repo_project), &repo, issue_id);
                let cached = backend_state.get(&key);
                if cached
                    .as_ref()
                    .is_some_and(|entry| issue.frontmatter.local_updated_at <= entry.updated_at)
                {
                    summary.skipped.push(issue.frontmatter.id.clone());
                    continue;
                }

                let mut record = provider.update_issue(&repo, issue_id, &upsert).await?;
                provider
                    .sync_labels(&repo, issue_id, &outbound_labels)
                    .await?;
                sync_lock_state(provider, &repo, issue_id, &issue, &record).await?;
                // Trigger state transitions (close/reopen) if the desired state
                // differs from what the backend returned.
                if let Some(ref desired_state) = upsert.state {
                    let is_closed = record.state.eq_ignore_ascii_case("closed");
                    if desired_state == "closed" && !is_closed {
                        provider
                            .close_issue(&repo, issue_id, upsert.state_reason.as_deref())
                            .await?;
                    } else if desired_state == "open" && is_closed {
                        provider.reopen_issue(&repo, issue_id).await?;
                    }
                }
                if let Some(state) = upsert.state.clone() {
                    record.state = state;
                }
                record.state_reason = upsert.state_reason.clone();
                record.discussion_locked = issue.frontmatter.discussion_locked;
                record.locked = issue.frontmatter.locked;
                record.lock_reason = issue.frontmatter.lock_reason.clone();
                record.updated_at = current_timestamp();

                let mut updated = issue;
                update_issue_from_backend(&mut updated, &record, repo_project);
                frontmatter::save_issue(path.as_std_path(), &updated)
                    .map_err(RiptskError::Other)?;
                backend_state.insert(key, backend_state_entry(&record));
                summary.updated.push(updated.frontmatter.id.clone());
                continue;
            }

            let mut record = provider.create_issue(&repo, &upsert).await?;
            provider
                .sync_labels(&repo, record.issue_id, &outbound_labels)
                .await?;
            if issue.frontmatter.status == crate::domain::issue::IssueState::Done {
                provider
                    .close_issue(&repo, record.issue_id, upsert.state_reason.as_deref())
                    .await?;
                record.state = "closed".into();
            } else {
                provider.reopen_issue(&repo, record.issue_id).await?;
                record.state = "open".into();
            }
            sync_lock_state(provider, &repo, record.issue_id, &issue, &record).await?;
            record.state_reason = upsert.state_reason.clone();
            record.discussion_locked = issue.frontmatter.discussion_locked;
            record.locked = issue.frontmatter.locked;
            record.lock_reason = issue.frontmatter.lock_reason.clone();
            record.updated_at = current_timestamp();

            let mut created = issue;
            update_issue_from_backend(&mut created, &record, repo_project);
            frontmatter::save_issue(path.as_std_path(), &created).map_err(RiptskError::Other)?;
            backend_state.insert(
                backend_state_key(provider_name(repo_project), &repo, record.issue_id),
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
                if key_provider != provider_name(repo_project) || key_repo != repo {
                    continue;
                }
                let outcome = provider.delete_issue(&repo, issue_id).await?;
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
        repo_project: &RepoProject,
        issue_id: u64,
    ) -> Result<Option<camino::Utf8PathBuf>, RiptskError> {
        let exact_id = issue_ids::format_id(&issue_ids::effective_key(repo_project), issue_id);
        if let Ok(path) = issue_store::find_issue(self.paths, &exact_id) {
            return Ok(Some(path));
        }

        for path in issue_store::list_issues(self.paths)? {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { .. } => continue,
                frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
            };
            let matches = match repo_project.tasks_backend.kind {
                BackendKind::Github => issue.frontmatter.github.as_ref().is_some_and(|meta| {
                    meta.repo == repo_project.tasks_backend.repo.clone().unwrap_or_default()
                        && meta.issue_id == Some(issue_id)
                }),
                BackendKind::Gitlab => issue.frontmatter.gitlab.as_ref().is_some_and(|meta| {
                    meta.repo == repo_project.tasks_backend.repo.clone().unwrap_or_default()
                        && meta.issue_id == Some(issue_id)
                }),
                BackendKind::Jira => issue.frontmatter.jira.as_ref().is_some_and(|meta| {
                    let expected_key = crate::adapters::jira::JiraProvider::project_key(
                        repo_project
                            .tasks_backend
                            .jira_project
                            .as_deref()
                            .unwrap_or_default(),
                    );
                    meta.project_key == expected_key && meta.issue_id == Some(issue_id)
                }),
                BackendKind::Local => false,
            };
            if matches {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    fn project_for_name(&self, project: &str) -> Option<&RepoProject> {
        self.config.projects.iter().find(|repo_project| {
            repo_project.name == project
                && matches!(
                    repo_project.tasks_backend.kind,
                    BackendKind::Github | BackendKind::Gitlab | BackendKind::Jira
                )
        })
    }

    fn write_conflict(
        &self,
        repo_project: &RepoProject,
        record: &BackendIssueRecord,
        issue: &IssueDocument,
        issue_path: &camino::Utf8PathBuf,
    ) -> Result<(), RiptskError> {
        let local_path = issue_store::local_backup_path(self.paths, &issue.frontmatter.id);
        fs::copy(issue_path, &local_path)?;

        let mut remote_doc = backend_to_local(record, repo_project);
        copy_local_only_fields(&mut remote_doc.frontmatter, &issue.frontmatter);
        let remote_path = issue_store::remote_backup_path(self.paths, &issue.frontmatter.id);
        frontmatter::save_issue(remote_path.as_std_path(), &remote_doc)
            .map_err(RiptskError::Other)?;

        let empty_base =
            tempfile::NamedTempFile::new_in(self.paths.issues_dir()).map_err(RiptskError::Io)?;
        let merged = CliGit::new().merge_file(
            local_path.as_std_path(),
            empty_base.path(),
            remote_path.as_std_path(),
        )?;
        fs::write(issue_path, merged)?;
        Ok(())
    }
}

async fn sync_lock_state<P: IssueTracker + ?Sized>(
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

fn provider_name(repo_project: &RepoProject) -> &'static str {
    match repo_project.tasks_backend.kind {
        BackendKind::Github => "github",
        BackendKind::Gitlab => "gitlab",
        BackendKind::Jira => "jira",
        BackendKind::Local => "local",
    }
}

fn issue_backend_issue_id(issue: &IssueDocument, repo_project: &RepoProject) -> Option<u64> {
    match repo_project.tasks_backend.kind {
        BackendKind::Github => issue
            .frontmatter
            .github
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Gitlab => issue
            .frontmatter
            .gitlab
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Jira => issue
            .frontmatter
            .jira
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Local => None,
    }
}

fn issue_backend_key(issue: &IssueDocument, repo_project: &RepoProject) -> Option<String> {
    let repo = sync_repo(repo_project).ok()?;
    let issue_id = issue_backend_issue_id(issue, repo_project)?;
    Some(backend_state_key(
        provider_name(repo_project),
        &repo,
        issue_id,
    ))
}

fn sync_repo(repo_project: &RepoProject) -> Result<String, RiptskError> {
    match repo_project.tasks_backend.kind {
        BackendKind::Github | BackendKind::Gitlab => {
            repo_project.tasks_backend.repo.clone().ok_or_else(|| {
                RiptskError::Config(format!(
                    "RepoProject {} is missing TasksBackend repo",
                    repo_project.name
                ))
            })
        }
        BackendKind::Jira => repo_project
            .tasks_backend
            .jira_project
            .clone()
            .ok_or_else(|| {
                RiptskError::Config(format!(
                    "RepoProject {} is missing JiraProject",
                    repo_project.name
                ))
            }),
        BackendKind::Local => Err(RiptskError::Config(format!(
            "RepoProject {} does not have a hosted TasksBackend",
            repo_project.name
        ))),
    }
}

fn effective_outbound_labels(base: &[String], repo_project: &RepoProject) -> Vec<String> {
    let mut labels = base.to_vec();
    if let Some(label) = backend_mapping::effective_repo_project_label(repo_project)
        && !labels.iter().any(|candidate| candidate == label)
    {
        labels.push(label.to_owned());
    }
    labels
}

fn parse_backend_state_key(key: &str) -> Option<(&str, &str, u64)> {
    let (provider, rest) = key.split_once(':')?;
    let (repo, issue_id) = rest.rsplit_once(':')?;
    Some((provider, repo, issue_id.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::backend::{BackendIssueUpsert, DeleteOutcome};
    use crate::config::default_config;
    use crate::domain::issue::{GithubIssueMeta, IssueFrontmatter, IssueState, Priority};
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet, VecDeque};
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
    struct FakeIssueTracker {
        state: Arc<Mutex<FakeState>>,
    }

    impl FakeIssueTracker {
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
    impl crate::adapters::backend::IssueTracker for FakeIssueTracker {
        async fn list_issues(&self, _repo: &str) -> Result<Vec<BackendIssueRecord>, RiptskError> {
            Ok(self.state.lock().expect("lock").listed.clone())
        }

        async fn get_issue(
            &self,
            _repo: &str,
            _issue_id: u64,
        ) -> Result<BackendIssueRecord, RiptskError> {
            Err(RiptskError::General("unused in test".into()))
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

        async fn close_issue(
            &self,
            repo: &str,
            issue_id: u64,
            _state_reason: Option<&str>,
        ) -> Result<(), RiptskError> {
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
    }

    #[test]
    fn pull_creates_local_issues_from_remote() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "Remote issue",
            "open",
            "2026-03-20T10:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
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

        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "New title",
            "open",
            "2026-03-20T12:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
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

        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "Remote title",
            "open",
            "2026-03-20T11:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
            .expect("pull");

        assert_eq!(summary.conflicts, vec!["GH-OWN-REP--42"]);
        assert!(paths.issues_dir().join("GH-OWN-REP--42.LOCAL.md").exists());
        assert!(paths.issues_dir().join("GH-OWN-REP--42.REMOTE.md").exists());
        let merged = std::fs::read_to_string(paths.issues_dir().join("GH-OWN-REP--42.md"))
            .expect("read merged file");
        assert!(frontmatter::has_conflict_markers(&merged));
    }

    #[test]
    fn done_skips_local_close_when_remote_auto_closed() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.branch = Some("feature/42".into());
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        document.frontmatter.branch = None;
        document.frontmatter.id_slug = None;
        save_issue(&paths, &document);

        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "Remote issue",
            "closed",
            "2026-03-20T12:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
            .expect("pull");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        assert!(summary.conflicts.is_empty());
        let saved = load_issue(&paths, "GH-OWN-REP--42");
        assert_eq!(saved.frontmatter.status, IssueState::Done);
        assert_eq!(saved.frontmatter.branch, None);
    }

    #[test]
    fn force_pull_after_done_prevents_conflict_when_auto_closed() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.branch = Some("feature/42".into());
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        document.frontmatter.branch = None;
        document.frontmatter.id_slug = None;
        save_issue(&paths, &document);

        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "Remote issue",
            "closed",
            "2026-03-20T12:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);
        let mut force_ids = HashSet::new();
        force_ids.insert("GH-OWN-REP--42".to_string());

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, true, Some(&force_ids), None))
            .expect("force pull");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        assert!(summary.conflicts.is_empty());
        assert_eq!(
            load_issue(&paths, "GH-OWN-REP--42").frontmatter.status,
            IssueState::Done
        );

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
            .expect("normal pull");

        assert!(summary.updated.is_empty());
        assert!(summary.conflicts.is_empty());
    }

    #[test]
    fn force_pull_after_done_no_conflict_when_remote_still_open() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.branch = Some("feature/42".into());
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        document.frontmatter.branch = None;
        document.frontmatter.id_slug = None;
        save_issue(&paths, &document);

        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "Remote issue",
            "open",
            "2026-03-20T12:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);
        let mut force_ids = HashSet::new();
        force_ids.insert("GH-OWN-REP--42".to_string());

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, true, Some(&force_ids), None))
            .expect("force pull");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        assert!(summary.conflicts.is_empty());

        let mut issue = load_issue(&paths, "GH-OWN-REP--42");
        issue.frontmatter.status = IssueState::Done;
        issue.frontmatter.local_updated_at = "2026-03-20T13:00:00Z".into();
        issue.frontmatter.branch = None;
        issue.frontmatter.id_slug = None;
        save_issue(&paths, &issue);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
            .expect("normal pull");

        assert!(summary.conflicts.is_empty());
    }

    #[test]
    fn force_ids_prevents_conflict_when_auto_close_races_normal_pull() {
        // Simulates: done sets local to "done", then normal pull sees the
        // auto-close that arrived after the get_issue check. With force_ids
        // the pull must not conflict.
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.branch = Some("feature/42".into());
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        // done clears branch and sets status to Done
        document.frontmatter.branch = None;
        document.frontmatter.id_slug = None;
        document.frontmatter.status = IssueState::Done;
        document.frontmatter.local_updated_at = "2026-03-20T13:00:00Z".into();
        save_issue(&paths, &document);

        // Remote auto-closed after the get_issue check
        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "Remote issue",
            "closed",
            "2026-03-20T12:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);
        let mut force_ids = HashSet::new();
        force_ids.insert("GH-OWN-REP--42".to_string());

        // Normal pull with force_ids — must not conflict
        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, Some(&force_ids)))
            .expect("pull with force_ids");

        assert!(summary.conflicts.is_empty());
        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        assert_eq!(
            load_issue(&paths, "GH-OWN-REP--42").frontmatter.status,
            IssueState::Done
        );
    }

    #[test]
    fn done_closes_locally_when_remote_still_open() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Remote issue", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        document.frontmatter.status = IssueState::Done;
        document.frontmatter.branch = None;
        document.frontmatter.local_updated_at = "2026-03-20T12:00:00Z".into();
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        let provider = FakeIssueTracker::default().with_update_response(
            42,
            record(42, "Remote issue", "open", "2026-03-20T12:30:00Z"),
        );
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.push(&provider, &backend))
            .expect("push");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        let saved = load_issue(&paths, "GH-OWN-REP--42");
        assert_eq!(saved.frontmatter.status, IssueState::Done);
        assert_eq!(saved.frontmatter.branch, None);

        let state = provider.state.lock().expect("lock");
        assert_eq!(state.updated.len(), 1);
        assert_eq!(state.updated[0].2.state.as_deref(), Some("closed"));
    }

    #[test]
    fn pull_conflict_preserves_local_only_fields() {
        let (paths, config, backend) = test_context();
        let original = record(42, "Old title", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        apply_local_only_fields(&mut document);
        document.body = "local body".into();
        save_issue(&paths, &document);

        let engine = SyncEngine::new(&paths, &config);
        let issue_path = paths.issues_dir().join("GH-OWN-REP--42.md");
        let mut remote = original.clone();
        remote.body = Some("remote body".into());

        engine
            .write_conflict(&backend, &remote, &document, &issue_path)
            .expect("write conflict");
        let remote = frontmatter::load_issue(
            paths
                .issues_dir()
                .join("GH-OWN-REP--42.REMOTE.md")
                .as_std_path(),
        )
        .expect("load remote backup");
        assert_local_only_fields(&remote);

        let merged = std::fs::read_to_string(paths.issues_dir().join("GH-OWN-REP--42.md"))
            .expect("read merged file");
        assert!(frontmatter::has_conflict_markers(&merged));
        assert_fields_not_conflicted(
            &merged,
            &[
                "pr_url: https://example.invalid/pulls/42",
                "pr_number: 42",
                "branch: feature/42",
                "order: 7",
                "recurring: weekly-42",
                "cycle: 2026-W13",
            ],
        );
    }

    #[test]
    fn pull_update_preserves_local_only_fields() {
        let runtime = runtime();
        let (paths, config, backend) = test_context();
        let original = record(42, "Old title", "open", "2026-03-20T10:00:00Z");
        let mut document = backend_to_local(&original, &backend);
        apply_local_only_fields(&mut document);
        document.frontmatter.local_updated_at = "2026-03-20T10:00:00Z".into();
        save_issue(&paths, &document);
        seed_backend_state(&paths, &backend, &original);

        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "New title",
            "open",
            "2026-03-20T12:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
            .expect("pull");

        assert_eq!(summary.updated, vec!["GH-OWN-REP--42"]);
        let saved = load_issue(&paths, "GH-OWN-REP--42");
        assert_eq!(saved.frontmatter.title, "New title");
        assert_local_only_fields(&saved);
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

        let provider = FakeIssueTracker::with_listed(Vec::new());
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
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
        let provider = FakeIssueTracker::with_listed(vec![record(
            42,
            "Remote issue",
            "open",
            "2026-03-20T10:00:00Z",
        )]);
        let engine = SyncEngine::new(&paths, &config);

        let summary = runtime
            .block_on(engine.pull(&provider, &backend, false, None, None))
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

        let provider = FakeIssueTracker::default().with_update_response(
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

        let provider = FakeIssueTracker::default().with_create_response(record(
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

        let provider = FakeIssueTracker::default();
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

        let provider = FakeIssueTracker::default().with_delete_outcome(DeleteOutcome::SoftClosed);
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
        save_conflicted_issue(&paths, "LO-LOC--1");

        let provider = FakeIssueTracker::default();
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

        let provider = FakeIssueTracker::default().with_update_response(
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
        let provider = FakeIssueTracker::default().with_update_response(42, update_response);
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
        save_conflicted_issue(&paths, "LO-CON--1");

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

    fn test_context() -> (AppPaths, Config, RepoProject) {
        let temp = tempdir().expect("temp dir");
        let root = temp.path().to_path_buf();
        std::mem::forget(temp);
        let repo = root.join("repo");
        let cache_root = root.join("cache");
        let paths = AppPaths {
            riptsk_repo: repo.to_string_lossy().as_ref().into(),
            cache_root: cache_root.to_string_lossy().as_ref().into(),
            state_root: root.join("state").to_string_lossy().as_ref().into(),
        };
        paths.ensure_repo_dirs().expect("repo dirs");
        let backend = RepoProject {
            name: "remote-project".into(),
            vc_backend: crate::models::VCBackendSpec {
                kind: BackendKind::Github,
                host: None,
                repo: Some("owner/repo".into()),
                path: None,
            },
            tasks_backend: crate::models::TasksBackendSpec {
                kind: BackendKind::Github,
                host: None,
                repo: Some("owner/repo".into()),
                jira_project: None,
                default_issue_type: None,
                path: None,
            },
            default_board: Some("personal".into()),
            default_org: None,
            key: Some("GH-OWN-REP".into()),
            repo_project_label: None,
        };
        let mut config = default_config();
        config.projects = vec![backend.clone()];
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
            assignee_account_id: None,
            assignee_name: None,
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
                jira: None,
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
                id_slug: Some("slug".into()),
                branch: None,
                pr_url: None,
                pr_number: None,
            },
            body: format!("{title} body"),
            remote_section: None,
        }
    }

    fn apply_local_only_fields(issue: &mut IssueDocument) {
        issue.frontmatter.pr_url = Some("https://example.invalid/pulls/42".into());
        issue.frontmatter.pr_number = Some(42);
        issue.frontmatter.branch = Some("feature/42".into());
        issue.frontmatter.order = Some(7);
        issue.frontmatter.recurring = Some("weekly-42".into());
        issue.frontmatter.cycle = Some("2026-W13".into());
        issue.frontmatter.board = "ops".into();
        issue.frontmatter.org = Some("eng".into());
        issue.frontmatter.priority = Some(Priority::High);
    }

    fn assert_local_only_fields(issue: &IssueDocument) {
        assert_eq!(
            issue.frontmatter.pr_url.as_deref(),
            Some("https://example.invalid/pulls/42")
        );
        assert_eq!(issue.frontmatter.pr_number, Some(42));
        assert_eq!(issue.frontmatter.branch.as_deref(), Some("feature/42"));
        assert_eq!(issue.frontmatter.order, Some(7));
        assert_eq!(issue.frontmatter.recurring.as_deref(), Some("weekly-42"));
        assert_eq!(issue.frontmatter.cycle.as_deref(), Some("2026-W13"));
        assert_eq!(issue.frontmatter.board, "ops");
        assert_eq!(issue.frontmatter.org.as_deref(), Some("eng"));
        assert_eq!(issue.frontmatter.priority, Some(Priority::High));
    }

    fn assert_fields_not_conflicted(content: &str, fields: &[&str]) {
        let mut in_conflict = false;
        for line in content.lines() {
            if line.starts_with("<<<<<<< ") {
                in_conflict = true;
                continue;
            }
            if line.starts_with(">>>>>>> ") {
                in_conflict = false;
                continue;
            }
            if fields.contains(&line) {
                assert!(
                    !in_conflict,
                    "field line unexpectedly appeared inside conflict block: {line}"
                );
            }
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
                jira: None,
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

    fn save_conflicted_issue(paths: &AppPaths, id: &str) {
        std::fs::write(
            paths.issues_dir().join(format!("{id}.md")),
            "<<<<<<< LOCAL\nlocal\n=======\nremote\n>>>>>>> REMOTE\n",
        )
        .expect("save conflicted issue");
    }

    fn load_issue(paths: &AppPaths, id: &str) -> IssueDocument {
        frontmatter::load_issue(paths.issues_dir().join(format!("{id}.md")).as_std_path())
            .expect("load issue")
    }

    fn seed_backend_state(paths: &AppPaths, backend: &RepoProject, record: &BackendIssueRecord) {
        let mut state = HashMap::new();
        state.insert(
            backend_state_key(
                provider_name(backend),
                &sync_repo(backend).expect("repo"),
                record.issue_id,
            ),
            backend_state_entry(record),
        );
        cache::save_backend_state(paths, &state).expect("save state");
    }
}
