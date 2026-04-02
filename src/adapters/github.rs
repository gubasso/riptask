use crate::adapters::backend::{
    BackendIssueRecord, BackendIssueUpsert, BackendPrRecord, CiPresence, DeleteOutcome,
    IssueTracker, MergeMethod, PrChecksStatus, VersionControl,
};
use crate::error::RiptskError;
use async_trait::async_trait;
use chrono::SecondsFormat;
use octocrab::models;
use octocrab::params::LockReason;

/// Format an octocrab error with full detail.
///
/// `octocrab::Error`'s `Display` for the `GitHub` variant just prints
/// "GitHub" (snafu default), losing the status code and message. This
/// helper extracts the inner `GitHubError` which has a proper `Display`.
fn format_octocrab_error(error: &octocrab::Error) -> String {
    match error {
        octocrab::Error::GitHub { source, .. } => {
            format!("{source} (HTTP {})", source.status_code.as_u16())
        }
        other => other.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct GithubProvider {
    pub client: octocrab::Octocrab,
}

impl GithubProvider {
    pub fn new(token: &str) -> Result<Self, RiptskError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let client = octocrab::Octocrab::builder()
            .personal_token(token.to_owned())
            .build()
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(Self { client })
    }

    fn split_owner_repo<'a>(&self, repo: &'a str) -> Result<(&'a str, &'a str), RiptskError> {
        repo.split_once('/')
            .ok_or_else(|| RiptskError::Config(format!("invalid github repo: {repo}")))
    }
}

#[async_trait]
impl IssueTracker for GithubProvider {
    async fn list_issues(&self, repo: &str) -> Result<Vec<BackendIssueRecord>, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let page = self
            .client
            .issues(owner, repo_name)
            .list()
            .state(octocrab::params::State::All)
            .per_page(100)
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        let issues = self
            .client
            .all_pages::<models::issues::Issue>(page)
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(issues
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
            .map(map_issue)
            .collect())
    }

    async fn get_issue(
        &self,
        repo: &str,
        issue_id: u64,
    ) -> Result<BackendIssueRecord, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let issue = self
            .client
            .issues(owner, repo_name)
            .get(issue_id)
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(map_issue(issue))
    }

    async fn create_issue(
        &self,
        repo: &str,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let handler = self.client.issues(owner, repo_name);
        let mut builder = handler
            .create(&issue.title)
            .body(&issue.body)
            .labels(issue.labels.clone());
        if !issue.assignees.is_empty() {
            builder = builder.assignees(issue.assignees.clone());
        }
        if let Some(milestone_id) = issue.milestone_id {
            builder = builder.milestone(milestone_id);
        }
        let created = builder
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(map_issue(created))
    }

    async fn update_issue(
        &self,
        repo: &str,
        issue_id: u64,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let handler = self.client.issues(owner, repo_name);
        let mut builder = handler
            .update(issue_id)
            .title(&issue.title)
            .body(&issue.body)
            .labels(&issue.labels)
            .assignees(&issue.assignees);
        if let Some(milestone_id) = issue.milestone_id {
            builder = builder.milestone(milestone_id);
        }
        if let Some(state) = issue.state.as_deref() {
            builder = builder.state(parse_github_issue_state(state)?);
        }
        if let Some(state_reason) = issue.state_reason.as_deref() {
            builder = builder.state_reason(parse_github_state_reason(state_reason)?);
        }
        let updated = builder
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(map_issue(updated))
    }

    async fn close_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .update(issue_id)
            .state(models::IssueState::Closed)
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(())
    }

    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .update(issue_id)
            .state(models::IssueState::Open)
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(())
    }

    async fn delete_issue(&self, repo: &str, issue_id: u64) -> Result<DeleteOutcome, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let issue = self
            .client
            .issues(owner, repo_name)
            .get(issue_id)
            .await
            .map_err(|e| RiptskError::Unreachable(format_octocrab_error(&e)))?;
        let node_id = issue.node_id;

        let query = r#"mutation($issueId: ID!) {
            deleteIssue(input: {issueId: $issueId}) {
                clientMutationId
            }
        }"#;
        let payload = serde_json::json!({
            "query": query,
            "variables": { "issueId": node_id }
        });
        match self.client.graphql::<serde_json::Value>(&payload).await {
            Ok(response) => {
                if let Some(errors) = response.get("errors") {
                    let msg = errors.to_string();
                    let lower = msg.to_lowercase();
                    if lower.contains("forbidden")
                        || lower.contains("insufficient")
                        || lower.contains("not allowed")
                        || msg.contains("403")
                    {
                        self.close_issue(repo, issue_id).await?;
                        return Ok(DeleteOutcome::SoftClosed);
                    }
                    return Err(RiptskError::Unreachable(format!(
                        "deleteIssue failed: {msg}"
                    )));
                }
                Ok(DeleteOutcome::HardDeleted)
            }
            Err(e) => {
                crate::ui::warn(&format!(
                    "GraphQL deleteIssue failed ({e}), falling back to close"
                ));
                self.close_issue(repo, issue_id).await?;
                Ok(DeleteOutcome::SoftClosed)
            }
        }
    }

    async fn lock_issue(
        &self,
        repo: &str,
        issue_id: u64,
        reason: Option<&str>,
    ) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let lock_reason = reason.and_then(parse_lock_reason);
        self.client
            .issues(owner, repo_name)
            .lock(issue_id, lock_reason)
            .await
            .map_err(|e| RiptskError::Unreachable(format_octocrab_error(&e)))?;
        Ok(())
    }

    async fn unlock_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .unlock(issue_id)
            .await
            .map_err(|e| RiptskError::Unreachable(format_octocrab_error(&e)))?;
        Ok(())
    }

    async fn sync_labels(
        &self,
        repo: &str,
        issue_id: u64,
        labels: &[String],
    ) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .replace_all_labels(issue_id, labels)
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(())
    }
}

