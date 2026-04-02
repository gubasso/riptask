use crate::adapters::backend::{
    BackendIssueRecord, BackendIssueUpsert, DeleteOutcome, IssueTracker,
};
use crate::domain::issue::IssueState;
use crate::error::RiptskError;
use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum JiraAuth {
    /// Jira Cloud: email + API token (Basic auth, base64-encoded `email:token`)
    Basic { email: String, token: String },
    /// Jira Server/DC: Personal Access Token (Bearer auth)
    Pat(String),
}

// ---------------------------------------------------------------------------
// Jira API v2 deserialization structs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JiraIssue {
    pub id: String,
    pub key: String,
    pub fields: JiraFields,
}

#[derive(Debug, Deserialize)]
pub struct JiraFields {
    pub summary: String,
    pub description: Option<String>,
    pub status: JiraStatus,
    pub resolution: Option<JiraResolution>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub assignee: Option<JiraUser>,
    #[serde(rename = "fixVersions", default)]
    pub fix_versions: Vec<JiraVersion>,
    #[serde(rename = "issuetype")]
    pub issue_type: JiraIssueType,
    pub priority: Option<JiraPriority>,
    pub duedate: Option<String>,
    pub security: Option<JiraSecurityLevel>,
    pub comment: Option<JiraCommentPage>,
    pub updated: String,
    pub created: String,
}

#[derive(Debug, Deserialize)]
pub struct JiraStatus {
    pub name: String,
    #[serde(rename = "statusCategory")]
    pub status_category: JiraStatusCategory,
}

