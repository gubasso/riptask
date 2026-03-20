use crate::error::RiptskError;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteIssueRecord {
    pub issue_id: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub body: Option<String>,
    pub url: String,
    pub updated_at: String,
    pub comments: Vec<String>,
    pub linked_mrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteIssueUpsert {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
}

#[async_trait]
pub trait RemoteProvider: Send + Sync {
    async fn list_issues(&self, repo: &str) -> Result<Vec<RemoteIssueRecord>, RiptskError>;
    async fn create_issue(
        &self,
        repo: &str,
        issue: &RemoteIssueUpsert,
    ) -> Result<RemoteIssueRecord, RiptskError>;
    async fn update_issue(
        &self,
        repo: &str,
        issue_id: u64,
        issue: &RemoteIssueUpsert,
    ) -> Result<RemoteIssueRecord, RiptskError>;
    async fn close_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError>;
    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError>;
    async fn sync_labels(
        &self,
        repo: &str,
        issue_id: u64,
        labels: &[String],
    ) -> Result<(), RiptskError>;
    async fn create_pr(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<String, RiptskError>;
    async fn default_branch(&self, repo: &str) -> Result<String, RiptskError>;
}
