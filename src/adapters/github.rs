use crate::adapters::remote::{RemoteIssueRecord, RemoteIssueUpsert, RemoteProvider};
use crate::error::TskError;
use async_trait::async_trait;
use octocrab::models;

#[derive(Debug, Clone)]
pub struct GithubProvider {
    pub client: octocrab::Octocrab,
}

impl GithubProvider {
    pub fn new(token: Option<&str>) -> Result<Self, TskError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let mut builder = octocrab::Octocrab::builder();
        if let Some(token) = token {
            builder = builder.personal_token(token.to_owned());
        }
        let client = builder
            .build()
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        Ok(Self { client })
    }

    fn split_owner_repo<'a>(&self, repo: &'a str) -> Result<(&'a str, &'a str), TskError> {
        repo.split_once('/')
            .ok_or_else(|| TskError::Config(format!("invalid github repo: {repo}")))
    }
}

#[async_trait]
impl RemoteProvider for GithubProvider {
    async fn list_issues(&self, repo: &str) -> Result<Vec<RemoteIssueRecord>, TskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let page = self
            .client
            .issues(owner, repo_name)
            .list()
            .state(octocrab::params::State::All)
            .per_page(100)
            .send()
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        let issues = self
            .client
            .all_pages::<models::issues::Issue>(page)
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        Ok(issues
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
            .map(map_issue)
            .collect())
    }

    async fn create_issue(
        &self,
        repo: &str,
        issue: &RemoteIssueUpsert,
    ) -> Result<RemoteIssueRecord, TskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let handler = self.client.issues(owner, repo_name);
        let mut builder = handler
            .create(&issue.title)
            .body(&issue.body)
            .labels(issue.labels.clone());
        if let Some(ref assignee) = issue.assignee {
            builder = builder.assignees(vec![assignee.clone()]);
        }
        let created = builder
            .send()
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        Ok(map_issue(created))
    }

    async fn update_issue(
        &self,
        repo: &str,
        issue_id: u64,
        issue: &RemoteIssueUpsert,
    ) -> Result<RemoteIssueRecord, TskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let handler = self.client.issues(owner, repo_name);
        let assignees = issue
            .assignee
            .as_ref()
            .map(|a| vec![a.clone()])
            .unwrap_or_default();
        let builder = handler
            .update(issue_id)
            .title(&issue.title)
            .body(&issue.body)
            .labels(&issue.labels)
            .assignees(&assignees);
        let updated = builder
            .send()
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        Ok(map_issue(updated))
    }

    async fn close_issue(&self, repo: &str, issue_id: u64) -> Result<(), TskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .update(issue_id)
            .state(models::IssueState::Closed)
            .send()
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        Ok(())
    }

    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), TskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .update(issue_id)
            .state(models::IssueState::Open)
            .send()
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        Ok(())
    }

    async fn sync_labels(
        &self,
        repo: &str,
        issue_id: u64,
        labels: &[String],
    ) -> Result<(), TskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .replace_all_labels(issue_id, labels)
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        Ok(())
    }

    async fn create_pr(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<String, TskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .create(title, head, base)
            .body(body)
            .send()
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        pull.html_url
            .map(|url| url.to_string())
            .ok_or_else(|| TskError::Unreachable(format!("missing PR url for {repo}")))
    }

    async fn default_branch(&self, repo: &str) -> Result<String, TskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let repository = self
            .client
            .repos(owner, repo_name)
            .get()
            .await
            .map_err(|error| TskError::Unreachable(error.to_string()))?;
        repository
            .default_branch
            .ok_or_else(|| TskError::Unreachable(format!("missing default branch for {repo}")))
    }
}

fn map_issue(issue: models::issues::Issue) -> RemoteIssueRecord {
    RemoteIssueRecord {
        issue_id: issue.number,
        title: issue.title,
        state: match issue.state {
            models::IssueState::Open => "open".into(),
            models::IssueState::Closed => "closed".into(),
            _ => "open".into(),
        },
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        assignee: issue.assignee.map(|assignee| assignee.login),
        body: issue.body,
        url: issue.html_url.to_string(),
        updated_at: issue.updated_at.to_string(),
        comments: Vec::new(),
        linked_mrs: Vec::new(),
    }
}
