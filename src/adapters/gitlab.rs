use crate::adapters::backend::{
    BackendIssueRecord, BackendIssueUpsert, BackendPrRecord, CiPresence, DeleteOutcome,
    IssueTracker, MergeMethod, PrChecksStatus, VersionControl,
};
use crate::error::RiptaskError;
use async_trait::async_trait;
use chrono::NaiveDate;
use gitlab::api::AsyncQuery;
use serde::Deserialize;

#[derive(Clone)]
pub struct GitlabProvider {
    host: String,
    token: String,
}

impl std::fmt::Debug for GitlabProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitlabProvider").finish_non_exhaustive()
    }
}

impl GitlabProvider {
    pub fn new(host: &str, token: &str) -> Result<Self, RiptaskError> {
        Ok(Self {
            host: host.to_owned(),
            token: token.to_owned(),
        })
    }

    async fn client(&self) -> Result<gitlab::AsyncGitlab, RiptaskError> {
        let builder = gitlab::Gitlab::builder(&self.host, &self.token);
        builder
            .build_async()
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))
    }

    async fn create_issue_request(
        &self,
        client: &gitlab::AsyncGitlab,
        repo: &str,
        issue: &BackendIssueUpsert,
        assignee_ids: &[u64],
        include_weight: bool,
    ) -> Result<GitlabIssue, RiptaskError> {
        let mut builder = gitlab::api::projects::issues::CreateIssue::builder();
        builder.project(repo).title(issue.title.as_str());
        builder.description(issue.body.as_str());
        if !issue.labels.is_empty() {
            builder.labels(issue.labels.iter().cloned());
        }
        if let Some(confidential) = issue.confidential {
            builder.confidential(confidential);
        }
        if let Some(milestone_id) = issue.milestone_id {
            builder.milestone_id(milestone_id);
        }
        if let Some(due_date) = parse_naive_date_opt(issue.due_date.as_deref())? {
            builder.due_date(due_date);
        }
        if include_weight && let Some(weight) = issue.weight {
            builder.weight(u64::from(weight));
        }
        if !assignee_ids.is_empty() {
            builder.assignee_ids(assignee_ids.iter().copied());
        }
        let endpoint = builder
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        endpoint
            .query_async(client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))
    }

    async fn edit_issue_request(
        &self,
        client: &gitlab::AsyncGitlab,
        repo: &str,
        issue_id: u64,
        issue: &BackendIssueUpsert,
        assignee_ids: &[u64],
        include_weight: bool,
    ) -> Result<GitlabIssue, RiptaskError> {
        let mut builder = gitlab::api::projects::issues::EditIssue::builder();
        builder.project(repo).issue(issue_id);
        builder.title(issue.title.as_str());
        builder.description(issue.body.as_str());
        builder.labels(issue.labels.iter().cloned());
        if let Some(confidential) = issue.confidential {
            builder.confidential(confidential);
        }
        if let Some(milestone_id) = issue.milestone_id {
            builder.milestone_id(milestone_id);
        }
        if let Some(due_date) = parse_naive_date_opt(issue.due_date.as_deref())? {
            builder.due_date(due_date);
        }
        if include_weight && let Some(weight) = issue.weight {
            builder.weight(u64::from(weight));
        }
        if let Some(discussion_locked) = issue.discussion_locked {
            builder.discussion_locked(discussion_locked);
        }
        if let Some(state) = issue.state.as_deref() {
            builder.state_event(parse_gitlab_issue_state_event(state)?);
        }
        if assignee_ids.is_empty() {
            builder.unassign();
        } else {
            builder.assignee_ids(assignee_ids.iter().copied());
        }
        let endpoint = builder
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        endpoint
            .query_async(client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))
    }

    async fn resolve_user_ids(&self, usernames: &[String]) -> Result<Vec<u64>, RiptaskError> {
        let client = self.client().await?;
        let mut ids = Vec::with_capacity(usernames.len());
        for username in usernames {
            let endpoint = gitlab::api::users::Users::builder()
                .username(username.as_str())
                .build()
                .map_err(|error| RiptaskError::Config(error.to_string()))?;
            let users: Vec<GitlabResolvedUser> = endpoint
                .query_async(&client)
                .await
                .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
            let user = users
                .into_iter()
                .find(|user| user.username == *username)
                .ok_or_else(|| {
                    RiptaskError::Unreachable(format!("GitLab user not found: {username}"))
                })?;
            ids.push(user.id);
        }
        Ok(ids)
    }

    async fn set_discussion_locked(
        &self,
        repo: &str,
        issue_id: u64,
        discussion_locked: bool,
    ) -> Result<(), RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::issues::EditIssue::builder()
            .project(repo)
            .issue(issue_id)
            .discussion_locked(discussion_locked)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let _: GitlabIssue = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(())
    }

    async fn rebase_and_wait(&self, repo: &str, number: u64) -> Result<(), RiptaskError> {
        let client = self.client().await?;

        let rebase_endpoint = gitlab::api::projects::merge_requests::RebaseMergeRequest::builder()
            .project(repo)
            .merge_request(number)
            .build()
            .map_err(|e| RiptaskError::Config(e.to_string()))?;
        gitlab::api::ignore(rebase_endpoint)
            .query_async(&client)
            .await
            .map_err(|e| {
                RiptaskError::General(format!("failed to trigger rebase for MR !{number}: {e}"))
            })?;

        let started = std::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;

            let poll_endpoint = gitlab::api::projects::merge_requests::MergeRequest::builder()
                .project(repo)
                .merge_request(number)
                .include_rebase_in_progress(true)
                .build()
                .map_err(|e| RiptaskError::Config(e.to_string()))?;
            let mr: GitlabMergeRequest = poll_endpoint
                .query_async(&client)
                .await
                .map_err(|e| RiptaskError::Unreachable(e.to_string()))?;

            if let Some(ref error) = mr.merge_error
                && !error.is_empty()
            {
                return Err(RiptaskError::General(format!(
                    "rebase failed for MR !{number}: {error}"
                )));
            }

            if mr.rebase_in_progress != Some(true) {
                return Ok(());
            }

            if started.elapsed() >= std::time::Duration::from_secs(600) {
                return Err(RiptaskError::General(format!(
                    "timed out waiting for rebase of MR !{number}"
                )));
            }
        }
    }
}