#[derive(Debug, Deserialize)]
pub struct JiraStatusCategory {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct JiraResolution {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct JiraUser {
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "accountId")]
    pub account_id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct JiraVersion {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct JiraIssueType {
    pub name: String,
    #[serde(default)]
    pub subtask: bool,
}

#[derive(Debug, Deserialize)]
pub struct JiraPriority {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct JiraSecurityLevel {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct JiraCommentPage {
    #[serde(default)]
    pub comments: Vec<JiraComment>,
}

#[derive(Debug, Deserialize)]
pub struct JiraComment {
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct JiraTransition {
    pub id: String,
    pub name: String,
    pub to: JiraTransitionTarget,
    #[serde(rename = "hasScreen", default)]
    pub has_screen: bool,
    #[serde(rename = "isAvailable", default = "default_true")]
    pub is_available: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct JiraTransitionTarget {
    pub name: String,
    #[serde(rename = "statusCategory")]
    pub status_category: JiraStatusCategory,
}

#[derive(Debug, Deserialize)]
pub struct JiraTransitionsResponse {
    pub transitions: Vec<JiraTransition>,
}

#[derive(Debug, Deserialize)]
pub struct JiraSearchResult {
    pub issues: Vec<JiraIssue>,
    pub total: u64,
}

#[derive(Debug, Deserialize)]
pub struct JiraCreateResponse {
    pub id: String,
    pub key: String,
    #[serde(rename = "self")]
    pub self_url: String,
}

// ---------------------------------------------------------------------------
// State mapping functions
// ---------------------------------------------------------------------------

/// Map Jira status category key to BackendIssueRecord.state ("open" / "closed").
pub fn jira_status_to_backend_state(status_category_key: &str) -> &'static str {
    match status_category_key {
        "done" => "closed",
        _ => "open", // "new", "indeterminate", "undefined"
    }
}

/// Map Jira status category key + status name to a `status::` label.
pub fn jira_status_to_label(status_category_key: &str, status_name: &str) -> String {
    let lower = status_name.to_lowercase();
    match status_category_key {
        "new" => {
            if lower.contains("backlog") {
                "status::backlog".into()
            } else {
                "status::todo".into()
            }
        }
        "indeterminate" => {
            if lower.contains("review") || lower.contains("qa") {
                "status::review".into()
            } else {
                "status::in-progress".into()
            }
        }
        "done" => "status::done".into(),
        // "undefined" — treat as backlog
        _ => "status::backlog".into(),
    }
}

/// Map Jira resolution name to riptsk state_reason (pull direction).
pub fn jira_resolution_to_state_reason(resolution: Option<&str>) -> Option<String> {
    resolution.map(|name| match name {
        "Won't Do" | "Wont Do" => "not_planned".into(),
        "Duplicate" => "duplicate".into(),
        _ => "completed".into(), // "Done", "Fixed", "Cannot Reproduce", etc.
    })
}

/// Map riptsk state_reason to Jira resolution name (push direction).
pub fn state_reason_to_jira_resolution(reason: Option<&str>) -> &'static str {
    match reason {
        Some("not_planned") => "Won't Do",
        Some("duplicate") => "Duplicate",
        _ => "Done",
    }
}

/// Find the best transition for a target IssueState.
pub fn find_transition<'a>(
    transitions: &'a [JiraTransition],
    target: &IssueState,
) -> Result<&'a JiraTransition, RiptskError> {
    let target_category = match target {
        IssueState::Backlog | IssueState::Todo => "new",
        IssueState::InProgress | IssueState::Review => "indeterminate",
        IssueState::Done => "done",
    };

    let candidates: Vec<&JiraTransition> = transitions
        .iter()
        .filter(|t| t.to.status_category.key == target_category && t.is_available)
        .collect();

    if candidates.is_empty() {
        return Err(RiptskError::General(format!(
            "no available transition to {target_category} category"
        )));
    }

    // Prefer exact name match
    if let Some(exact) = candidates
        .iter()
        .find(|t| name_matches_state(&t.to.name, target))
    {
        return Ok(exact);
    }

    Ok(candidates[0])
}

fn name_matches_state(status_name: &str, target: &IssueState) -> bool {
    let lower = status_name.to_lowercase();
    match target {
        IssueState::Backlog => lower.contains("backlog"),
        IssueState::Todo => lower.contains("to do") || lower.contains("open"),
        IssueState::InProgress => lower.contains("progress"),
        IssueState::Review => lower.contains("review"),
        IssueState::Done => true, // Any done-category status is fine
    }
}

// ---------------------------------------------------------------------------
// Timestamp normalization
// ---------------------------------------------------------------------------

/// Convert Jira's timestamp format to RFC 3339.
/// Jira: "2024-03-28T09:15:42.123+0000" → RFC 3339: "2024-03-28T09:15:42.123+00:00"
pub fn normalize_jira_timestamp(jira_ts: &str) -> String {
    match chrono::DateTime::parse_from_str(jira_ts, "%Y-%m-%dT%H:%M:%S%.3f%z") {
        Ok(dt) => dt.to_rfc3339(),
        Err(_) => jira_ts.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Field mapping: Jira → BackendIssueRecord
// ---------------------------------------------------------------------------

pub fn jira_issue_to_record(issue: &JiraIssue, host: &str) -> BackendIssueRecord {
    let category_key = &issue.fields.status.status_category.key;
    let status_name = &issue.fields.status.name;
    let state = jira_status_to_backend_state(category_key).to_owned();
    let status_label = jira_status_to_label(category_key, status_name);
    let state_reason =
        jira_resolution_to_state_reason(issue.fields.resolution.as_ref().map(|r| r.name.as_str()));

    let mut labels = issue.fields.labels.clone();
    // Remove existing status:: labels before adding the computed one
    labels.retain(|l| !l.starts_with("status::"));
    labels.push(status_label);

    let assignees = issue
        .fields
        .assignee
        .as_ref()
        .map(|u| vec![u.display_name.clone()])
        .unwrap_or_default();

    let milestone = issue.fields.fix_versions.first().map(|v| v.name.clone());
    let milestone_id = issue
        .fields
        .fix_versions
        .first()
        .and_then(|v| v.id.parse::<u64>().ok());

    let url = format!("{}/browse/{}", host.trim_end_matches('/'), issue.key);

    let comments = issue
        .fields
        .comment
        .as_ref()
        .map(|page| page.comments.iter().map(|c| c.body.clone()).collect())
        .unwrap_or_default();

    let confidential = issue.fields.security.as_ref().map(|_| true);

    BackendIssueRecord {
        issue_id: issue.id.parse::<u64>().unwrap_or(0),
        node_id: None,
        title: issue.fields.summary.clone(),
        state,
        state_reason,
        labels,
        assignees,
        milestone,
        milestone_id,
        body: issue.fields.description.clone(),
        url,
        updated_at: normalize_jira_timestamp(&issue.fields.updated),
        due_date: issue.fields.duedate.clone(),
        weight: None,
        confidential,
        discussion_locked: None,
        issue_type: Some(issue.fields.issue_type.name.clone()),
        locked: None,
        lock_reason: None,
        comments,
        linked_mrs: vec![],
    }
}

// ---------------------------------------------------------------------------
// Field mapping: BackendIssueUpsert → Jira JSON payloads
// ---------------------------------------------------------------------------

/// Build the JSON payload for POST /rest/api/2/issue (create).
pub fn upsert_to_jira_create(
    upsert: &BackendIssueUpsert,
    project_key: &str,
    issue_type: &str,
) -> serde_json::Value {
    let mut fields = serde_json::json!({
        "project": { "key": project_key },
        "summary": upsert.title,
        "issuetype": { "name": issue_type },
    });

    let map = fields.as_object_mut().unwrap();
    if !upsert.body.is_empty() {
        map.insert(
            "description".into(),
            serde_json::Value::String(upsert.body.clone()),
        );
    }
    if !upsert.labels.is_empty() {
        map.insert("labels".into(), serde_json::json!(upsert.labels));
    }
    if let Some(milestone_id) = upsert.milestone_id {
        map.insert(
            "fixVersions".into(),
            serde_json::json!([{ "id": milestone_id.to_string() }]),
        );
    }
    if let Some(ref due) = upsert.due_date {
        map.insert("duedate".into(), serde_json::Value::String(due.clone()));
    }

    serde_json::json!({ "fields": fields })
}

/// Build the JSON payload for PUT /rest/api/2/issue/{id} (update).
pub fn upsert_to_jira_update(upsert: &BackendIssueUpsert) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    fields.insert(
        "summary".into(),
        serde_json::Value::String(upsert.title.clone()),
    );
    fields.insert(
        "description".into(),
        if upsert.body.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(upsert.body.clone())
        },
    );
    fields.insert("labels".into(), serde_json::json!(upsert.labels));
    if let Some(milestone_id) = upsert.milestone_id {
        fields.insert(
            "fixVersions".into(),
            serde_json::json!([{ "id": milestone_id.to_string() }]),
        );
    } else {
        fields.insert("fixVersions".into(), serde_json::json!([]));
    }
    if let Some(ref due) = upsert.due_date {
        fields.insert("duedate".into(), serde_json::Value::String(due.clone()));
    } else {
        fields.insert("duedate".into(), serde_json::Value::Null);
    }

    serde_json::json!({ "fields": fields })
}

// ---------------------------------------------------------------------------
// JiraProvider — HTTP client wrapper
// ---------------------------------------------------------------------------

pub struct JiraProvider {
    client: reqwest::Client,
    host: String,
    auth_header: String,
}

impl JiraProvider {
    pub fn new(host: &str, auth: JiraAuth) -> Result<Self, RiptskError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let auth_header = match auth {
            JiraAuth::Basic {
                ref email,
                ref token,
            } => {
                use base64::Engine;
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(format!("{email}:{token}"));
                format!("Basic {encoded}")
            }
            JiraAuth::Pat(ref token) => format!("Bearer {token}"),
        };
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| RiptskError::General(format!("failed to build HTTP client: {e}")))?;
        Ok(Self {
            client,
            host: host.trim_end_matches('/').to_owned(),
            auth_header,
        })
    }

