use serde::{Deserialize, Serialize};

/// Supported backend types. GitHub and GitLab provide both issue tracking and
/// version control. Jira is issue-only — use the `vc` field on [`BackendConfig`]
/// to link a GitHub/GitLab backend for PRs and branches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Github,
    Gitlab,
    Jira,
    Local,
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Jira => "jira",
            Self::Local => "local",
        }
    }
}

/// Configuration for a single backend entry in `riptsk.yaml`.
///
/// A backend can serve as an issue tracker, a version control provider, or both.
/// The `vc` field allows decoupling: e.g. "issues in Jira, PRs on GitHub".
///
/// # Example config
/// ```yaml
/// backends:
///   - name: github-repo
///     type: github
///     repo: owner/repo
///
///   - name: jira-myteam
///     type: jira
///     host: https://myteam.atlassian.net
///     repo: myteam/PROJ          # org/PROJECT_KEY format
///     default_issue_type: Task
///     vc: github-repo            # PRs/branches go to GitHub
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackendConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub backend: Backend,
    /// Required for Jira (e.g. `https://myteam.atlassian.net`).
    /// Optional for GitHub (defaults to github.com) and GitLab (defaults to gitlab.com).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Repository/project identifier. Format varies by backend:
    /// - GitHub: `owner/repo`
    /// - GitLab: `group/project`
    /// - Jira: `org/PROJECT_KEY` (org is user-chosen, PROJECT_KEY is the Jira project)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_board: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Name of another backend to use for version control (PRs, branches).
    /// Must reference a GitHub or GitLab backend. When omitted:
    /// - GitHub/GitLab backends provide their own VC
    /// - Jira backends are issue-only (no PR/branch commands)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vc: Option<String>,
    /// Jira-specific: default issue type for creation (e.g. "Task", "Story", "Bug").
    /// When absent, the type is discovered from Jira's create metadata endpoint.
    /// Set this to avoid the extra API call on every issue creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_issue_type: Option<String>,
    /// Project key used to build issue IDs (`{KEY}--{number}`).
    /// When absent, the key is derived from `repo`/`name` at runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}
