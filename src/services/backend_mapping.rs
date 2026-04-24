//! Backend mapping — builds providers from config and converts between remote and local formats.

use crate::adapters::backend::{
    BackendIssueRecord, BackendIssueUpsert, BackendProvider, IssueTracker, VersionControl,
};
use crate::adapters::github::GithubProvider;
use crate::adapters::gitlab::GitlabProvider;
use crate::domain::issue::{
    GithubIssueMeta, GitlabIssueMeta, IssueDocument, IssueFrontmatter, IssueState, JiraIssueMeta,
    Priority,
};
use crate::error::RiptaskError;
use crate::models::{BackendKind, RepoProject, VCBackendSpec};
use crate::services::issue_ids;
use crate::services::issue_service::{generate_slug, now_utc};
use std::process::Command;

#[derive(Debug)]
enum CredentialSource {
    EnvVar(&'static str),
    CliTool(&'static str),
}

#[derive(Debug, Clone)]
pub struct GitHttpAuth {
    pub host: String,
    pub token: String,
}

impl std::fmt::Display for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnvVar(name) | Self::CliTool(name) => f.write_str(name),
        }
    }
}

pub fn build_hosted_provider(
    vc_backend: &VCBackendSpec,
    display_name: &str,
) -> Result<Box<dyn BackendProvider>, RiptaskError> {
    match vc_backend.kind {
        BackendKind::Github => {
            let (token, source) = resolve_github_token(display_name)?;
            crate::ui::info(&format!(
                "auth: github VCBackend '{}' using {}",
                display_name, source
            ));
            Ok(Box::new(GithubProvider::new(&token)?))
        }
        BackendKind::Gitlab => {
            let host = vc_backend
                .host
                .as_deref()
                .unwrap_or("https://gitlab.com")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            let (token, source) = resolve_gitlab_token(display_name, &host)?;
            crate::ui::info(&format!(
                "auth: gitlab VCBackend '{}' using {}",
                display_name, source
            ));
            Ok(Box::new(GitlabProvider::new(&host, &token)?))
        }
        BackendKind::Local => Err(RiptaskError::Config(format!(
            "RepoProject {}: VCBackend is local-only",
            display_name
        ))),
        BackendKind::Jira => Err(RiptaskError::Config(format!(
            "RepoProject {}: VCBackend cannot be jira",
            display_name
        ))),
    }
}

pub fn build_issue_tracker(
    repo_project: &RepoProject,
) -> Result<Box<dyn IssueTracker>, RiptaskError> {
    match repo_project.tasks_backend.kind {
        BackendKind::Github => {
            let (token, source) = resolve_github_token(&repo_project.name)?;
            crate::ui::info(&format!(
                "auth: github TasksBackend '{}' using {}",
                repo_project.name, source
            ));
            Ok(Box::new(GithubProvider::new(&token)?))
        }
        BackendKind::Gitlab => {
            let host = repo_project
                .tasks_backend
                .host
                .as_deref()
                .unwrap_or("https://gitlab.com")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            let (token, source) = resolve_gitlab_token(&repo_project.name, &host)?;
            crate::ui::info(&format!(
                "auth: gitlab TasksBackend '{}' using {}",
                repo_project.name, source
            ));
            Ok(Box::new(GitlabProvider::new(&host, &token)?))
        }
        BackendKind::Jira => {
            let host =
                repo_project.tasks_backend.host.as_deref().ok_or_else(|| {
                    RiptaskError::Config("Jira TasksBackend requires 'host'".into())
                })?;
            let (auth, source) = resolve_jira_credentials(&repo_project.name, host)?;
            crate::ui::info(&format!(
                "auth: jira TasksBackend '{}' using {}",
                repo_project.name, source
            ));
            Ok(Box::new(crate::adapters::jira::JiraProvider::with_config(
                host,
                auth,
                repo_project.tasks_backend.default_issue_type.clone(),
                repo_project.repo_project_label.clone(),
            )?))
        }
        BackendKind::Local => Err(RiptaskError::Config(format!(
            "RepoProject {}: TasksBackend is local-only",
            repo_project.name
        ))),
    }
}

