use serde::{Deserialize, Serialize};

/// Kind of provider — identifies the external service.
/// Renamed from `Backend` to disambiguate it from the RepoProject config shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Github,
    Gitlab,
    Jira,
    Local,
}

impl BackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Jira => "jira",
            Self::Local => "local",
        }
    }
}

/// Version-control nature of a RepoProject's remote.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VCBackendSpec {
    #[serde(rename = "type")]
    pub kind: BackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// GitHub: `owner/repo`. GitLab: `group/project`. Local: unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Local VC only: canonical absolute path to the RepoProject directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Task-tracker nature of a RepoProject's issue backend.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TasksBackendSpec {
    #[serde(rename = "type")]
    pub kind: BackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// GitHub: `owner/repo`. GitLab: `group/project`. Local: unused.
    /// For Jira, use `jira_project` instead of `repo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Jira only: `org/PROJECT_KEY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira_project: Option<String>,
    /// Jira only: default issue type for creation (e.g. "Task", "Story", "Bug").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_issue_type: Option<String>,
    /// Local tasks backend only: canonical absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A RepoProject — the coding project unit. Corresponds to one registered directory.
///
/// When `vc_backend` and `tasks_backend` describe the same remote (e.g. GitHub for
/// both), the info is duplicated intentionally. Explicit over clever.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoProject {
    pub name: String,
    pub vc_backend: VCBackendSpec,
    pub tasks_backend: TasksBackendSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_board: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_org: Option<String>,
    /// Short ID prefix (e.g. `RIPTASK` in `RIPTASK--123`). Derived at registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Label used to partition a shared Jira project across multiple RepoProjects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_project_label: Option<String>,
}
