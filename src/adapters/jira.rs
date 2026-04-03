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

/// Lightweight struct for API responses where only `id` and `key` are needed
/// (e.g., when fetching with `?fields=status` which omits most fields).
#[derive(Debug, Deserialize)]
struct JiraIssueKey {
    pub id: String,
    pub key: String,
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
    /// Transition-screen fields (present when fetched with `expand=transitions.fields`).
    #[serde(default)]
    pub fields: Option<serde_json::Value>,
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

/// Response from the newer `/search/jql` endpoint (Jira Cloud).
#[derive(Debug, Deserialize)]
struct JiraSearchJqlResult {
    pub issues: Vec<JiraIssue>,
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
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
///
/// When transition fields metadata is available (via `expand=transitions.fields`),
/// transitions that require unsupported screen fields (other than `resolution`)
/// are excluded from candidates.
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

    // Filter out transitions that require unsupported screen fields
    let supported: Vec<&JiraTransition> = candidates
        .iter()
        .filter(|t| !has_unsupported_required_fields(t))
        .copied()
        .collect();

    let pool = if supported.is_empty() {
        // All transitions require unsupported fields — warn but try the first candidate anyway
        &candidates
    } else {
        &supported
    };

    // Prefer exact name match
    if let Some(exact) = pool.iter().find(|t| name_matches_state(&t.to.name, target)) {
        return Ok(exact);
    }

    Ok(pool[0])
}

/// Check whether a transition has required screen fields we don't support.
/// We support `resolution` (set during close); anything else is unsupported.
fn has_unsupported_required_fields(transition: &JiraTransition) -> bool {
    let Some(ref fields_val) = transition.fields else {
        return false;
    };
    let Some(fields_obj) = fields_val.as_object() else {
        return false;
    };
    for (key, schema) in fields_obj {
        if key == "resolution" {
            continue;
        }
        let required = schema
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if required {
            return true;
        }
    }
    false
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

    let (assignees, assignee_account_id, assignee_name) = match &issue.fields.assignee {
        Some(u) => (
            vec![u.display_name.clone()],
            u.account_id.clone(),
            u.name.clone(),
        ),
        None => (Vec::new(), None, None),
    };

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
        assignee_account_id,
        assignee_name,
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
    /// Configured default issue type for creation (from `BackendConfig.default_issue_type`).
    configured_issue_type: Option<String>,
}

impl std::fmt::Debug for JiraProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraProvider")
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

impl JiraProvider {
    pub fn new(host: &str, auth: JiraAuth) -> Result<Self, RiptskError> {
        Self::with_issue_type(host, auth, None)
    }