pub fn build_version_control(
    vc_backend: &VCBackendSpec,
    display_name: &str,
) -> Result<Box<dyn VersionControl>, RiptaskError> {
    match vc_backend.kind {
        BackendKind::Github | BackendKind::Gitlab => {
            let provider = build_hosted_provider(vc_backend, display_name)?;
            Ok(provider)
        }
        BackendKind::Local => Err(RiptaskError::Config(format!(
            "RepoProject {}: VCBackend is local-only",
            display_name
        ))),
        BackendKind::Jira => Err(RiptaskError::Config(format!(
            "RepoProject {}: VCBackend cannot be jira",
            display_name
        ))),
    }
}

pub fn resolve_git_auth(vc_backend: &VCBackendSpec, display_name: &str) -> Option<GitHttpAuth> {
    match vc_backend.kind {
        BackendKind::Gitlab => {
            let host = vc_backend
                .host
                .as_deref()
                .unwrap_or("https://gitlab.com")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            let (token, _source) = resolve_gitlab_token(display_name, &host).ok()?;
            Some(GitHttpAuth { host, token })
        }
        BackendKind::Github => {
            let (token, _source) = resolve_github_token(display_name).ok()?;
            Some(GitHttpAuth {
                host: "github.com".into(),
                token,
            })
        }
        BackendKind::Local | BackendKind::Jira => None,
    }
}

pub fn issue_to_upsert(doc: &IssueDocument, repo_project: &RepoProject) -> BackendIssueUpsert {
    let state = match doc.frontmatter.status {
        IssueState::Done => "closed",
        _ => "open",
    };
    let state_reason = sanitize_state_reason(state, doc.frontmatter.state_reason.as_deref());
    let assignee_account_id = doc
        .frontmatter
        .jira
        .as_ref()
        .and_then(|meta| meta.assignee_account_id.clone());
    let assignee_name = doc
        .frontmatter
        .jira
        .as_ref()
        .and_then(|meta| meta.assignee_name.clone());
    let mut labels = build_state_labels(&doc.frontmatter.status, &doc.frontmatter.labels);
    if let Some(label) = effective_repo_project_label(repo_project)
        && !labels.iter().any(|candidate| candidate == label)
    {
        labels.push(label.to_owned());
    }
    BackendIssueUpsert {
        title: doc.frontmatter.title.clone(),
        body: doc.body.clone(),
        state: Some(state.into()),
        state_reason,
        labels,
        assignees: doc.frontmatter.assignees.clone(),
        milestone_id: doc
            .frontmatter
            .github
            .as_ref()
            .and_then(|meta| meta.milestone_id)
            .or_else(|| {
                doc.frontmatter
                    .gitlab
                    .as_ref()
                    .and_then(|meta| meta.milestone_id)
            }),
        due_date: doc.frontmatter.due.clone(),
        weight: doc.frontmatter.weight,
        confidential: doc.frontmatter.confidential,
        discussion_locked: doc.frontmatter.discussion_locked,
        assignee_account_id,
        assignee_name,
    }
}

