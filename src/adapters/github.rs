use crate::adapters::backend::{
    BackendIssueRecord, BackendIssueUpsert, BackendProvider, DeleteOutcome,
};
use crate::error::RiptskError;
use async_trait::async_trait;
use octocrab::models;
use octocrab::params::LockReason;

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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        Ok(Self { client })
    }

    fn split_owner_repo<'a>(&self, repo: &'a str) -> Result<(&'a str, &'a str), RiptskError> {
        repo.split_once('/')
            .ok_or_else(|| RiptskError::Config(format!("invalid github repo: {repo}")))
    }
}

#[async_trait]
impl BackendProvider for GithubProvider {
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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        let issues = self
            .client
            .all_pages::<models::issues::Issue>(page)
            .await
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        Ok(issues
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
            .map(map_issue)
            .collect())
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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        Ok(())
    }

    async fn delete_issue(&self, repo: &str, issue_id: u64) -> Result<DeleteOutcome, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let issue = self
            .client
            .issues(owner, repo_name)
            .get(issue_id)
            .await
            .map_err(|e| RiptskError::Unreachable(e.to_string()))?;
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
                eprintln!(
                    "warn: GraphQL deleteIssue failed ({}), falling back to close",
                    e
                );
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
            .map_err(|e| RiptskError::Unreachable(e.to_string()))?;
        Ok(())
    }

    async fn unlock_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .unlock(issue_id)
            .await
            .map_err(|e| RiptskError::Unreachable(e.to_string()))?;
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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        Ok(())
    }

    async fn create_pr(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<String, RiptskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .create(title, head, base)
            .body(body)
            .send()
            .await
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        pull.html_url
            .map(|url| url.to_string())
            .ok_or_else(|| RiptskError::Unreachable(format!("missing PR url for {repo}")))
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
            .map_err(|e| RiptskError::Unreachable(e.to_string()))?;
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
            .map_err(|e| RiptskError::Unreachable(e.to_string()))?;
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
            .map_err(|e| RiptskError::Unreachable(e.to_string()))?;

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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
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
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
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
        updated_at: issue.updated_at.to_string(),
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