    fn api_url(&self, path: &str) -> String {
        format!("{}/rest/api/2{}", self.host, path)
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response, RiptskError> {
        self.client
            .get(self.api_url(path))
            .header(AUTHORIZATION, &self.auth_header)
            .header(CONTENT_TYPE, "application/json")
            .send()
            .await
            .map_err(|e| RiptskError::Unreachable(format!("Jira API error: {e}")))
    }

    async fn post(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, RiptskError> {
        self.client
            .post(self.api_url(path))
            .header(AUTHORIZATION, &self.auth_header)
            .header(CONTENT_TYPE, "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| RiptskError::Unreachable(format!("Jira API error: {e}")))
    }

    async fn put(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, RiptskError> {
        self.client
            .put(self.api_url(path))
            .header(AUTHORIZATION, &self.auth_header)
            .header(CONTENT_TYPE, "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| RiptskError::Unreachable(format!("Jira API error: {e}")))
    }

    async fn delete_request(&self, path: &str) -> Result<reqwest::Response, RiptskError> {
        self.client
            .delete(self.api_url(path))
            .header(AUTHORIZATION, &self.auth_header)
            .send()
            .await
            .map_err(|e| RiptskError::Unreachable(format!("Jira API error: {e}")))
    }

    async fn check_response(response: reqwest::Response) -> Result<reqwest::Response, RiptskError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let url = response.url().to_string();
        let body = response.text().await.unwrap_or_default();
        Err(RiptskError::Unreachable(format!(
            "Jira API returned {status} for {url}: {body}"
        )))
    }

    /// Extract the project key from the `repo` field. Format: `org/PROJECT_KEY`.
    pub fn project_key(repo: &str) -> &str {
        repo.rsplit('/').next().unwrap_or(repo)
    }

    async fn search_issues(&self, jql: &str) -> Result<Vec<JiraIssue>, RiptskError> {
        let mut all_issues = Vec::new();
        let mut start_at = 0u64;
        let max_results = 100u64;

        loop {
            let path = format!(
                "/search?jql={}&startAt={}&maxResults={}&fields=summary,description,status,resolution,labels,assignee,fixVersions,issuetype,priority,duedate,security,comment,updated,created",
                urlencoding::encode(jql),
                start_at,
                max_results
            );
            let response = Self::check_response(self.get(&path).await?).await?;
            let result: JiraSearchResult = response
                .json()
                .await
                .map_err(|e| RiptskError::General(format!("failed to parse Jira search: {e}")))?;
            let count = result.issues.len() as u64;
            all_issues.extend(result.issues);
            if start_at + count >= result.total {
                break;
            }
            start_at += count;
        }

        Ok(all_issues)
    }

    async fn get_transitions(&self, issue_key: &str) -> Result<Vec<JiraTransition>, RiptskError> {
        let path = format!("/issue/{issue_key}/transitions");
        let response = Self::check_response(self.get(&path).await?).await?;
        let result: JiraTransitionsResponse = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse transitions: {e}")))?;
        Ok(result.transitions)
    }

    async fn transition_issue(
        &self,
        issue_key: &str,
        transition_id: &str,
        fields: Option<serde_json::Value>,
    ) -> Result<(), RiptskError> {
        let mut body = serde_json::json!({
            "transition": { "id": transition_id }
        });
        if let Some(f) = fields {
            body.as_object_mut().unwrap().insert("fields".into(), f);
        }
        let path = format!("/issue/{issue_key}/transitions");
        Self::check_response(self.post(&path, &body).await?).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// IssueTracker implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl IssueTracker for JiraProvider {
    async fn list_issues(&self, repo: &str) -> Result<Vec<BackendIssueRecord>, RiptskError> {
        let project_key = Self::project_key(repo);
        let jql = format!("project = {project_key} ORDER BY updated DESC");
        let issues = self.search_issues(&jql).await?;
        Ok(issues
            .iter()
            .map(|issue| jira_issue_to_record(issue, &self.host))
            .collect())
    }

    async fn get_issue(
        &self,
        _repo: &str,
        issue_id: u64,
    ) -> Result<BackendIssueRecord, RiptskError> {
        let path = format!(
            "/issue/{}?fields=summary,description,status,resolution,labels,assignee,fixVersions,issuetype,priority,duedate,security,comment,updated,created",
            issue_id
        );
        let response = Self::check_response(self.get(&path).await?).await?;
        let issue: JiraIssue = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse Jira issue: {e}")))?;
        Ok(jira_issue_to_record(&issue, &self.host))
    }

    async fn create_issue(
        &self,
        repo: &str,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptskError> {
        let project_key = Self::project_key(repo);
        let payload = upsert_to_jira_create(issue, project_key, "Task");
        let response = Self::check_response(self.post("/issue", &payload).await?).await?;
        let created: JiraCreateResponse = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse create response: {e}")))?;

        // Fetch the full issue to return a complete record
        let issue_id: u64 = created.id.parse().unwrap_or(0);
        self.get_issue(repo, issue_id).await
    }

    async fn update_issue(
        &self,
        _repo: &str,
        issue_id: u64,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptskError> {
        let payload = upsert_to_jira_update(issue);
        let path = format!("/issue/{issue_id}");
        Self::check_response(self.put(&path, &payload).await?).await?;

        // Fetch updated issue
        self.get_issue(_repo, issue_id).await
    }

    async fn close_issue(&self, _repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        // First get the issue key (needed for transitions API)
        let path = format!("/issue/{issue_id}?fields=status");
        let response = Self::check_response(self.get(&path).await?).await?;
        let issue: JiraIssue = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse Jira issue: {e}")))?;

        let transitions = self.get_transitions(&issue.key).await?;
        let transition = find_transition(&transitions, &IssueState::Done)?;

        // Include resolution when transitioning to done
        let fields = serde_json::json!({
            "resolution": { "name": "Done" }
        });
        self.transition_issue(&issue.key, &transition.id, Some(fields))
            .await
    }

    async fn reopen_issue(&self, _repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        let path = format!("/issue/{issue_id}?fields=status");
        let response = Self::check_response(self.get(&path).await?).await?;
        let issue: JiraIssue = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse Jira issue: {e}")))?;

        let transitions = self.get_transitions(&issue.key).await?;
        let transition = find_transition(&transitions, &IssueState::Todo)?;
        self.transition_issue(&issue.key, &transition.id, None)
            .await?;

        // Clear resolution after reopening
        let clear_body = serde_json::json!({ "fields": { "resolution": null } });
        let update_path = format!("/issue/{}", issue.key);
        // Best-effort — some workflows auto-clear resolution
        let _ = self.put(&update_path, &clear_body).await;
        Ok(())
    }

    async fn delete_issue(&self, _repo: &str, issue_id: u64) -> Result<DeleteOutcome, RiptskError> {
        let path = format!("/issue/{issue_id}");
        match self.delete_request(&path).await {
            Ok(response) if response.status().is_success() => Ok(DeleteOutcome::HardDeleted),
            Ok(response) if response.status().as_u16() == 403 => {
                // Permission denied — fall back to close
                self.close_issue(_repo, issue_id).await?;
                Ok(DeleteOutcome::SoftClosed)
            }
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                Err(RiptskError::Unreachable(format!(
                    "Jira delete failed ({status}): {body}"
                )))
            }
            Err(e) => Err(e),
        }
    }

    async fn lock_issue(
        &self,
        _repo: &str,
        _issue_id: u64,
        _reason: Option<&str>,
    ) -> Result<(), RiptskError> {
        // Jira has no lock concept — no-op
        Ok(())
    }

    async fn unlock_issue(&self, _repo: &str, _issue_id: u64) -> Result<(), RiptskError> {
        // Jira has no lock concept — no-op
        Ok(())
    }

    async fn sync_labels(
        &self,
        _repo: &str,
        issue_id: u64,
        labels: &[String],
    ) -> Result<(), RiptskError> {
        let payload = serde_json::json!({
            "fields": { "labels": labels }
        });
        let path = format!("/issue/{issue_id}");
        Self::check_response(self.put(&path, &payload).await?).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jira_status_category_mapping() {
        assert_eq!(jira_status_to_backend_state("new"), "open");
        assert_eq!(jira_status_to_backend_state("indeterminate"), "open");
        assert_eq!(jira_status_to_backend_state("done"), "closed");
        assert_eq!(jira_status_to_backend_state("undefined"), "open");
    }

    #[test]
    fn jira_status_label_mapping() {
        assert_eq!(jira_status_to_label("new", "Backlog"), "status::backlog");
        assert_eq!(jira_status_to_label("new", "To Do"), "status::todo");
        assert_eq!(jira_status_to_label("new", "Open"), "status::todo");
        assert_eq!(
            jira_status_to_label("indeterminate", "In Progress"),
            "status::in-progress"
        );
        assert_eq!(
            jira_status_to_label("indeterminate", "In Review"),
            "status::review"
        );
        assert_eq!(
            jira_status_to_label("indeterminate", "Code Review"),
            "status::review"
        );
        assert_eq!(
            jira_status_to_label("indeterminate", "In QA"),
            "status::review"
        );
        assert_eq!(jira_status_to_label("done", "Done"), "status::done");
        assert_eq!(jira_status_to_label("done", "Closed"), "status::done");
        assert_eq!(
            jira_status_to_label("undefined", "Unknown"),
            "status::backlog"
        );
    }

    #[test]
    fn resolution_mapping() {
        assert_eq!(
            jira_resolution_to_state_reason(Some("Done")),
            Some("completed".into())
        );
        assert_eq!(
            jira_resolution_to_state_reason(Some("Won't Do")),
            Some("not_planned".into())
        );
        assert_eq!(
            jira_resolution_to_state_reason(Some("Duplicate")),
            Some("duplicate".into())
        );
        assert_eq!(
            jira_resolution_to_state_reason(Some("Cannot Reproduce")),
            Some("completed".into())
        );
        assert_eq!(jira_resolution_to_state_reason(None), None);
    }

    #[test]
    fn state_reason_to_resolution_mapping() {
        assert_eq!(
            state_reason_to_jira_resolution(Some("not_planned")),
            "Won't Do"
        );
        assert_eq!(
            state_reason_to_jira_resolution(Some("duplicate")),
            "Duplicate"
        );
        assert_eq!(state_reason_to_jira_resolution(Some("completed")), "Done");
        assert_eq!(state_reason_to_jira_resolution(None), "Done");
    }

    #[test]
    fn find_transition_prefers_exact_name_match() {
        let transitions = vec![
            JiraTransition {
                id: "1".into(),
                name: "Start Progress".into(),
                to: JiraTransitionTarget {
                    name: "In Development".into(),
                    status_category: JiraStatusCategory {
                        key: "indeterminate".into(),
                        name: "In Progress".into(),
                    },
                },
                has_screen: false,
                is_available: true,
            },
            JiraTransition {
                id: "2".into(),
                name: "Submit for Review".into(),
                to: JiraTransitionTarget {
                    name: "In Review".into(),
                    status_category: JiraStatusCategory {
                        key: "indeterminate".into(),
                        name: "In Progress".into(),
                    },
                },
                has_screen: false,
                is_available: true,
            },
        ];

        let result = find_transition(&transitions, &IssueState::Review).unwrap();
        assert_eq!(result.id, "2");
    }

    #[test]
    fn find_transition_falls_back_to_first_in_category() {
        let transitions = vec![JiraTransition {
            id: "31".into(),
            name: "Close".into(),
            to: JiraTransitionTarget {
                name: "Closed".into(),
                status_category: JiraStatusCategory {
                    key: "done".into(),
                    name: "Done".into(),
                },
            },
            has_screen: false,
            is_available: true,
        }];

        let result = find_transition(&transitions, &IssueState::Done).unwrap();
        assert_eq!(result.id, "31");
    }

    #[test]
    fn find_transition_errors_when_no_match() {
        let transitions = vec![JiraTransition {
            id: "1".into(),
            name: "Start".into(),
            to: JiraTransitionTarget {
                name: "In Progress".into(),
                status_category: JiraStatusCategory {
                    key: "indeterminate".into(),
                    name: "In Progress".into(),
                },
            },
            has_screen: false,
            is_available: true,
        }];

        let result = find_transition(&transitions, &IssueState::Done);
        assert!(result.is_err());
    }

    #[test]
    fn normalize_jira_timestamp_converts_format() {
        let jira_ts = "2024-03-28T09:15:42.123+0000";
        let result = normalize_jira_timestamp(jira_ts);
        assert!(result.contains("+00:00"), "got: {result}");
    }

    #[test]
    fn normalize_jira_timestamp_preserves_valid_rfc3339() {
        let ts = "2024-03-28T09:15:42+00:00";
        let result = normalize_jira_timestamp(ts);
        assert!(result.contains("+00:00"));
    }

    #[test]
    fn project_key_extraction() {
        assert_eq!(JiraProvider::project_key("myteam/PROJ"), "PROJ");
        assert_eq!(JiraProvider::project_key("PROJ"), "PROJ");
        assert_eq!(JiraProvider::project_key("org/sub/PROJ"), "PROJ");
    }

    #[test]
    fn jira_issue_to_record_basic() {
        let issue = JiraIssue {
            id: "10042".into(),
            key: "PROJ-123".into(),
            fields: JiraFields {
                summary: "Fix the bug".into(),
                description: Some("A detailed description".into()),
                status: JiraStatus {
                    name: "In Progress".into(),
                    status_category: JiraStatusCategory {
                        key: "indeterminate".into(),
                        name: "In Progress".into(),
                    },
                },
                resolution: None,
                labels: vec!["bug".into()],
                assignee: Some(JiraUser {
                    display_name: "Alice".into(),
                    account_id: Some("abc123".into()),
                    name: None,
                }),
                fix_versions: vec![JiraVersion {
                    id: "10001".into(),
                    name: "1.0".into(),
                }],
                issue_type: JiraIssueType {
                    name: "Bug".into(),
                    subtask: false,
                },
                priority: Some(JiraPriority {
                    name: "High".into(),
                }),
                duedate: Some("2024-04-01".into()),
                security: None,
                comment: Some(JiraCommentPage {
                    comments: vec![JiraComment {
                        body: "Working on it".into(),
                    }],
                }),
                updated: "2024-03-28T09:15:42.123+0000".into(),
                created: "2024-03-20T10:00:00.000+0000".into(),
            },
        };

        let record = jira_issue_to_record(&issue, "https://myteam.atlassian.net");

        assert_eq!(record.issue_id, 10042);
        assert_eq!(record.title, "Fix the bug");
        assert_eq!(record.state, "open");
        assert_eq!(record.state_reason, None);
        assert!(record.labels.contains(&"bug".to_string()));
        assert!(record.labels.contains(&"status::in-progress".to_string()));
        assert_eq!(record.assignees, vec!["Alice"]);
        assert_eq!(record.milestone, Some("1.0".into()));
        assert_eq!(record.milestone_id, Some(10001));
        assert_eq!(record.body, Some("A detailed description".into()));
        assert_eq!(record.url, "https://myteam.atlassian.net/browse/PROJ-123");
        assert_eq!(record.due_date, Some("2024-04-01".into()));
        assert_eq!(record.issue_type, Some("Bug".into()));
        assert_eq!(record.comments, vec!["Working on it"]);
        assert_eq!(record.confidential, None);
    }

    #[test]
    fn jira_issue_to_record_closed_with_resolution() {
        let issue = JiraIssue {
            id: "10043".into(),
            key: "PROJ-124".into(),
            fields: JiraFields {
                summary: "Old issue".into(),
                description: None,
                status: JiraStatus {
                    name: "Done".into(),
                    status_category: JiraStatusCategory {
                        key: "done".into(),
                        name: "Done".into(),
                    },
                },
                resolution: Some(JiraResolution {
                    name: "Won't Do".into(),
                }),
                labels: vec![],
                assignee: None,
                fix_versions: vec![],
                issue_type: JiraIssueType {
                    name: "Task".into(),
                    subtask: false,
                },
                priority: None,
                duedate: None,
                security: None,
                comment: None,
                updated: "2024-03-28T09:15:42.123+0000".into(),
                created: "2024-03-20T10:00:00.000+0000".into(),
            },
        };

        let record = jira_issue_to_record(&issue, "https://example.atlassian.net");

        assert_eq!(record.state, "closed");
        assert_eq!(record.state_reason, Some("not_planned".into()));
        assert!(record.assignees.is_empty());
        assert_eq!(record.milestone, None);
    }

    #[test]
    fn upsert_to_jira_create_generates_valid_json() {
        let upsert = BackendIssueUpsert {
            title: "New task".into(),
            body: "Description here".into(),
            state: Some("open".into()),
            state_reason: None,
            labels: vec!["feature".into(), "status::todo".into()],
            assignees: vec![],
            milestone_id: Some(10001),
            due_date: Some("2024-04-01".into()),
            weight: None,
            confidential: None,
            discussion_locked: None,
        };

        let json = upsert_to_jira_create(&upsert, "PROJ", "Task");

        assert_eq!(json["fields"]["project"]["key"], "PROJ");
        assert_eq!(json["fields"]["summary"], "New task");
        assert_eq!(json["fields"]["issuetype"]["name"], "Task");
        assert_eq!(json["fields"]["description"], "Description here");
        assert_eq!(json["fields"]["duedate"], "2024-04-01");
    }

    #[test]
    fn upsert_to_jira_update_generates_valid_json() {
        let upsert = BackendIssueUpsert {
            title: "Updated title".into(),
            body: String::new(),
            state: Some("open".into()),
            state_reason: None,
            labels: vec!["status::in-progress".into()],
            assignees: vec![],
            milestone_id: None,
            due_date: None,
            weight: None,
            confidential: None,
            discussion_locked: None,
        };

        let json = upsert_to_jira_update(&upsert);

        assert_eq!(json["fields"]["summary"], "Updated title");
        assert!(json["fields"]["description"].is_null());
        assert!(json["fields"]["fixVersions"].as_array().unwrap().is_empty());
        assert!(json["fields"]["duedate"].is_null());
    }
}
