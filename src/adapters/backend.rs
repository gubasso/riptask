//! Backend abstraction layer — separates **issue tracking** from **version control**.
//!
//! GitHub and GitLab provide both issue tracking and version control in a single
//! platform, but Jira is issue-only. Rather than stubbing out PR/branch methods
//! with errors, we split the domain into two independent traits:
//!
//! - [`IssueTracker`] — CRUD for issues (GitHub Issues, GitLab Issues, Jira)
//! - [`VersionControl`] — PRs, branches, CI checks (GitHub, GitLab)
//!
//! [`BackendProvider`] is a convenience alias for implementations that do both
//! (GitHub, GitLab). Commands receive only the trait(s) they actually need,
//! enabling natural configurations like "issues in Jira, PRs on GitHub" via the
//! `vc` config field.

use crate::error::RiptaskError;
use async_trait::async_trait;
use serde::Serialize;

/// Outcome of a delete attempt — backends that forbid hard deletion (e.g. Jira
/// when the user lacks delete permissions) may fall back to closing the issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeleteOutcome {
    #[default]
    HardDeleted,
    SoftClosed,
}

/// Canonical representation of a remote issue, used as the exchange format
/// between any backend (GitHub/GitLab/Jira) and the local issue store.
/// Fields that don't apply to a particular backend are set to `None`/empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendIssueRecord {
    pub issue_id: u64,
    /// GitHub-specific GraphQL node ID. `None` for GitLab/Jira.
    pub node_id: Option<String>,
    pub title: String,
    /// `"open"` or `"closed"` — normalized across all backends.
    pub state: String,
    /// Close reason: `"completed"`, `"not_planned"`, `"duplicate"`. Maps to
    /// Jira resolutions ("Done", "Won't Do", "Duplicate") and GitHub state reasons.
    pub state_reason: Option<String>,
    /// Includes `status::` labels for fine-grained state (backlog/todo/in-progress/review).
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    /// Jira: first `fixVersions[].name`. GitHub/GitLab: milestone title.
    pub milestone: Option<String>,
    pub milestone_id: Option<u64>,
    pub body: Option<String>,
    /// Human-facing browse URL (not the API URL).
    pub url: String,
    /// RFC 3339 timestamp of last remote update.
    pub updated_at: String,
    pub due_date: Option<String>,
    /// GitLab issue weight. `None` for GitHub/Jira.
    pub weight: Option<u32>,
    /// Jira: `true` when a security level is set. GitLab: confidential flag.
    pub confidential: Option<bool>,
    pub discussion_locked: Option<bool>,
    /// Jira issue type name ("Task", "Bug", "Story", etc.). `None` for GitHub/GitLab.
    pub issue_type: Option<String>,
    pub locked: Option<bool>,
    pub lock_reason: Option<String>,
    pub comments: Vec<String>,
    pub linked_mrs: Vec<String>,
    /// Jira Cloud `accountId` — stored for push round-trip so we don't need to
    /// resolve displayName → accountId on every update.
    pub assignee_account_id: Option<String>,
    /// Jira Server/DC `name` (username) — stored for push round-trip.
    pub assignee_name: Option<String>,
}

