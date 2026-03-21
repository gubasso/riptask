use crate::error::RiptskError;
use async_trait::async_trait;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeleteOutcome {
    #[default]
    HardDeleted,
    SoftClosed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendIssueRecord {
    pub issue_id: u64,
    pub node_id: Option<String>,
    pub title: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub milestone_id: Option<u64>,
    pub body: Option<String>,
    pub url: String,
    pub updated_at: String,
    pub due_date: Option<String>,
    pub weight: Option<u32>,
    pub confidential: Option<bool>,
    pub discussion_locked: Option<bool>,
    pub issue_type: Option<String>,
    pub locked: Option<bool>,
    pub lock_reason: Option<String>,
    pub comments: Vec<String>,
    pub linked_mrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendIssueUpsert {
    pub title: String,
    pub body: String,
    pub state: Option<String>,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub milestone_id: Option<u64>,
    pub due_date: Option<String>,
    pub weight: Option<u32>,
    pub confidential: Option<bool>,
    pub discussion_locked: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct BackendPrRecord {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub state: String,
    pub head: String,
    pub base: String,
    pub updated_at: String,
}

#[async_trait]
pub trait BackendProvider: Send + Sync {
    async fn list_issues(&self, repo: &str) -> Result<Vec<BackendIssueRecord>, RiptskError>;
    async fn create_issue(
        &self,
        repo: &str,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptskError>;
    async fn update_issue(
        &self,
        repo: &str,
        issue_id: u64,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptskError>;
    async fn close_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError>;
    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError>;
    async fn delete_issue(&self, repo: &str, issue_id: u64) -> Result<DeleteOutcome, RiptskError>;
    async fn lock_issue(
        &self,
        repo: &str,
        issue_id: u64,
        reason: Option<&str>,
    ) -> Result<(), RiptskError>;
    async fn unlock_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError>;
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
    ) -> Result<BackendPrRecord, RiptskError>;
    async fn get_pr(&self, repo: &str, number: u64) -> Result<BackendPrRecord, RiptskError>;
    async fn update_pr(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptskError>;
    async fn find_pr_by_branch(
        &self,
        repo: &str,
        head: &str,
        base: &str,
    ) -> Result<Option<BackendPrRecord>, RiptskError>;
    async fn create_branch(
        &self,
        repo: &str,
        branch_name: &str,
        base_ref: &str,
        issue_id: u64,
    ) -> Result<(), RiptskError>;
    async fn default_branch(&self, repo: &str) -> Result<String, RiptskError>;
    async fn delete_branch(&self, repo: &str, branch_name: &str) -> Result<(), RiptskError>;
}