#[async_trait]
impl IssueTracker for GitlabProvider {
    async fn list_issues(&self, repo: &str) -> Result<Vec<BackendIssueRecord>, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::issues::ProjectIssues::builder()
            .project(repo)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let issues: Vec<GitlabIssue> = gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(issues.into_iter().map(map_issue).collect())
    }

    async fn get_issue(
        &self,
        repo: &str,
        issue_id: u64,
    ) -> Result<BackendIssueRecord, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::issues::Issue::builder()
            .project(repo)
            .issue(issue_id)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let issue: GitlabIssue = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(map_issue(issue))
    }

    async fn create_issue(
        &self,
        repo: &str,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptaskError> {
        let client = self.client().await?;
        let assignee_ids = self.resolve_user_ids(&issue.assignees).await?;
        let created = match self
            .create_issue_request(&client, repo, issue, &assignee_ids, true)
            .await
        {
            Ok(issue) => issue,
            Err(error) if issue.weight.is_some() && is_weight_error(&error) => {
                crate::ui::warn(&format!(
                    "GitLab create_issue weight failed ({error}), retrying without weight"
                ));
                self.create_issue_request(&client, repo, issue, &assignee_ids, false)
                    .await?
            }
            Err(error) => return Err(error),
        };
        Ok(map_issue(created))
    }

    async fn update_issue(
        &self,
        repo: &str,
        issue_id: u64,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptaskError> {
        let client = self.client().await?;
        let assignee_ids = self.resolve_user_ids(&issue.assignees).await?;
        let updated = match self
            .edit_issue_request(&client, repo, issue_id, issue, &assignee_ids, true)
            .await
        {
            Ok(issue) => issue,
            Err(error) if issue.weight.is_some() && is_weight_error(&error) => {
                crate::ui::warn(&format!(
                    "GitLab update_issue weight failed ({error}), retrying without weight"
                ));
                self.edit_issue_request(&client, repo, issue_id, issue, &assignee_ids, false)
                    .await?
            }
            Err(error) => return Err(error),
        };
        Ok(map_issue(updated))
    }

    async fn close_issue(
        &self,
        repo: &str,
        issue_id: u64,
        _state_reason: Option<&str>,
    ) -> Result<(), RiptaskError> {
        let client = self.client().await?;
        edit_issue_state(
            &client,
            repo,
            issue_id,
            gitlab::api::projects::issues::IssueStateEvent::Close,
        )
        .await
    }

    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptaskError> {
        let client = self.client().await?;
        edit_issue_state(
            &client,
            repo,
            issue_id,
            gitlab::api::projects::issues::IssueStateEvent::Reopen,
        )
        .await
    }

    async fn delete_issue(&self, repo: &str, issue_id: u64) -> Result<DeleteOutcome, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::issues::DeleteIssue::builder()
            .project(repo)
            .issue(issue_id)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        match gitlab::api::ignore(endpoint).query_async(&client).await {
            Ok(()) => Ok(DeleteOutcome::HardDeleted),
            Err(error) => {
                let msg = error.to_string();
                if msg.contains("404") || msg.contains("410") {
                    Ok(DeleteOutcome::HardDeleted)
                } else {
                    Err(RiptaskError::Unreachable(msg))
                }
            }
        }
    }

    async fn lock_issue(
        &self,
        repo: &str,
        issue_id: u64,
        _reason: Option<&str>,
    ) -> Result<(), RiptaskError> {
        self.set_discussion_locked(repo, issue_id, true).await
    }

    async fn unlock_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptaskError> {
        self.set_discussion_locked(repo, issue_id, false).await
    }

    async fn sync_labels(
        &self,
        repo: &str,
        issue_id: u64,
        labels: &[String],
    ) -> Result<(), RiptaskError> {
        let client = self.client().await?;
        let mut builder = gitlab::api::projects::issues::EditIssue::builder();
        builder.project(repo).issue(issue_id);
        builder.labels(labels.iter().cloned());
        let endpoint = builder
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let _: GitlabIssue = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl VersionControl for GitlabProvider {
    async fn create_pr(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::merge_requests::CreateMergeRequest::builder()
            .project(repo)
            .source_branch(head)
            .target_branch(base)
            .title(title)
            .description(body)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let merge_request: GitlabMergeRequest = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(map_merge_request(merge_request))
    }

    async fn get_pr(&self, repo: &str, number: u64) -> Result<BackendPrRecord, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::merge_requests::MergeRequest::builder()
            .project(repo)
            .merge_request(number)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let merge_request: GitlabMergeRequest = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(map_merge_request(merge_request))
    }

    async fn update_pr(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::merge_requests::EditMergeRequest::builder()
            .project(repo)
            .merge_request(number)
            .title(title)
            .description(body)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let merge_request: GitlabMergeRequest = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(map_merge_request(merge_request))
    }

    async fn find_pr_by_branch(
        &self,
        repo: &str,
        head: &str,
        base: &str,
    ) -> Result<Option<BackendPrRecord>, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::merge_requests::MergeRequests::builder()
            .project(repo)
            .source_branch(head)
            .target_branch(base)
            .state(gitlab::api::merge_requests::MergeRequestState::Opened)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let merge_requests: Vec<GitlabMergeRequest> =
            gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
                .query_async(&client)
                .await
                .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(merge_requests.into_iter().next().map(map_merge_request))
    }

    async fn merge_pr(
        &self,
        repo: &str,
        number: u64,
        method: MergeMethod,
        _commit_title: Option<&str>,
        commit_message: Option<&str>,
    ) -> Result<(), RiptaskError> {
        if method == MergeMethod::Rebase {
            self.rebase_and_wait(repo, number).await?;
            let client = self.client().await?;
            let mut builder = gitlab::api::projects::merge_requests::MergeMergeRequest::builder();
            builder.project(repo).merge_request(number);
            if let Some(message) = commit_message {
                builder.merge_commit_message(message);
            }
            let endpoint = builder.build().map_err(|e| {
                RiptaskError::General(format!("failed to build merge request: {e}"))
            })?;
            gitlab::api::ignore(endpoint)
                .query_async(&client)
                .await
                .map_err(|e| {
                    RiptaskError::General(format!("failed to merge MR !{number} after rebase: {e}"))
                })?;
            return Ok(());
        }
        let client = self.client().await?;
        let mut builder = gitlab::api::projects::merge_requests::MergeMergeRequest::builder();
        builder.project(repo).merge_request(number);
        if method == MergeMethod::Squash {
            builder.squash(true);
        }
        if let Some(message) = commit_message {
            builder.merge_commit_message(message);
        }
        let endpoint = builder.build().map_err(|error| {
            RiptaskError::General(format!("failed to build merge request: {error}"))
        })?;
        gitlab::api::ignore(endpoint)
            .query_async(&client)
            .await
            .map_err(|error| {
                RiptaskError::General(format!("failed to merge MR !{number}: {error}"))
            })?;
        Ok(())
    }

    async fn get_pr_checks_status(
        &self,
        repo: &str,
        number: u64,
    ) -> Result<PrChecksStatus, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::merge_requests::MergeRequest::builder()
            .project(repo)
            .merge_request(number)
            .build()
            .map_err(|error| RiptaskError::General(error.to_string()))?;
        let merge_request: GitlabMergeRequest = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        match merge_request.head_pipeline {
            Some(pipeline) => match pipeline.status.as_str() {
                "success" => Ok(PrChecksStatus::Passed),
                "failed" | "canceled" => Ok(PrChecksStatus::Failed),
                "running"
                | "pending"
                | "created"
                | "waiting_for_resource"
                | "preparing"
                | "manual"
                | "scheduled" => Ok(PrChecksStatus::Pending),
                _ => Ok(PrChecksStatus::None),
            },
            None => Ok(PrChecksStatus::None),
        }
    }

    async fn get_ci_presence(&self, repo: &str) -> Result<CiPresence, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::pipelines::Pipelines::builder()
            .project(repo)
            .build()
            .map_err(|error| RiptaskError::General(error.to_string()))?;
        let pipelines: Vec<serde_json::Value> =
            gitlab::api::paged(endpoint, gitlab::api::Pagination::Limit(1))
                .query_async(&client)
                .await
                .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(CiPresence {
            has_remote_ci: !pipelines.is_empty(),
            remote_workflow_names: if pipelines.is_empty() {
                vec![]
            } else {
                vec!["pipeline".into()]
            },
        })
    }

    async fn create_branch(
        &self,
        repo: &str,
        branch_name: &str,
        base_ref: &str,
        _issue_id: Option<u64>,
    ) -> Result<(), RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::repository::branches::CreateBranch::builder()
            .project(repo)
            .branch(branch_name)
            .ref_(base_ref)
            .build()
            .map_err(|e| RiptaskError::Config(e.to_string()))?;
        let _: serde_json::Value = endpoint
            .query_async(&client)
            .await
            .map_err(|e| RiptaskError::Unreachable(e.to_string()))?;
        Ok(())
    }

    async fn default_branch(&self, repo: &str) -> Result<String, RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::Project::builder()
            .project(repo)
            .build()
            .map_err(|error| RiptaskError::Config(error.to_string()))?;
        let project: GitlabProject = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
        Ok(project.default_branch)
    }

    async fn delete_branch(&self, repo: &str, branch_name: &str) -> Result<(), RiptaskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::repository::branches::DeleteBranch::builder()
            .project(repo)
            .branch(branch_name)
            .build()
            .map_err(|e| RiptaskError::Config(e.to_string()))?;
        gitlab::api::ignore(endpoint)
            .query_async(&client)
            .await
            .map_err(|e| RiptaskError::Unreachable(e.to_string()))?;
        Ok(())
    }
}