pub fn backend_to_local(record: &BackendIssueRecord, repo_project: &RepoProject) -> IssueDocument {
    let state = state_from_backend(record);
    let issue_id = issue_ids::format_id(&issue_ids::effective_key(repo_project), record.issue_id);
    let task_repo = repo_project.tasks_backend.repo.clone().unwrap_or_default();
    let jira_project = repo_project
        .tasks_backend
        .jira_project
        .as_deref()
        .unwrap_or_default();
    IssueDocument {
        frontmatter: IssueFrontmatter {
            id: issue_id.clone(),
            title: record.title.clone(),
            status: state.clone(),
            board: repo_project
                .default_board
                .clone()
                .unwrap_or_else(|| "personal".into()),
            project: repo_project.name.clone(),
            org: repo_project.default_org.clone(),
            priority: Some(Priority::Medium),
            labels: labels_without_status(
                &record.labels,
                effective_repo_project_label(repo_project),
            ),
            assignees: record.assignees.clone(),
            milestone: record.milestone.clone(),
            state_reason: record.state_reason.clone(),
            cycle: None,
            order: Some(1),
            gitlab: if repo_project.tasks_backend.kind == BackendKind::Gitlab {
                Some(GitlabIssueMeta {
                    repo: task_repo.clone(),
                    issue_id: Some(record.issue_id),
                    milestone_id: record.milestone_id,
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(state.clone()),
                })
            } else {
                None
            },
            github: if repo_project.tasks_backend.kind == BackendKind::Github {
                Some(GithubIssueMeta {
                    repo: task_repo,
                    issue_id: Some(record.issue_id),
                    node_id: record.node_id.clone(),
                    milestone_id: record.milestone_id,
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(state.clone()),
                })
            } else {
                None
            },
            jira: if repo_project.tasks_backend.kind == BackendKind::Jira {
                let issue_key = extract_jira_key_from_url(&record.url);
                Some(JiraIssueMeta {
                    project_key: crate::adapters::jira::JiraProvider::project_key(jira_project)
                        .to_owned(),
                    issue_key,
                    issue_id: Some(record.issue_id),
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(state.clone()),
                    issue_type: record.issue_type.clone(),
                    assignee_account_id: record.assignee_account_id.clone(),
                    assignee_name: record.assignee_name.clone(),
                })
            } else {
                None
            },
            local_updated_at: record.updated_at.clone(),
            due: record.due_date.clone(),
            weight: record.weight,
            confidential: record.confidential,
            discussion_locked: record.discussion_locked,
            issue_type: record.issue_type.clone(),
            locked: record.locked,
            lock_reason: record.lock_reason.clone(),
            recurring: None,
            remote_deleted: false,
            id_slug: Some(generate_slug(&issue_id, &record.title)),
            branch: None,
            pr_url: None,
            pr_number: None,
        },
        body: record.body.clone().unwrap_or_default(),
        remote_section: None,
    }
}

pub fn copy_local_only_fields(target: &mut IssueFrontmatter, source: &IssueFrontmatter) {
    target.pr_url = source.pr_url.clone();
    target.pr_number = source.pr_number;
    target.branch = source.branch.clone();
    target.order = source.order;
    target.recurring = source.recurring.clone();
    target.cycle = source.cycle.clone();
    target.board = source.board.clone();
    target.org = source.org.clone();
    target.priority = source.priority.clone();
}

pub fn build_state_labels(state: &IssueState, labels: &[String]) -> Vec<String> {
    let mut labels = labels
        .iter()
        .filter(|label| !label.starts_with("status::"))
        .cloned()
        .collect::<Vec<_>>();
    labels.push(format!("status::{}", state.as_str()));
    labels
}

pub fn update_issue_from_backend(
    issue: &mut IssueDocument,
    record: &BackendIssueRecord,
    repo_project: &RepoProject,
) {
    let state = state_from_backend(record);
    issue.frontmatter.title = record.title.clone();
    issue.frontmatter.status = state.clone();
    issue.frontmatter.labels =
        labels_without_status(&record.labels, effective_repo_project_label(repo_project));
    issue.frontmatter.assignees = record.assignees.clone();
    issue.frontmatter.milestone = record.milestone.clone();
    issue.frontmatter.state_reason = record.state_reason.clone();
    issue.frontmatter.local_updated_at = record.updated_at.clone();
    issue.frontmatter.due = record.due_date.clone();
    issue.frontmatter.weight = record.weight;
    issue.frontmatter.confidential = record.confidential;
    issue.frontmatter.discussion_locked = record.discussion_locked;
    issue.frontmatter.issue_type = record.issue_type.clone();
    issue.frontmatter.locked = record.locked;
    issue.frontmatter.lock_reason = record.lock_reason.clone();
    issue.frontmatter.id_slug = Some(generate_slug(&issue.frontmatter.id, &record.title));
    issue.body = record.body.clone().unwrap_or_default();
    match repo_project.tasks_backend.kind {
        BackendKind::Github => {
            issue.frontmatter.github = Some(GithubIssueMeta {
                repo: repo_project.tasks_backend.repo.clone().unwrap_or_default(),
                issue_id: Some(record.issue_id),
                node_id: record.node_id.clone(),
                milestone_id: record.milestone_id,
                url: Some(record.url.clone()),
                updated_at: record.updated_at.clone(),
                last_pushed_state: Some(state),
            });
        }
        BackendKind::Gitlab => {
            issue.frontmatter.gitlab = Some(GitlabIssueMeta {
                repo: repo_project.tasks_backend.repo.clone().unwrap_or_default(),
                issue_id: Some(record.issue_id),
                milestone_id: record.milestone_id,
                url: Some(record.url.clone()),
                updated_at: record.updated_at.clone(),
                last_pushed_state: Some(state),
            });
        }
        BackendKind::Jira => {
            let issue_key = extract_jira_key_from_url(&record.url);
            issue.frontmatter.jira = Some(JiraIssueMeta {
                project_key: crate::adapters::jira::JiraProvider::project_key(
                    repo_project
                        .tasks_backend
                        .jira_project
                        .as_deref()
                        .unwrap_or_default(),
                )
                .to_owned(),
                issue_key,
                issue_id: Some(record.issue_id),
                url: Some(record.url.clone()),
                updated_at: record.updated_at.clone(),
                last_pushed_state: Some(state),
                issue_type: record.issue_type.clone(),
                assignee_account_id: record.assignee_account_id.clone(),
                assignee_name: record.assignee_name.clone(),
            });
        }
        BackendKind::Local => {}
    }
}