#[async_trait]
impl VersionControl for GithubProvider {
    async fn create_pr(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .create(title, head, base)
            .body(body)
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        map_pull_request(pull, repo)
    }

    async fn get_pr(&self, repo: &str, number: u64) -> Result<BackendPrRecord, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .get(number)
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        map_pull_request(pull, repo)
    }

    async fn update_pr(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .update(number)
            .title(title)
            .body(body)
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        map_pull_request(pull, repo)
    }

    async fn find_pr_by_branch(
        &self,
        repo: &str,
        head: &str,
        base: &str,
    ) -> Result<Option<BackendPrRecord>, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let page = self
            .client
            .pulls(owner, repo_name)
            .list()
            .head(format!("{owner}:{head}"))
            .base(base)
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        let Some(pull) = page.items.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(map_pull_request(pull, repo)?))
    }

    async fn merge_pr(
        &self,
        repo: &str,
        number: u64,
        method: MergeMethod,
        commit_title: Option<&str>,
        commit_message: Option<&str>,
    ) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pulls = self.client.pulls(owner, repo_name);
        let mut builder = pulls.merge(number).method(match method {
            MergeMethod::Merge => octocrab::params::pulls::MergeMethod::Merge,
            MergeMethod::Squash => octocrab::params::pulls::MergeMethod::Squash,
            MergeMethod::Rebase => octocrab::params::pulls::MergeMethod::Rebase,
        });
        if let Some(title) = commit_title {
            builder = builder.title(title);
        }
        if let Some(message) = commit_message {
            builder = builder.message(message);
        }
        builder.send().await.map_err(|error| {
            RiptskError::General(format!("failed to merge PR #{number}: {error}"))
        })?;
        Ok(())
    }

    async fn get_pr_checks_status(
        &self,
        repo: &str,
        number: u64,
    ) -> Result<PrChecksStatus, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pr = self.get_pr(repo, number).await?;
        let head_sha = pr
            .head_sha
            .ok_or_else(|| RiptskError::General("PR head SHA not available".into()))?;

        // Check commit statuses (legacy integrations)
        let status_result = self
            .client
            .get::<octocrab::models::CombinedStatus, _, _>(
                format!("/repos/{owner}/{repo_name}/commits/{head_sha}/status"),
                None::<&()>,
            )
            .await
            .ok();

        // Check runs (GitHub Actions, third-party apps)
        let check_runs_result = self
            .client
            .checks(owner.to_string(), repo_name.to_string())
            .list_check_runs_for_git_ref(octocrab::params::repos::Commitish(head_sha))
            .send()
            .await
            .ok();

        // Evaluate check runs
        let checks_status = check_runs_result.and_then(|page| {
            let runs = page.check_runs;
            if runs.is_empty() {
                return None;
            }
            let any_in_progress = runs.iter().any(|r| r.completed_at.is_none());
            if any_in_progress {
                return Some(PrChecksStatus::Pending);
            }
            // All completed — only success/skipped/neutral count as passed
            let all_passed = runs.iter().all(|r| {
                matches!(
                    r.conclusion.as_deref(),
                    Some("success" | "skipped" | "neutral")
                )
            });
            if all_passed {
                Some(PrChecksStatus::Passed)
            } else {
                Some(PrChecksStatus::Failed)
            }
        });

        // Evaluate legacy commit statuses.
        // GitHub returns state:"pending" with an empty statuses array when no
        // legacy statuses exist. Treat that as None, not Pending, so it does
        // not override a valid Passed/Failed from check runs.
        let status_state = status_result.and_then(|combined| {
            if combined.statuses.is_empty() {
                return None;
            }
            Some(match combined.state {
                octocrab::models::StatusState::Success => PrChecksStatus::Passed,
                octocrab::models::StatusState::Failure | octocrab::models::StatusState::Error => {
                    PrChecksStatus::Failed
                }
                octocrab::models::StatusState::Pending => PrChecksStatus::Pending,
                _ => PrChecksStatus::None,
            })
        });

        // Merge both signals: worst status wins
        match (checks_status, status_state) {
            (Some(PrChecksStatus::Failed), _) | (_, Some(PrChecksStatus::Failed)) => {
                Ok(PrChecksStatus::Failed)
            }
            (Some(PrChecksStatus::Pending), _) | (_, Some(PrChecksStatus::Pending)) => {
                Ok(PrChecksStatus::Pending)
            }
            (Some(PrChecksStatus::Passed), _) | (_, Some(PrChecksStatus::Passed)) => {
                Ok(PrChecksStatus::Passed)
            }
            _ => Ok(PrChecksStatus::None),
        }
    }

    async fn get_ci_presence(&self, repo: &str) -> Result<CiPresence, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;

        // Try GitHub Actions workflows API. This may fail with 403 if the token
        // lacks Actions:read permission — that's fine, we fall back below.
        let mut names: Vec<String> = Vec::new();
        if let Ok(response) = self
            .client
            .get::<serde_json::Value, _, _>(
                format!("/repos/{owner}/{repo_name}/actions/workflows"),
                None::<&()>,
            )
            .await
            && let Some(arr) = response.get("workflows").and_then(|w| w.as_array())
        {
            names = arr
                .iter()
                .filter_map(|workflow| {
                    workflow
                        .get("path")
                        .and_then(|path| path.as_str())
                        .map(|path| {
                            std::path::Path::new(path)
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string()
                        })
                })
                .collect();
        }

        if !names.is_empty() {
            return Ok(CiPresence {
                has_remote_ci: true,
                remote_workflow_names: names,
            });
        }

        // Fallback: check if the repo has any check runs OR legacy commit
        // statuses on the default branch. This covers third-party CI (Jenkins,
        // CircleCI, etc.) and tokens that lack Actions:read permission.
        let has_third_party_ci =
            if let Ok(repo_info) = self.client.repos(owner, repo_name).get().await {
                let default_branch = repo_info.default_branch.unwrap_or_default();
                if !default_branch.is_empty() {
                    let has_check_runs = self
                        .client
                        .checks(owner.to_string(), repo_name.to_string())
                        .list_check_runs_for_git_ref(octocrab::params::repos::Commitish(
                            default_branch.clone(),
                        ))
                        .send()
                        .await
                        .ok()
                        .is_some_and(|page| !page.check_runs.is_empty());

                    let encoded_ref = default_branch.replace('/', "%2F");
                    let has_commit_statuses = self
                        .client
                        .get::<octocrab::models::CombinedStatus, _, _>(
                            format!("/repos/{owner}/{repo_name}/commits/{encoded_ref}/status"),
                            None::<&()>,
                        )
                        .await
                        .ok()
                        .is_some_and(|combined| !combined.statuses.is_empty());

                    has_check_runs || has_commit_statuses
                } else {
                    false
                }
            } else {
                false
            };

        Ok(CiPresence {
            has_remote_ci: has_third_party_ci,
            remote_workflow_names: Vec::new(),
        })
    }

    async fn create_branch(
        &self,
        repo: &str,
        branch_name: &str,
        base_ref: &str,
        issue_id: u64,
    ) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;

        // Resolve base branch to SHA
        let base = self
            .client
            .repos(owner, repo_name)
            .get_ref(&octocrab::params::repos::Reference::Branch(
                base_ref.to_owned(),
            ))
            .await
            .map_err(|e| RiptskError::Unreachable(format_octocrab_error(&e)))?;
        let sha = match base.object {
            octocrab::models::repos::Object::Commit { sha, .. }
            | octocrab::models::repos::Object::Tag { sha, .. } => sha,
            _ => {
                return Err(RiptskError::Unreachable(format!(
                    "unsupported git ref object for base branch {base_ref}"
                )));
            }
        };

        // Get issue node_id for GraphQL linking
        let issue = self
            .client
            .issues(owner, repo_name)
            .get(issue_id)
            .await
            .map_err(|e| RiptskError::Unreachable(format_octocrab_error(&e)))?;
        let issue_node_id = issue.node_id;

        // Use createLinkedBranch GraphQL mutation to create branch linked to issue
        let query = r#"mutation($issueId: ID!, $oid: GitObjectID!, $name: String) {
            createLinkedBranch(input: {issueId: $issueId, oid: $oid, name: $name}) {
                linkedBranch { ref { name } }
            }
        }"#;
        let payload = serde_json::json!({
            "query": query,
            "variables": {
                "issueId": issue_node_id,
                "oid": sha,
                "name": branch_name,
            }
        });
        let response: serde_json::Value = self
            .client
            .graphql(&payload)
            .await
            .map_err(|e| RiptskError::Unreachable(format_octocrab_error(&e)))?;

        if let Some(errors) = response.get("errors") {
            return Err(RiptskError::Unreachable(format!(
                "GitHub createLinkedBranch failed: {errors}"
            )));
        }

        Ok(())
    }

    async fn default_branch(&self, repo: &str) -> Result<String, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let repository = self
            .client
            .repos(owner, repo_name)
            .get()
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        repository
            .default_branch
            .ok_or_else(|| RiptskError::Unreachable(format!("missing default branch for {repo}")))
    }

    async fn delete_branch(&self, repo: &str, branch_name: &str) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .repos(owner, repo_name)
            .delete_ref(&octocrab::params::repos::Reference::Branch(
                branch_name.to_owned(),
            ))
            .await
            .map_err(|error| RiptskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(())
    }
}