    pub fn with_issue_type(
        host: &str,
        auth: JiraAuth,
        configured_issue_type: Option<String>,
    ) -> Result<Self, RiptskError> {
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
            configured_issue_type,
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

    const SEARCH_FIELDS: &str = "summary,description,status,resolution,labels,assignee,fixVersions,issuetype,priority,duedate,security,comment,updated,created";
    const MAX_SEARCH_PAGES: usize = 100;

    async fn search_issues(&self, jql: &str) -> Result<Vec<JiraIssue>, RiptskError> {
        // Try the new /search/jql endpoint first (Jira Cloud, required since Oct 2025).
        // Falls back to the legacy /search endpoint for Server/DC.
        match self.search_issues_jql(jql).await {
            Ok(issues) => Ok(issues),
            Err(ref e) if e.to_string().contains("404") => {
                // /search/jql not available (Server/DC) — use legacy endpoint
                self.search_issues_legacy(jql).await
            }
            Err(e) => Err(e),
        }
    }

    /// Jira Cloud: `/rest/api/2/search/jql` with nextPageToken cursor pagination.
    async fn search_issues_jql(&self, jql: &str) -> Result<Vec<JiraIssue>, RiptskError> {
        let mut all_issues = Vec::new();
        let mut next_page_token: Option<String> = None;
        let max_results = 100u64;

        for _ in 0..Self::MAX_SEARCH_PAGES {
            let mut path = format!(
                "/search/jql?jql={}&maxResults={}&fields={}",
                urlencoding::encode(jql),
                max_results,
                Self::SEARCH_FIELDS,
            );
            if let Some(ref token) = next_page_token {
                path.push_str(&format!("&nextPageToken={}", urlencoding::encode(token)));
            }
            let response = Self::check_response(self.get(&path).await?).await?;
            let result: JiraSearchJqlResult = response.json().await.map_err(|e| {
                RiptskError::General(format!("failed to parse Jira search/jql: {e}"))
            })?;
            all_issues.extend(result.issues);
            match result.next_page_token {
                Some(token) => next_page_token = Some(token),
                None => break,
            }
        }

        Ok(all_issues)
    }

    /// Jira Server/DC: legacy `/rest/api/2/search` with startAt/maxResults pagination.
    async fn search_issues_legacy(&self, jql: &str) -> Result<Vec<JiraIssue>, RiptskError> {
        let mut all_issues = Vec::new();
        let mut start_at = 0u64;
        let max_results = 100u64;

        for _ in 0..Self::MAX_SEARCH_PAGES {
            let path = format!(
                "/search?jql={}&startAt={}&maxResults={}&fields={}",
                urlencoding::encode(jql),
                start_at,
                max_results,
                Self::SEARCH_FIELDS,
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
        let path = format!("/issue/{issue_key}/transitions?expand=transitions.fields");
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
        let response = self.post(&path, &body).await?;
        if response.status().as_u16() == 409 {
            // 409 Conflict — retry once (transition race)
            Self::check_response(self.post(&path, &body).await?).await?;
        } else {
            Self::check_response(response).await?;
        }
        Ok(())
    }

    /// Search for users assignable to issues in a project.
    pub async fn search_assignable_users(
        &self,
        project_key: &str,
        query: &str,
    ) -> Result<Vec<JiraUser>, RiptskError> {
        let path = format!(
            "/user/assignable/search?project={}&query={}",
            urlencoding::encode(project_key),
            urlencoding::encode(query)
        );
        let response = Self::check_response(self.get(&path).await?).await?;
        response
            .json::<Vec<JiraUser>>()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse assignable users: {e}")))
    }

    /// Fetch valid issue types for a project from Jira's create metadata.
    /// Returns the list of non-subtask issue type names.
    pub async fn fetch_create_issue_types(
        &self,
        project_key: &str,
    ) -> Result<Vec<String>, RiptskError> {
        let path = format!(
            "/issue/createmeta?projectKeys={}&expand=projects.issuetypes",
            urlencoding::encode(project_key)
        );
        let response = Self::check_response(self.get(&path).await?).await?;
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse create metadata: {e}")))?;
        let mut types = Vec::new();
        if let Some(projects) = body.get("projects").and_then(|v| v.as_array()) {
            for project in projects {
                if let Some(issue_types) = project.get("issuetypes").and_then(|v| v.as_array()) {
                    for it in issue_types {
                        let is_subtask =
                            it.get("subtask").and_then(|v| v.as_bool()).unwrap_or(false);
                        if !is_subtask && let Some(name) = it.get("name").and_then(|v| v.as_str()) {
                            types.push(name.to_owned());
                        }
                    }
                }
            }
        }
        Ok(types)
    }

    /// Resolve the issue type to use for creation.
    /// Priority: explicit config > metadata discovery > "Task" fallback.
    pub async fn resolve_issue_type(
        &self,
        project_key: &str,
        configured_type: Option<&str>,
    ) -> Result<String, RiptskError> {
        if let Some(t) = configured_type {
            return Ok(t.to_owned());
        }
        let types = self.fetch_create_issue_types(project_key).await?;
        if types.is_empty() {
            return Err(RiptskError::General(format!(
                "no creatable issue types found for project {project_key}; \
                 set 'default_issue_type' in the backend config"
            )));
        }
        // Prefer "Task" if present, otherwise use the first available type
        if types.iter().any(|t| t == "Task") {
            return Ok("Task".into());
        }
        Ok(types.into_iter().next().unwrap())
    }

    /// Set the assignee on a Jira issue using the dedicated assignee endpoint.
    /// Uses `accountId` for Cloud or `name` for Server/DC.
    async fn set_assignee(
        &self,
        issue_key: &str,
        project_key: &str,
        upsert: &BackendIssueUpsert,
    ) -> Result<(), RiptskError> {
        let path = format!("/issue/{issue_key}/assignee");

        let assignee_display = match upsert.assignees.first() {
            Some(name) => name,
            None => {
                // Clear assignee: PUT with accountId: null (Cloud) or name: null (Server/DC)
                let payload = serde_json::json!({ "accountId": null });
                let _ = self.put(&path, &payload).await;
                return Ok(());
            }
        };

        // Prefer stored account ID from round-trip metadata (Jira Cloud)
        if let Some(ref account_id) = upsert.assignee_account_id {
            let payload = serde_json::json!({ "accountId": account_id });
            Self::check_response(self.put(&path, &payload).await?).await?;
            return Ok(());
        }

        // Prefer stored username from round-trip metadata (Jira Server/DC)
        if let Some(ref name) = upsert.assignee_name {
            let payload = serde_json::json!({ "name": name });
            Self::check_response(self.put(&path, &payload).await?).await?;
            return Ok(());
        }

        // Fall back to searching for the user
        let users = self
            .search_assignable_users(project_key, assignee_display)
            .await?;
        if let Some(user) = users.first() {
            let payload = if let Some(ref account_id) = user.account_id {
                serde_json::json!({ "accountId": account_id })
            } else if let Some(ref name) = user.name {
                serde_json::json!({ "name": name })
            } else {
                return Err(RiptskError::General(format!(
                    "assignable user '{}' has no accountId or name",
                    user.display_name
                )));
            };
            Self::check_response(self.put(&path, &payload).await?).await?;
        }
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
        let issue_type = self
            .resolve_issue_type(project_key, self.configured_issue_type.as_deref())
            .await?;
        let payload = upsert_to_jira_create(issue, project_key, &issue_type);
        let response = Self::check_response(self.post("/issue", &payload).await?).await?;
        let created: JiraCreateResponse = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse create response: {e}")))?;

        // Fetch the full issue to return a complete record
        let issue_id: u64 = created.id.parse().unwrap_or(0);

        // Set assignee separately (requires issue key from the created response)
        if !issue.assignees.is_empty() {
            self.set_assignee(&created.key, project_key, issue).await?;
        }

        self.get_issue(repo, issue_id).await
    }

    async fn update_issue(
        &self,
        repo: &str,
        issue_id: u64,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptskError> {
        let payload = upsert_to_jira_update(issue);
        let path = format!("/issue/{issue_id}");
        Self::check_response(self.put(&path, &payload).await?).await?;

        // Set assignee separately (need issue key, fetch it)
        let issue_path = format!("/issue/{issue_id}?fields=status");
        let response = Self::check_response(self.get(&issue_path).await?).await?;
        let jira_key: JiraIssueKey = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse Jira issue key: {e}")))?;
        let project_key = Self::project_key(repo);
        self.set_assignee(&jira_key.key, project_key, issue).await?;

        // Fetch updated issue
        self.get_issue(repo, issue_id).await
    }

    async fn close_issue(
        &self,
        _repo: &str,
        issue_id: u64,
        state_reason: Option<&str>,
    ) -> Result<(), RiptskError> {
        // First get the issue key (needed for transitions API)
        let path = format!("/issue/{issue_id}?fields=status");
        let response = Self::check_response(self.get(&path).await?).await?;
        let issue: JiraIssueKey = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse Jira issue key: {e}")))?;

        let transitions = self.get_transitions(&issue.key).await?;
        let transition = find_transition(&transitions, &IssueState::Done)?;

        // Map state_reason to the appropriate Jira resolution
        let resolution = state_reason_to_jira_resolution(state_reason);
        let fields = serde_json::json!({
            "resolution": { "name": resolution }
        });
        self.transition_issue(&issue.key, &transition.id, Some(fields))
            .await
    }

    async fn reopen_issue(&self, _repo: &str, issue_id: u64) -> Result<(), RiptskError> {
        let path = format!("/issue/{issue_id}?fields=status");
        let response = Self::check_response(self.get(&path).await?).await?;
        let issue: JiraIssueKey = response
            .json()
            .await
            .map_err(|e| RiptskError::General(format!("failed to parse Jira issue key: {e}")))?;

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
                self.close_issue(_repo, issue_id, None).await?;
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
                fields: None,
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
                fields: None,
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
            fields: None,
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
            fields: None,
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
        assert_eq!(record.assignee_account_id, Some("abc123".into()));
        assert_eq!(record.assignee_name, None);
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
            assignee_account_id: None,
            assignee_name: None,
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
            assignee_account_id: None,
            assignee_name: None,
        };

        let json = upsert_to_jira_update(&upsert);

        assert_eq!(json["fields"]["summary"], "Updated title");
        assert!(json["fields"]["description"].is_null());
        assert!(json["fields"]["fixVersions"].as_array().unwrap().is_empty());
        assert!(json["fields"]["duedate"].is_null());
    }

    // -----------------------------------------------------------------------
    // Wiremock integration tests
    // -----------------------------------------------------------------------

    fn jira_issue_json(id: &str, key: &str, summary: &str, status_key: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "key": key,
            "fields": {
                "summary": summary,
                "description": "body",
                "status": {
                    "name": "To Do",
                    "statusCategory": { "key": status_key, "name": "To Do" }
                },
                "resolution": null,
                "labels": [],
                "assignee": {
                    "displayName": "Alice Smith",
                    "accountId": "abc-123",
                    "name": null
                },
                "fixVersions": [],
                "issuetype": { "name": "Task", "subtask": false },
                "priority": { "name": "Medium" },
                "duedate": null,
                "security": null,
                "comment": { "comments": [] },
                "updated": "2024-03-28T09:15:42.123+0000",
                "created": "2024-03-20T10:00:00.000+0000"
            }
        })
    }

    fn mock_provider(host: &str) -> JiraProvider {
        JiraProvider::new(
            host,
            JiraAuth::Basic {
                email: "test@test.com".into(),
                token: "token".into(),
            },
        )
        .unwrap()
    }

    /// Mount a createmeta mock that returns "Task" as available issue type for PROJ.
    async fn mount_createmeta_mock(server: &wiremock::MockServer) {
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                "/rest/api/2/issue/createmeta.*",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "projects": [{
                        "key": "PROJ",
                        "issuetypes": [
                            { "name": "Task", "subtask": false },
                            { "name": "Sub-task", "subtask": true }
                        ]
                    }]
                })),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn wiremock_list_issues_cloud() {
        let server = wiremock::MockServer::start().await;
        // Mock the new /search/jql endpoint (Jira Cloud)
        let search_body = serde_json::json!({
            "issues": [
                jira_issue_json("10001", "PROJ-1", "First issue", "new"),
                jira_issue_json("10002", "PROJ-2", "Second issue", "done"),
            ],
            "nextPageToken": null
        });
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/search/jql.*"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(&search_body))
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let records = provider.list_issues("myteam/PROJ").await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].title, "First issue");
        assert_eq!(records[0].state, "open");
        assert_eq!(records[1].title, "Second issue");
        assert_eq!(records[1].state, "closed");
    }

    #[tokio::test]
    async fn wiremock_list_issues_server_dc_fallback() {
        let server = wiremock::MockServer::start().await;
        // /search/jql returns 404 (Server/DC doesn't have it)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/search/jql.*"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;
        // Legacy /search endpoint works
        let search_body = serde_json::json!({
            "issues": [jira_issue_json("10001", "PROJ-1", "Server issue", "new")],
            "total": 1
        });
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/rest/api/2/search"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(&search_body))
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let records = provider.list_issues("myteam/PROJ").await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].title, "Server issue");
    }

    #[tokio::test]
    async fn wiremock_list_issues_cloud_pagination() {
        let server = wiremock::MockServer::start().await;
        // Page 1: returns a nextPageToken
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/search/jql.*"))
            .and(wiremock::matchers::query_param_is_missing("nextPageToken"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "issues": [jira_issue_json("10001", "PROJ-1", "Page 1 issue", "new")],
                    "nextPageToken": "page2token"
                })),
            )
            .mount(&server)
            .await;
        // Page 2: no nextPageToken (end of results)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/search/jql.*"))
            .and(wiremock::matchers::query_param(
                "nextPageToken",
                "page2token",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "issues": [jira_issue_json("10002", "PROJ-2", "Page 2 issue", "done")],
                    "nextPageToken": null
                })),
            )
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let records = provider.list_issues("myteam/PROJ").await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].title, "Page 1 issue");
        assert_eq!(records[1].title, "Page 2 issue");
    }

    #[tokio::test]
    async fn wiremock_create_issue() {
        let server = wiremock::MockServer::start().await;
        mount_createmeta_mock(&server).await;

        // Mock POST /issue
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/rest/api/2/issue"))
            .respond_with(
                wiremock::ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "id": "10050",
                    "key": "PROJ-50",
                    "self": "https://example.atlassian.net/rest/api/2/issue/10050"
                })),
            )
            .mount(&server)
            .await;

        // Mock GET /issue/10050 (fetch after create)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10050.*"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(jira_issue_json("10050", "PROJ-50", "New task", "new")),
            )
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let upsert = BackendIssueUpsert {
            title: "New task".into(),
            body: "desc".into(),
            state: Some("open".into()),
            ..Default::default()
        };
        let record = provider.create_issue("myteam/PROJ", &upsert).await.unwrap();
        assert_eq!(record.issue_id, 10050);
        assert_eq!(record.title, "New task");
    }

    #[tokio::test]
    async fn wiremock_update_issue() {
        let server = wiremock::MockServer::start().await;

        // Mock PUT /issue/10001
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/rest/api/2/issue/10001"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        // Mock GET /issue/10001 (fetch after update + assignee fetch)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10001.*"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(jira_issue_json(
                    "10001",
                    "PROJ-1",
                    "Updated title",
                    "new",
                )),
            )
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let upsert = BackendIssueUpsert {
            title: "Updated title".into(),
            body: String::new(),
            state: Some("open".into()),
            ..Default::default()
        };
        let record = provider
            .update_issue("myteam/PROJ", 10001, &upsert)
            .await
            .unwrap();
        assert_eq!(record.title, "Updated title");
    }

    #[tokio::test]
    async fn wiremock_close_issue_done_resolution() {
        let server = wiremock::MockServer::start().await;

        // Mock GET /issue/10001 (fetch status)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10001.*"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "10001",
                "key": "PROJ-1",
                "fields": {
                    "summary": "task", "description": null,
                    "status": { "name": "To Do", "statusCategory": { "key": "new", "name": "To Do" } },
                    "resolution": null, "labels": [], "assignee": null,
                    "fixVersions": [], "issuetype": { "name": "Task", "subtask": false },
                    "priority": null, "duedate": null, "security": null,
                    "comment": { "comments": [] },
                    "updated": "2024-03-28T09:15:42.123+0000",
                    "created": "2024-03-20T10:00:00.000+0000"
                }
            })))
            .mount(&server)
            .await;

        // Mock GET transitions
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/rest/api/2/issue/PROJ-1/transitions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "transitions": [{
                    "id": "31",
                    "name": "Done",
                    "to": { "name": "Done", "statusCategory": { "key": "done", "name": "Done" } },
                    "hasScreen": false,
                    "isAvailable": true
                }]
            })))
            .mount(&server)
            .await;

        // Mock POST transition — capture the request body to verify resolution
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(
                "/rest/api/2/issue/PROJ-1/transitions",
            ))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "transition": { "id": "31" },
                "fields": { "resolution": { "name": "Done" } }
            })))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        provider
            .close_issue("myteam/PROJ", 10001, Some("completed"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn wiremock_close_issue_wont_do_resolution() {
        let server = wiremock::MockServer::start().await;

        // Mock GET /issue/10001
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10001.*"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "10001", "key": "PROJ-1",
                "fields": {
                    "summary": "task", "description": null,
                    "status": { "name": "To Do", "statusCategory": { "key": "new", "name": "To Do" } },
                    "resolution": null, "labels": [], "assignee": null,
                    "fixVersions": [], "issuetype": { "name": "Task", "subtask": false },
                    "priority": null, "duedate": null, "security": null,
                    "comment": { "comments": [] },
                    "updated": "2024-03-28T09:15:42.123+0000",
                    "created": "2024-03-20T10:00:00.000+0000"
                }
            })))
            .mount(&server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/rest/api/2/issue/PROJ-1/transitions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "transitions": [{
                    "id": "31", "name": "Done",
                    "to": { "name": "Done", "statusCategory": { "key": "done", "name": "Done" } },
                    "hasScreen": false, "isAvailable": true
                }]
            })))
            .mount(&server)
            .await;

        // Verify "Won't Do" resolution for not_planned
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(
                "/rest/api/2/issue/PROJ-1/transitions",
            ))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "transition": { "id": "31" },
                "fields": { "resolution": { "name": "Won't Do" } }
            })))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        provider
            .close_issue("myteam/PROJ", 10001, Some("not_planned"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn wiremock_reopen_issue() {
        let server = wiremock::MockServer::start().await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10001.*"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "10001", "key": "PROJ-1",
                "fields": {
                    "summary": "task", "description": null,
                    "status": { "name": "Done", "statusCategory": { "key": "done", "name": "Done" } },
                    "resolution": { "name": "Done" }, "labels": [], "assignee": null,
                    "fixVersions": [], "issuetype": { "name": "Task", "subtask": false },
                    "priority": null, "duedate": null, "security": null,
                    "comment": { "comments": [] },
                    "updated": "2024-03-28T09:15:42.123+0000",
                    "created": "2024-03-20T10:00:00.000+0000"
                }
            })))
            .mount(&server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/rest/api/2/issue/PROJ-1/transitions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "transitions": [{
                    "id": "11", "name": "Reopen",
                    "to": { "name": "To Do", "statusCategory": { "key": "new", "name": "To Do" } },
                    "hasScreen": false, "isAvailable": true
                }]
            })))
            .mount(&server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(
                "/rest/api/2/issue/PROJ-1/transitions",
            ))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path("/rest/api/2/issue/PROJ-1"))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        provider.reopen_issue("myteam/PROJ", 10001).await.unwrap();
    }

    #[tokio::test]
    async fn wiremock_assignee_uses_account_id_when_available() {
        let server = wiremock::MockServer::start().await;
        mount_createmeta_mock(&server).await;

        // Mock POST /issue (create)
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/rest/api/2/issue"))
            .respond_with(
                wiremock::ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "id": "10060", "key": "PROJ-60",
                    "self": "https://example.atlassian.net/rest/api/2/issue/10060"
                })),
            )
            .mount(&server)
            .await;

        // Mock PUT /issue/PROJ-60/assignee (dedicated assignee endpoint)
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path(
                "/rest/api/2/issue/PROJ-60/assignee",
            ))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "accountId": "known-id-123"
            })))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        // Mock GET /issue/10060 (fetch after create)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10060.*"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(jira_issue_json(
                    "10060",
                    "PROJ-60",
                    "Assigned task",
                    "new",
                )),
            )
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let upsert = BackendIssueUpsert {
            title: "Assigned task".into(),
            body: String::new(),
            state: Some("open".into()),
            assignees: vec!["Alice Smith".into()],
            assignee_account_id: Some("known-id-123".into()),
            ..Default::default()
        };
        let record = provider.create_issue("myteam/PROJ", &upsert).await.unwrap();
        assert_eq!(record.title, "Assigned task");
    }

    #[tokio::test]
    async fn wiremock_assignee_falls_back_to_search() {
        let server = wiremock::MockServer::start().await;
        mount_createmeta_mock(&server).await;

        // Mock POST /issue (create)
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/rest/api/2/issue"))
            .respond_with(
                wiremock::ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "id": "10061", "key": "PROJ-61",
                    "self": "https://example.atlassian.net/rest/api/2/issue/10061"
                })),
            )
            .mount(&server)
            .await;

        // Mock GET /user/assignable/search (fallback search)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                "/rest/api/2/user/assignable/search.*",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    { "displayName": "Alice Smith", "accountId": "found-id-456", "name": null }
                ])),
            )
            .mount(&server)
            .await;

        // Mock PUT /issue/PROJ-61/assignee (dedicated assignee endpoint)
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path(
                "/rest/api/2/issue/PROJ-61/assignee",
            ))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        // Mock GET /issue/10061 (fetch after create)
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10061.*"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(jira_issue_json(
                    "10061",
                    "PROJ-61",
                    "Searched task",
                    "new",
                )),
            )
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let upsert = BackendIssueUpsert {
            title: "Searched task".into(),
            body: String::new(),
            state: Some("open".into()),
            assignees: vec!["Alice Smith".into()],
            assignee_account_id: None, // No cached ID, forces search
            ..Default::default()
        };
        let record = provider.create_issue("myteam/PROJ", &upsert).await.unwrap();
        assert_eq!(record.title, "Searched task");
        assert_eq!(record.assignee_account_id, Some("abc-123".into()));
    }

    #[tokio::test]
    async fn wiremock_pull_captures_assignee_identity() {
        let server = wiremock::MockServer::start().await;
        let search_body = serde_json::json!({
            "issues": [jira_issue_json("10001", "PROJ-1", "Issue with assignee", "new")],
            "total": 1
        });
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/search.*"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(&search_body))
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let records = provider.list_issues("myteam/PROJ").await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].assignees, vec!["Alice Smith"]);
        assert_eq!(records[0].assignee_account_id, Some("abc-123".into()));
        assert_eq!(records[0].assignee_name, None);
    }

    #[tokio::test]
    async fn wiremock_search_assignable_users() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex(
                "/rest/api/2/user/assignable/search.*",
            ))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    { "displayName": "Alice Smith", "accountId": "abc-123", "name": null },
                    { "displayName": "Alice Jones", "accountId": "def-456", "name": null }
                ])),
            )
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let users = provider
            .search_assignable_users("PROJ", "Alice")
            .await
            .unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].display_name, "Alice Smith");
        assert_eq!(users[0].account_id, Some("abc-123".into()));
    }

    #[tokio::test]
    async fn wiremock_assignee_uses_stored_name_for_server_dc() {
        let server = wiremock::MockServer::start().await;
        mount_createmeta_mock(&server).await;

        // Mock POST /issue (create)
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/rest/api/2/issue"))
            .respond_with(
                wiremock::ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "id": "10070", "key": "PROJ-70",
                    "self": "https://example.atlassian.net/rest/api/2/issue/10070"
                })),
            )
            .mount(&server)
            .await;

        // Mock PUT /issue/PROJ-70/assignee (dedicated assignee endpoint, Server/DC name)
        wiremock::Mock::given(wiremock::matchers::method("PUT"))
            .and(wiremock::matchers::path(
                "/rest/api/2/issue/PROJ-70/assignee",
            ))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "name": "jdoe"
            })))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        // Mock GET /issue/10070
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10070.*"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(jira_issue_json(
                    "10070",
                    "PROJ-70",
                    "Server DC task",
                    "new",
                )),
            )
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        let upsert = BackendIssueUpsert {
            title: "Server DC task".into(),
            body: String::new(),
            state: Some("open".into()),
            assignees: vec!["John Doe".into()],
            assignee_account_id: None,
            assignee_name: Some("jdoe".into()),
            ..Default::default()
        };
        let record = provider.create_issue("myteam/PROJ", &upsert).await.unwrap();
        assert_eq!(record.title, "Server DC task");
    }

    #[tokio::test]
    async fn wiremock_close_issue_uses_minimal_key_response() {
        // Verify JiraIssueKey works with a response containing only id, key, and
        // a minimal fields object (as Jira returns with ?fields=status)
        let server = wiremock::MockServer::start().await;

        // Mock GET /issue/10001 — return only id, key, and status field
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path_regex("/rest/api/2/issue/10001.*"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "10001",
                    "key": "PROJ-1",
                    "fields": {
                        "status": {
                            "name": "To Do",
                            "statusCategory": { "key": "new", "name": "To Do" }
                        }
                    }
                })),
            )
            .mount(&server)
            .await;

        // Mock transitions
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/rest/api/2/issue/PROJ-1/transitions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "transitions": [{
                        "id": "31", "name": "Done",
                        "to": { "name": "Done", "statusCategory": { "key": "done", "name": "Done" } },
                        "hasScreen": false, "isAvailable": true
                    }]
                })),
            )
            .mount(&server)
            .await;

        // Mock POST transition
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(
                "/rest/api/2/issue/PROJ-1/transitions",
            ))
            .respond_with(wiremock::ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let provider = mock_provider(&server.uri());
        // This would fail if still using JiraIssue instead of JiraIssueKey,
        // because the response lacks required fields like summary, issuetype, etc.
        provider
            .close_issue("myteam/PROJ", 10001, Some("completed"))
            .await
            .unwrap();
    }
}