pub fn backend_state_entry(
    record: &BackendIssueRecord,
) -> crate::domain::backend_state::BackendStateEntry {
    crate::domain::backend_state::BackendStateEntry {
        title: record.title.clone(),
        state: record.state.clone(),
        state_reason: record.state_reason.clone(),
        labels: record.labels.clone(),
        assignees: record.assignees.clone(),
        milestone: record.milestone.clone(),
        milestone_id: record.milestone_id,
        due_date: record.due_date.clone(),
        weight: record.weight,
        confidential: record.confidential,
        discussion_locked: record.discussion_locked,
        updated_at: record.updated_at.clone(),
    }
}

pub fn current_timestamp() -> String {
    now_utc()
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn run_gh_auth_token(host: &str) -> Result<String, String> {
    let output = match Command::new("gh")
        .args(["auth", "token", "--hostname", host])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("'gh' not found on PATH".into());
        }
        Err(error) => return Err(format!("'gh' failed to run: {error}")),
    };
    if !output.status.success() {
        let status = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".into());
        return Err(format!("'gh auth token' exited with status {status}"));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if stdout.is_empty() {
        return Err("'gh auth token' returned empty output (expected raw token on stdout)".into());
    }
    Ok(stdout)
}

fn run_glab_auth_token(host: &str) -> Result<String, String> {
    let output = match Command::new("glab")
        .args(["auth", "status", "--show-token", "--hostname", host])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("'glab' not found on PATH".into());
        }
        Err(error) => return Err(format!("'glab' failed to run: {error}")),
    };
    if !output.status.success() {
        let status = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".into());
        return Err(format!("'glab auth status' exited with status {status}"));
    }
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if stderr.trim().is_empty() {
        return Err("'glab auth status' produced no output on stderr".into());
    }
    parse_glab_token(&stderr).ok_or_else(|| {
        "glab output did not contain expected 'Token found:' line (glab may have changed its output format)".into()
    })
}