async fn edit_issue_state(
    client: &gitlab::AsyncGitlab,
    repo: &str,
    issue_id: u64,
    state_event: gitlab::api::projects::issues::IssueStateEvent,
) -> Result<(), RiptaskError> {
    let endpoint = gitlab::api::projects::issues::EditIssue::builder()
        .project(repo)
        .issue(issue_id)
        .state_event(state_event)
        .build()
        .map_err(|error| RiptaskError::Config(error.to_string()))?;
    let _: GitlabIssue = endpoint
        .query_async(client)
        .await
        .map_err(|error| RiptaskError::Unreachable(error.to_string()))?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabUser {
    id: u64,
    username: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabMilestone {
    id: u64,
    title: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabIssue {
    iid: u64,
    title: String,
    description: Option<String>,
    state: String,
    labels: Vec<String>,
    #[serde(default)]
    assignees: Vec<GitlabUser>,
    #[serde(default)]
    milestone: Option<GitlabMilestone>,
    web_url: String,
    updated_at: String,
    #[serde(default)]
    due_date: Option<String>,
    #[serde(default)]
    weight: Option<u32>,
    #[serde(default)]
    confidential: Option<bool>,
    #[serde(default)]
    discussion_locked: Option<bool>,
    #[serde(default)]
    issue_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabResolvedUser {
    id: u64,
    username: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabMergeRequest {
    iid: u64,
    title: String,
    #[serde(default)]
    description: Option<String>,
    state: String,
    source_branch: String,
    target_branch: String,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    head_pipeline: Option<GitlabPipeline>,
    #[serde(default)]
    rebase_in_progress: Option<bool>,
    #[serde(default)]
    merge_error: Option<String>,
    updated_at: String,
    web_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabPipeline {
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabProject {
    default_branch: String,
}

fn map_issue(issue: GitlabIssue) -> BackendIssueRecord {
    let milestone = issue
        .milestone
        .as_ref()
        .map(|milestone| milestone.title.clone());
    let milestone_id = issue.milestone.as_ref().map(|milestone| milestone.id);
    BackendIssueRecord {
        issue_id: issue.iid,
        node_id: None,
        title: issue.title,
        state: issue.state,
        state_reason: None,
        labels: issue.labels,
        assignees: issue
            .assignees
            .into_iter()
            .map(|user| user.username)
            .collect(),
        milestone,
        milestone_id,
        body: issue.description,
        url: issue.web_url,
        updated_at: normalize_timestamp(&issue.updated_at),
        due_date: issue.due_date,
        weight: issue.weight,
        confidential: issue.confidential,
        discussion_locked: issue.discussion_locked,
        issue_type: issue.issue_type,
        locked: None,
        lock_reason: None,
        comments: Vec::new(),
        linked_mrs: Vec::new(),
        assignee_account_id: None,
        assignee_name: None,
    }
}

fn map_merge_request(merge_request: GitlabMergeRequest) -> BackendPrRecord {
    let merged = merge_request.state == "merged";
    BackendPrRecord {
        number: merge_request.iid,
        title: merge_request.title,
        body: merge_request.description.unwrap_or_default(),
        url: merge_request.web_url,
        state: merge_request.state,
        head: merge_request.source_branch,
        head_sha: merge_request.sha,
        base: merge_request.target_branch,
        node_id: None,
        merged,
        updated_at: normalize_timestamp(&merge_request.updated_at),
    }
}

fn normalize_timestamp(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|_| ts.to_owned())
}

fn parse_naive_date_opt(value: Option<&str>) -> Result<Option<NaiveDate>, RiptaskError> {
    value
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|error| RiptaskError::Config(error.to_string()))
        })
        .transpose()
}

fn parse_gitlab_issue_state_event(
    state: &str,
) -> Result<gitlab::api::projects::issues::IssueStateEvent, RiptaskError> {
    match state {
        "open" | "opened" => Ok(gitlab::api::projects::issues::IssueStateEvent::Reopen),
        "closed" => Ok(gitlab::api::projects::issues::IssueStateEvent::Close),
        other => Err(RiptaskError::Config(format!(
            "unsupported gitlab issue state: {other}"
        ))),
    }
}

fn is_weight_error(error: &RiptaskError) -> bool {
    match error {
        RiptaskError::Unreachable(msg) | RiptaskError::Config(msg) => {
            msg.to_lowercase().contains("weight")
        }
        _ => false,
    }
}