fn map_issue(issue: models::issues::Issue) -> BackendIssueRecord {
    let milestone = issue
        .milestone
        .as_ref()
        .map(|milestone| milestone.title.clone());
    let milestone_id = issue
        .milestone
        .as_ref()
        .and_then(|milestone| u64::try_from(milestone.number).ok());
    BackendIssueRecord {
        issue_id: issue.number,
        node_id: Some(issue.node_id),
        title: issue.title,
        state: match issue.state {
            models::IssueState::Open => "open".into(),
            models::IssueState::Closed => "closed".into(),
            _ => "open".into(),
        },
        state_reason: issue.state_reason.map(github_state_reason_to_string),
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        assignees: issue
            .assignees
            .into_iter()
            .map(|assignee| assignee.login)
            .collect(),
        milestone,
        milestone_id,
        body: issue.body,
        url: issue.html_url.to_string(),
        updated_at: issue.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        due_date: None,
        weight: None,
        confidential: None,
        discussion_locked: None,
        issue_type: None,
        locked: Some(issue.locked),
        lock_reason: issue.active_lock_reason,
        comments: Vec::new(),
        linked_mrs: Vec::new(),
    }
}

fn map_pull_request(
    pull: models::pulls::PullRequest,
    repo: &str,
) -> Result<BackendPrRecord, RiptskError> {
    let url = pull
        .html_url
        .map(|url| url.to_string())
        .ok_or_else(|| RiptskError::Unreachable(format!("missing PR url for {repo}")))?;
    let state = if pull.merged_at.is_some() {
        "merged".into()
    } else {
        match pull.state {
            Some(models::IssueState::Open) => "open".into(),
            Some(models::IssueState::Closed) => "closed".into(),
            _ => "open".into(),
        }
    };
    Ok(BackendPrRecord {
        number: pull.number,
        title: pull.title.unwrap_or_default(),
        body: pull.body.unwrap_or_default(),
        url,
        state,
        head: pull.head.ref_field,
        head_sha: Some(pull.head.sha),
        base: pull.base.ref_field,
        node_id: pull.node_id,
        merged: pull.merged.unwrap_or(false) || pull.merged_at.is_some(),
        updated_at: pull
            .updated_at
            .map(|updated_at| updated_at.to_rfc3339_opts(SecondsFormat::Secs, true))
            .unwrap_or_default(),
    })
}