/// Data sent to a backend when creating or updating an issue.
/// State changes are not included here — Jira requires transitions API, and
/// GitHub/GitLab handle state via separate parameters on close/reopen.
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
    pub assignee_account_id: Option<String>,
    pub assignee_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct BackendPrRecord {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub url: String,
    pub state: String,
    pub head: String,
    pub head_sha: Option<String>,
    pub base: String,
    pub node_id: Option<String>,
    pub merged: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum MergeMethod {
    Merge,
    Squash,
    #[default]
    Rebase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrChecksStatus {
    #[default]
    None,
    Pending,
    Passed,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct PrChecksReport {
    pub head_sha: String,
    pub expected: Vec<String>,
    pub registered: Vec<String>,
    pub items: Vec<PrCheckItem>,
    pub status: PrChecksStatus,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PrCheckItem {
    pub name: String,
    pub state: PrChecksStatus,
    pub url: Option<String>,
}

/// Issue tracking operations — implemented by GitHub, GitLab, and Jira.
///
/// The `repo` parameter meaning varies by backend:
/// - GitHub/GitLab: `"owner/repo"` or `"group/project"`
/// - Jira: `"org/PROJECT_KEY"` — org is user-chosen, PROJECT_KEY is the Jira project
#[async_trait]
pub trait IssueTracker: Send + Sync {
    async fn list_issues(&self, repo: &str) -> Result<Vec<BackendIssueRecord>, RiptaskError>;
    async fn get_issue(
        &self,
        repo: &str,
        issue_id: u64,
    ) -> Result<BackendIssueRecord, RiptaskError>;
    async fn create_issue(
        &self,
        repo: &str,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptaskError>;
    async fn update_issue(
        &self,
        repo: &str,
        issue_id: u64,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptaskError>;
    /// Close an issue. `state_reason` maps to Jira resolutions ("completed" → "Done",
    /// "not_planned" → "Won't Do", "duplicate" → "Duplicate") and GitHub close reasons.
    async fn close_issue(
        &self,
        repo: &str,
        issue_id: u64,
        state_reason: Option<&str>,
    ) -> Result<(), RiptaskError>;
    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptaskError>;
    async fn delete_issue(&self, repo: &str, issue_id: u64) -> Result<DeleteOutcome, RiptaskError>;
    async fn lock_issue(
        &self,
        repo: &str,
        issue_id: u64,
        reason: Option<&str>,
    ) -> Result<(), RiptaskError>;
    async fn unlock_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptaskError>;
    async fn sync_labels(
        &self,
        repo: &str,
        issue_id: u64,
        labels: &[String],
    ) -> Result<(), RiptaskError>;
}

/// Version control operations — implemented by GitHub and GitLab only.
/// Jira does not implement this trait; instead, a Jira backend can reference
/// a GitHub/GitLab backend via the `vc` config field for PR/branch operations.
#[async_trait]
pub trait VersionControl: Send + Sync {
    async fn create_pr(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptaskError>;
    async fn get_pr(&self, repo: &str, number: u64) -> Result<BackendPrRecord, RiptaskError>;
    async fn update_pr(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptaskError>;
    async fn find_pr_by_branch(
        &self,
        repo: &str,
        head: &str,
        base: &str,
    ) -> Result<Option<BackendPrRecord>, RiptaskError>;
    async fn merge_pr(
        &self,
        repo: &str,
        number: u64,
        method: MergeMethod,
        commit_title: Option<&str>,
        commit_message: Option<&str>,
    ) -> Result<(), RiptaskError>;
    async fn get_pr_checks_report(
        &self,
        repo: &str,
        number: u64,
    ) -> Result<PrChecksReport, RiptaskError>;
    /// Create a branch. `issue_id` is passed through so GitHub can create a
    /// linked branch (GraphQL `createLinkedBranch`). GitLab may ignore it.
    /// This is data flow from the calling command, not trait coupling.
    async fn create_branch(
        &self,
        repo: &str,
        branch_name: &str,
        base_ref: &str,
        issue_id: Option<u64>,
    ) -> Result<(), RiptaskError>;
    async fn default_branch(&self, repo: &str) -> Result<String, RiptaskError>;
    async fn delete_branch(&self, repo: &str, branch_name: &str) -> Result<(), RiptaskError>;
}

/// Convenience: backends that provide both issue tracking and version control (GitHub, GitLab).
pub trait BackendProvider: IssueTracker + VersionControl {}
impl<T: IssueTracker + VersionControl> BackendProvider for T {}

#[cfg(test)]
mod tests {
    use super::MergeMethod;

    #[test]
    fn default_merge_method_is_rebase() {
        assert_eq!(MergeMethod::default(), MergeMethod::Rebase);
    }
}
