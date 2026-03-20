use crate::adapters::backend::{BackendIssueRecord, BackendIssueUpsert, BackendProvider};
use crate::error::RiptskError;
use async_trait::async_trait;
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
    pub fn new(host: &str, token: &str) -> Result<Self, RiptskError> {
        Ok(Self {
            host: host.to_owned(),
            token: token.to_owned(),
        })
    }

    async fn client(&self) -> Result<gitlab::AsyncGitlab, RiptskError> {
        let builder = gitlab::Gitlab::builder(&self.host, &self.token);
        builder
            .build_async()
            .await
            .map_err(|error| RiptskError::Unreachable(error.to_string()))
    }
}

#[async_trait]
impl BackendProvider for GitlabProvider {
    async fn list_issues(&self, repo: &str) -> Result<Vec<BackendIssueRecord>, RiptskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::issues::ProjectIssues::builder()
            .project(repo)
            .build()
            .map_err(|error| RiptskError::Config(error.to_string()))?;
        let issues: Vec<GitlabIssue> = gitlab::api::paged(endpoint, gitlab::api::Pagination::All)
            .query_async(&client)
            .await
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        Ok(issues.into_iter().map(map_issue).collect())
    }

    async fn create_issue(
        &self,
        repo: &str,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptskError> {
        let client = self.client().await?;
        let mut builder = gitlab::api::projects::issues::CreateIssue::builder();
        builder.project(repo).title(issue.title.as_str());
        builder.description(issue.body.as_str());
        if !issue.labels.is_empty() {
            builder.labels(issue.labels.iter().cloned());
        }
        let endpoint = builder
            .build()
            .map_err(|error| RiptskError::Config(error.to_string()))?;
        let created: GitlabIssue = endpoint
            .query_async(&client)
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
        let client = self.client().await?;
        let mut builder = gitlab::api::projects::issues::EditIssue::builder();
        builder.project(repo).issue(issue_id);
        builder.title(issue.title.as_str());
        builder.description(issue.body.as_str());
        builder.labels(issue.labels.iter().cloned());
        let endpoint = builder
            .build()
            .map_err(|error| RiptskError::Config(error.to_string()))?;
        let updated: GitlabIssue = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        Ok(map_issue(updated))
    }

    async fn close_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        let client = self.client().await?;
        edit_issue_state(
            &client,
            repo,
            issue_id,
            gitlab::api::projects::issues::IssueStateEvent::Close,
        )
        .await
    }

    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        let client = self.client().await?;
        edit_issue_state(
            &client,
            repo,
            issue_id,
            gitlab::api::projects::issues::IssueStateEvent::Reopen,
        )
        .await
    }

    async fn sync_labels(
        &self,
        repo: &str,
        issue_id: u64,
        labels: &[String],
    ) -> Result<(), RiptskError> {
        let client = self.client().await?;
        let mut builder = gitlab::api::projects::issues::EditIssue::builder();
        builder.project(repo).issue(issue_id);
        builder.labels(labels.iter().cloned());
        let endpoint = builder
            .build()
            .map_err(|error| RiptskError::Config(error.to_string()))?;
        let _: GitlabIssue = endpoint
            .query_async(&client)
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
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::merge_requests::CreateMergeRequest::builder()
            .project(repo)
            .source_branch(head)
            .target_branch(base)
            .title(title)
            .description(body)
            .build()
            .map_err(|error| RiptskError::Config(error.to_string()))?;
        let merge_request: GitlabMergeRequest = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        Ok(merge_request.web_url)
    }

    async fn create_branch(
        &self,
        repo: &str,
        branch_name: &str,
        base_ref: &str,
        _issue_id: u64,
    ) -> Result<(), RiptskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::repository::branches::CreateBranch::builder()
            .project(repo)
            .branch(branch_name)
            .ref_(base_ref)
            .build()
            .map_err(|e| RiptskError::Config(e.to_string()))?;
        let _: serde_json::Value = endpoint
            .query_async(&client)
            .await
            .map_err(|e| RiptskError::Unreachable(e.to_string()))?;
        Ok(())
    }

    async fn default_branch(&self, repo: &str) -> Result<String, RiptskError> {
        let client = self.client().await?;
        let endpoint = gitlab::api::projects::Project::builder()
            .project(repo)
            .build()
            .map_err(|error| RiptskError::Config(error.to_string()))?;
        let project: GitlabProject = endpoint
            .query_async(&client)
            .await
            .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
        Ok(project.default_branch)
    }
}

async fn edit_issue_state(
    client: &gitlab::AsyncGitlab,
    repo: &str,
    issue_id: u64,
    state_event: gitlab::api::projects::issues::IssueStateEvent,
) -> Result<(), RiptskError> {
    let endpoint = gitlab::api::projects::issues::EditIssue::builder()
        .project(repo)
        .issue(issue_id)
        .state_event(state_event)
        .build()
        .map_err(|error| RiptskError::Config(error.to_string()))?;
    let _: GitlabIssue = endpoint
        .query_async(client)
        .await
        .map_err(|error| RiptskError::Unreachable(error.to_string()))?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabUser {
    username: String,
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
    web_url: String,
    updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabMergeRequest {
    web_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitlabProject {
    default_branch: String,
}

fn map_issue(issue: GitlabIssue) -> BackendIssueRecord {
    BackendIssueRecord {
        issue_id: issue.iid,
        title: issue.title,
        state: issue.state,
        labels: issue.labels,
        assignee: issue.assignees.into_iter().next().map(|user| user.username),
        body: issue.description,
        url: issue.web_url,
        updated_at: issue.updated_at,
        comments: Vec::new(),
        linked_mrs: Vec::new(),
    }
}