fn parse_glab_token(stderr_output: &str) -> Option<String> {
    stderr_output.lines().find_map(|line| {
        let normalized = line
            .trim()
            .trim_start_matches(|ch: char| !ch.is_alphanumeric());
        normalized
            .strip_prefix("Token found:")
            .or_else(|| normalized.strip_prefix("Token:"))
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn resolve_github_token(display_name: &str) -> Result<(String, CredentialSource), RiptaskError> {
    if let Some(token) = non_empty_env("GITHUB_TOKEN") {
        return Ok((token, CredentialSource::EnvVar("GITHUB_TOKEN")));
    }
    if let Some(token) = non_empty_env("GH_TOKEN") {
        return Ok((token, CredentialSource::EnvVar("GH_TOKEN")));
    }
    match run_gh_auth_token("github.com") {
        Ok(token) => Ok((token, CredentialSource::CliTool("gh auth token"))),
        Err(reason) => Err(RiptaskError::Auth(format!(
            "GitHub authentication failed for VC/Tasks backend '{display_name}'\n\n\
Tried: GITHUB_TOKEN, GH_TOKEN, gh auth token\n\n\
{reason}\n\n\
To fix, do one of:\n\
  • export GITHUB_TOKEN=<your-token>\n\
  • export GH_TOKEN=<your-token>\n\
  • gh auth login"
        ))),
    }
}

fn resolve_gitlab_token(
    display_name: &str,
    host: &str,
) -> Result<(String, CredentialSource), RiptaskError> {
    if let Some(token) = non_empty_env("GITLAB_TOKEN") {
        return Ok((token, CredentialSource::EnvVar("GITLAB_TOKEN")));
    }
    match run_glab_auth_token(host) {
        Ok(token) => Ok((
            token,
            CredentialSource::CliTool("glab auth status --show-token"),
        )),
        Err(reason) => Err(RiptaskError::Auth(format!(
            "GitLab authentication failed for VC/Tasks backend '{display_name}' (host: {host})\n\n\
Tried: GITLAB_TOKEN, glab auth status --show-token\n\n\
{reason}\n\n\
To fix, do one of:\n\
  • export GITLAB_TOKEN=<your-token>\n\
  • glab auth login --hostname {host}"
        ))),
    }
}

fn resolve_jira_credentials(
    display_name: &str,
    host: &str,
) -> Result<(crate::adapters::jira::JiraAuth, CredentialSource), RiptaskError> {
    use crate::adapters::jira::JiraAuth;
    if let Some(token) = non_empty_env("JIRA_API_TOKEN") {
        if let Some(email) = non_empty_env("JIRA_EMAIL") {
            return Ok((
                JiraAuth::Basic { email, token },
                CredentialSource::EnvVar("JIRA_API_TOKEN+JIRA_EMAIL"),
            ));
        }
        return Ok((
            JiraAuth::Pat(token),
            CredentialSource::EnvVar("JIRA_API_TOKEN"),
        ));
    }
    match run_jira_cli_go_auth(host) {
        Ok((login, token, is_bearer)) => {
            let auth = if is_bearer {
                JiraAuth::Pat(token)
            } else {
                JiraAuth::Basic {
                    email: login,
                    token,
                }
            };
            Ok((auth, CredentialSource::CliTool("jira-cli-go keychain")))
        }
        Err(_) => Err(RiptaskError::Auth(format!(
            "Jira authentication failed for TasksBackend '{display_name}' (host: {host})\n\n\
Tried: JIRA_API_TOKEN, jira-cli-go keychain\n\n\
To fix, do one of:\n\
  • export JIRA_API_TOKEN=<token> JIRA_EMAIL=<email>\n\
  • brew install ankitpokhrel/jira-cli/jira-cli && jira init"
        ))),
    }
}

fn run_jira_cli_go_auth(host: &str) -> Result<(String, String, bool), String> {
    let config_path = dirs_jira_cli_config();
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("cannot read jira-cli config at {config_path}: {e}"))?;

    #[derive(serde::Deserialize)]
    struct JiraCliGoConfig {
        server: Option<String>,
        login: Option<String>,
        #[serde(default)]
        auth_type: Option<String>,
    }

    let config: JiraCliGoConfig =
        serde_yaml_ng::from_str(&content).map_err(|e| format!("invalid jira-cli config: {e}"))?;
    let server = config
        .server
        .ok_or_else(|| "jira-cli config missing 'server' field".to_string())?;
    let login = config
        .login
        .ok_or_else(|| "jira-cli config missing 'login' field".to_string())?;
    let server_trimmed = server.trim_end_matches('/');
    let host_trimmed = host.trim_end_matches('/');
    if !server_trimmed.eq_ignore_ascii_case(host_trimmed) {
        return Err(format!(
            "jira-cli config server '{server}' does not match expected host '{host}'"
        ));
    }
    let entry =
        keyring::Entry::new("jira-cli", &login).map_err(|e| format!("keyring error: {e}"))?;
    let token = entry
        .get_password()
        .map_err(|e| format!("cannot read jira-cli token from keychain: {e}"))?;
    let auth_type_env = std::env::var("JIRA_AUTH_TYPE").ok();
    let is_bearer = config
        .auth_type
        .as_deref()
        .or(auth_type_env.as_deref())
        .is_some_and(|kind| kind.eq_ignore_ascii_case("bearer"));
    if is_bearer {
        Ok((login, token, true))
    } else {
        Ok((login, token, false))
    }
}

fn dirs_jira_cli_config() -> String {
    if let Ok(config_dir) = std::env::var("XDG_CONFIG_HOME") {
        return format!("{config_dir}/.jira/.config.yml");
    }
    if let Ok(home) = std::env::var("HOME") {
        return format!("{home}/.config/.jira/.config.yml");
    }
    "~/.config/.jira/.config.yml".into()
}

fn state_from_backend(record: &BackendIssueRecord) -> IssueState {
    if record.state.eq_ignore_ascii_case("closed") || record.state.eq_ignore_ascii_case("done") {
        return IssueState::Done;
    }
    for label in &record.labels {
        if let Some(state) = label.strip_prefix("status::") {
            return match state {
                "backlog" => IssueState::Backlog,
                "todo" => IssueState::Todo,
                "in-progress" => IssueState::InProgress,
                "review" => IssueState::Review,
                "done" => IssueState::Done,
                _ => IssueState::Backlog,
            };
        }
    }
    IssueState::Backlog
}

fn sanitize_state_reason(state: &str, reason: Option<&str>) -> Option<String> {
    let reason = reason?;
    let valid = match state {
        "closed" => matches!(reason, "completed" | "not_planned" | "duplicate"),
        "open" => matches!(reason, "reopened"),
        _ => true,
    };
    if valid { Some(reason.to_owned()) } else { None }
}

fn extract_jira_key_from_url(url: &str) -> Option<String> {
    url.rsplit('/').next().map(|value| value.to_owned())
}

/// Return the RepoProject's partition label only when it is meaningful for the
/// active tasks backend. Today only Jira uses label-partitioning, so non-Jira
/// tasks backends ignore the label even if one is stored on the RepoProject.
pub fn effective_repo_project_label(repo_project: &RepoProject) -> Option<&str> {
    if repo_project.tasks_backend.kind == BackendKind::Jira {
        repo_project.repo_project_label.as_deref()
    } else {
        None
    }
}

fn labels_without_status(labels: &[String], repo_project_label: Option<&str>) -> Vec<String> {
    labels
        .iter()
        .filter(|label| !label.starts_with("status::"))
        .filter(|label| Some(label.as_str()) != repo_project_label)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        copy_local_only_fields, issue_to_upsert, non_empty_env, parse_glab_token, resolve_git_auth,
        sanitize_state_reason,
    };
    use crate::domain::issue::{IssueDocument, IssueFrontmatter, IssueState, Priority};
    use crate::models::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct EnvGuard {
        name: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn unset(name: &'static str) -> Self {
            let original = std::env::var(name).ok();
            unsafe {
                std::env::remove_var(name);
            }
            Self { name, original }
        }

        fn set(name: &'static str, value: &str) -> Self {
            let original = std::env::var(name).ok();
            unsafe {
                std::env::set_var(name, value);
            }
            Self { name, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => unsafe {
                    std::env::set_var(self.name, value);
                },
                None => unsafe {
                    std::env::remove_var(self.name);
                },
            }
        }
    }

    fn repo_project(kind: BackendKind, label: Option<&str>) -> RepoProject {
        RepoProject {
            name: "demo".into(),
            vc_backend: VCBackendSpec {
                kind: BackendKind::Github,
                host: None,
                repo: Some("owner/repo".into()),
                path: None,
            },
            tasks_backend: TasksBackendSpec {
                kind,
                host: Some("https://jira.example".into()),
                repo: Some("owner/repo".into()),
                jira_project: Some("org/PROJ".into()),
                default_issue_type: None,
                path: None,
            },
            default_board: Some("personal".into()),
            default_org: None,
            key: Some("DEMO".into()),
            repo_project_label: label.map(str::to_owned),
        }
    }

    fn frontmatter_fixture(id: &str, title: &str) -> IssueFrontmatter {
        IssueFrontmatter {
            id: id.into(),
            title: title.into(),
            status: IssueState::Todo,
            board: "ops".into(),
            project: "test-project".into(),
            org: Some("eng".into()),
            priority: Some(Priority::High),
            labels: vec!["bug".into()],
            assignees: vec!["alice".into()],
            milestone: Some("M1".into()),
            state_reason: None,
            cycle: Some("2026-W13".into()),
            order: Some(7),
            gitlab: None,
            github: None,
            jira: None,
            local_updated_at: "2026-03-20T10:00:00Z".into(),
            due: None,
            weight: None,
            confidential: None,
            discussion_locked: None,
            issue_type: None,
            locked: None,
            lock_reason: None,
            recurring: Some("weekly-42".into()),
            remote_deleted: false,
            id_slug: Some("slug".into()),
            branch: Some("feature/42".into()),
            pr_url: Some("https://example.invalid/pulls/42".into()),
            pr_number: Some(42),
        }
    }

    #[test]
    fn non_empty_env_returns_none_for_unset_var() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::unset("RIPTSK_TEST_NON_EMPTY_ENV");
        assert_eq!(non_empty_env("RIPTSK_TEST_NON_EMPTY_ENV"), None);
    }

    #[test]
    fn parse_glab_token_extracts_token_from_multiline_output() {
        let stderr_output = "gitlab.suse.de\n  ✓ Logged in to gitlab.suse.de as user\n  ✓ Token found: glpat-xxxx\n  ✓ REST API Endpoint: https://gitlab.suse.de/api/v4/";
        assert_eq!(parse_glab_token(stderr_output), Some("glpat-xxxx".into()));
    }

    #[test]
    fn resolve_git_auth_returns_none_for_local_backend() {
        let vc_backend = VCBackendSpec {
            kind: BackendKind::Local,
            host: None,
            repo: None,
            path: None,
        };
        assert!(resolve_git_auth(&vc_backend, "local").is_none());
    }

    #[test]
    fn copy_local_only_fields_copies_all_fields() {
        let source = frontmatter_fixture("SRC", "Source title");
        let mut target = frontmatter_fixture("DST", "Target title");
        target.pr_url = None;
        copy_local_only_fields(&mut target, &source);
        assert_eq!(
            target.pr_url.as_deref(),
            Some("https://example.invalid/pulls/42")
        );
        assert_eq!(target.pr_number, Some(42));
        assert_eq!(target.branch.as_deref(), Some("feature/42"));
    }

    #[test]
    fn sanitize_state_reason_drops_reopened_when_closing() {
        assert_eq!(sanitize_state_reason("closed", Some("reopened")), None);
    }

    #[test]
    fn issue_to_upsert_injects_repo_project_label() {
        let repo_project = repo_project(BackendKind::Jira, Some("proj::repo-a"));
        let issue = IssueDocument {
            frontmatter: frontmatter_fixture("DEMO--1", "Title"),
            body: "Body".into(),
            remote_section: None,
        };
        let upsert = issue_to_upsert(&issue, &repo_project);
        assert!(upsert.labels.contains(&"proj::repo-a".to_string()));
        assert!(upsert.labels.iter().any(|label| label == "status::todo"));
    }

    #[test]
    fn issue_to_upsert_skips_label_for_non_jira_backend() {
        // Auto-registration may populate `repo_project_label` on every RepoProject
        // (not just Jira), but label injection is strictly a Jira partitioning
        // feature. GitHub/GitLab upserts must not get an extra project label.
        let repo_project = repo_project(BackendKind::Github, Some("proj::repo-a"));
        let issue = IssueDocument {
            frontmatter: frontmatter_fixture("DEMO--1", "Title"),
            body: "Body".into(),
            remote_section: None,
        };
        let upsert = issue_to_upsert(&issue, &repo_project);
        assert!(
            !upsert.labels.iter().any(|label| label == "proj::repo-a"),
            "GitHub upsert labels should not contain repo_project_label: {:?}",
            upsert.labels
        );
    }

    #[test]
    fn issue_to_upsert_dedupes_repo_project_label() {
        let repo_project = repo_project(BackendKind::Jira, Some("proj::repo-a"));
        let mut frontmatter = frontmatter_fixture("DEMO--1", "Title");
        frontmatter.labels.push("proj::repo-a".into());
        let issue = IssueDocument {
            frontmatter,
            body: "Body".into(),
            remote_section: None,
        };
        let upsert = issue_to_upsert(&issue, &repo_project);
        assert_eq!(
            upsert
                .labels
                .iter()
                .filter(|label| *label == "proj::repo-a")
                .count(),
            1
        );
    }
}
