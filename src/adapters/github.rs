use crate::adapters::backend::{BackendIssueRecord, BackendIssueUpsert, BackendProvider};
use crate::error::RiptskError;
use async_trait::async_trait;
use octocrab::models;

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
        if let Some(ref assignee) = issue.assignee {
            builder = builder.assignees(vec![assignee.clone()]);
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
}

fn map_issue(issue: models::issues::Issue) -> BackendIssueRecord {
    BackendIssueRecord {
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