fn github_state_reason_to_string(reason: models::issues::IssueStateReason) -> String {
    match reason {
        models::issues::IssueStateReason::Completed => "completed".into(),
        models::issues::IssueStateReason::NotPlanned => "not_planned".into(),
        models::issues::IssueStateReason::Reopened => "reopened".into(),
        models::issues::IssueStateReason::Duplicate => "duplicate".into(),
        _ => "completed".into(),
    }
}

fn parse_github_issue_state(state: &str) -> Result<models::IssueState, RiptskError> {
    match state {
        "open" => Ok(models::IssueState::Open),
        "closed" => Ok(models::IssueState::Closed),
        other => Err(RiptskError::Config(format!(
            "unsupported github issue state: {other}"
        ))),
    }
}

fn parse_github_state_reason(
    state_reason: &str,
) -> Result<models::issues::IssueStateReason, RiptskError> {
    match state_reason {
        "completed" => Ok(models::issues::IssueStateReason::Completed),
        "not_planned" => Ok(models::issues::IssueStateReason::NotPlanned),
        "reopened" => Ok(models::issues::IssueStateReason::Reopened),
        "duplicate" => Ok(models::issues::IssueStateReason::Duplicate),
        other => Err(RiptskError::Config(format!(
            "unsupported github issue state reason: {other}"
        ))),
    }
}

fn parse_lock_reason(reason: &str) -> Option<LockReason> {
    match reason {
        "off-topic" => Some(LockReason::OffTopic),
        "too heated" => Some(LockReason::TooHeated),
        "resolved" => Some(LockReason::Resolved),
        "spam" => Some(LockReason::Spam),
        _ => None,
    }
}
