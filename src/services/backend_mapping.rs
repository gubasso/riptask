use crate::adapters::backend::{
    BackendIssueRecord, BackendIssueUpsert, BackendProvider, IssueTracker, VersionControl,
};
use crate::adapters::github::GithubProvider;
use crate::adapters::gitlab::GitlabProvider;
use crate::config::Config;
use crate::domain::issue::{
    GithubIssueMeta, GitlabIssueMeta, IssueDocument, IssueFrontmatter, IssueState, JiraIssueMeta,
    Priority,
};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
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

pub fn build_provider_for_backend(
    backend: &BackendConfig,
) -> Result<Box<dyn BackendProvider>, RiptskError> {
    match backend.backend {
        Backend::Github => {
            let (token, source) = resolve_github_token(&backend.name)?;
            crate::ui::info(&format!(
                "auth: github backend '{}' using {}",
                backend.name, source
            ));
            Ok(Box::new(GithubProvider::new(&token)?))
        }
        Backend::Gitlab => {
            let host = backend
                .host
                .as_deref()
                .unwrap_or("https://gitlab.com")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            let (token, source) = resolve_gitlab_token(&backend.name, &host)?;
            crate::ui::info(&format!(
                "auth: gitlab backend '{}' using {}",
                backend.name, source
            ));
            Ok(Box::new(GitlabProvider::new(&host, &token)?))
        }
        Backend::Jira => Err(RiptskError::Config(
            "Jira backend uses build_issue_tracker() — see resolve_vc_for_backend()".into(),
        )),
        Backend::Local => Err(RiptskError::Config(format!(
            "backend {} is local-only",
            backend.name
        ))),
    }
}

/// Build an issue tracker for a backend config.
/// Works for Github, Gitlab, Jira. Errors for Local.
pub fn build_issue_tracker(backend: &BackendConfig) -> Result<Box<dyn IssueTracker>, RiptskError> {
    match backend.backend {
        Backend::Github => {
            let (token, source) = resolve_github_token(&backend.name)?;
            crate::ui::info(&format!(
                "auth: github backend '{}' using {}",
                backend.name, source
            ));
            Ok(Box::new(GithubProvider::new(&token)?))
        }
        Backend::Gitlab => {
            let host = backend
                .host
                .as_deref()
                .unwrap_or("https://gitlab.com")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            let (token, source) = resolve_gitlab_token(&backend.name, &host)?;
            crate::ui::info(&format!(
                "auth: gitlab backend '{}' using {}",
                backend.name, source
            ));
            Ok(Box::new(GitlabProvider::new(&host, &token)?))
        }
        Backend::Jira => {
            let host = backend
                .host
                .as_deref()
                .ok_or_else(|| RiptskError::Config("Jira backend requires 'host'".into()))?;
            let (auth, source) = resolve_jira_credentials(&backend.name, host)?;
            crate::ui::info(&format!(
                "auth: jira backend '{}' using {}",
                backend.name, source
            ));
            Ok(Box::new(
                crate::adapters::jira::JiraProvider::with_issue_type(
                    host,
                    auth,
                    backend.default_issue_type.clone(),
                )?,
            ))
        }
        Backend::Local => Err(RiptskError::Config(format!(
            "backend {} is local-only",
            backend.name
        ))),
    }
}

/// Build a version control provider for a backend config.
/// Works for Github, Gitlab. Errors for Local.
pub fn build_version_control(
    backend: &BackendConfig,
) -> Result<Box<dyn VersionControl>, RiptskError> {
    match backend.backend {
        Backend::Github => {
            let (token, source) = resolve_github_token(&backend.name)?;
            crate::ui::info(&format!(
                "auth: github backend '{}' using {}",
                backend.name, source
            ));
            Ok(Box::new(GithubProvider::new(&token)?))
        }
        Backend::Gitlab => {
            let host = backend
                .host
                .as_deref()
                .unwrap_or("https://gitlab.com")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            let (token, source) = resolve_gitlab_token(&backend.name, &host)?;
            crate::ui::info(&format!(
                "auth: gitlab backend '{}' using {}",
                backend.name, source
            ));
            Ok(Box::new(GitlabProvider::new(&host, &token)?))
        }
        Backend::Jira => Err(RiptskError::Config(
            "Jira is an issue tracker — use the 'vc' config field to link a GitHub/GitLab backend for PRs and branches".into(),
        )),
        Backend::Local => Err(RiptskError::Config(format!(
            "backend {} is local-only — no version control",
            backend.name
        ))),
    }
}

pub type VcResolution<'a> = Option<(Box<dyn VersionControl>, &'a BackendConfig)>;

/// Resolve the VC provider for a given issue backend.
/// If backend has `vc` field, look up that backend and build its VC provider.
/// If backend itself supports VC (Github/Gitlab), use itself.
/// If neither, return None (issue-only project).
pub fn resolve_vc_for_backend<'a>(
    backend: &'a BackendConfig,
    config: &'a Config,
) -> Result<VcResolution<'a>, RiptskError> {
    if let Some(vc_name) = &backend.vc {
        let vc_backend = config
            .backends
            .iter()
            .find(|b| &b.name == vc_name)
            .ok_or_else(|| {
                RiptskError::Config(format!(
                    "backend '{}' references vc '{}' which does not exist",
                    backend.name, vc_name
                ))
            })?;
        if !matches!(vc_backend.backend, Backend::Github | Backend::Gitlab) {
            return Err(RiptskError::Config(format!(
                "vc '{}' must be a github or gitlab backend",
                vc_name
            )));
        }
        let provider = build_version_control(vc_backend)?;
        return Ok(Some((provider, vc_backend)));
    }
    // If backend itself supports VC (Github/Gitlab), use itself
    match backend.backend {
        Backend::Github | Backend::Gitlab => {
            let provider = build_version_control(backend)?;
            Ok(Some((provider, backend)))
        }
        _ => Ok(None),
    }
}

pub fn resolve_git_auth(backend: &BackendConfig) -> Option<GitHttpAuth> {
    match backend.backend {
        Backend::Gitlab => {
            let host = backend
                .host
                .as_deref()
                .unwrap_or("https://gitlab.com")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            let (token, _source) = resolve_gitlab_token(&backend.name, &host).ok()?;
            Some(GitHttpAuth { host, token })
        }
        Backend::Github => {
            let (token, _source) = resolve_github_token(&backend.name).ok()?;
            Some(GitHttpAuth {
                host: "github.com".into(),
                token,
            })
        }
        Backend::Jira => None,
        Backend::Local => None,
    }
}

pub fn issue_to_upsert(doc: &IssueDocument) -> BackendIssueUpsert {
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
    BackendIssueUpsert {
        title: doc.frontmatter.title.clone(),
        body: doc.body.clone(),
        state: Some(state.into()),
        state_reason,
        labels: build_state_labels(&doc.frontmatter.status, &doc.frontmatter.labels),
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

pub fn backend_to_local(record: &BackendIssueRecord, backend: &BackendConfig) -> IssueDocument {
    let state = state_from_backend(record);
    let issue_id = issue_ids::format_id(
        &issue_ids::derive_scope_from_backend(backend),
        record.issue_id,
    );
    IssueDocument {
        frontmatter: IssueFrontmatter {
            id: issue_id.clone(),
            title: record.title.clone(),
            status: state.clone(),
            board: backend
                .default_board
                .clone()
                .unwrap_or_else(|| "personal".into()),
            project: backend.name.clone(),
            org: backend.default_org.clone(),
            priority: Some(Priority::Medium),
            labels: labels_without_status(&record.labels),
            assignees: record.assignees.clone(),
            milestone: record.milestone.clone(),
            state_reason: record.state_reason.clone(),
            cycle: None,
            order: Some(1),
            gitlab: if backend.backend == Backend::Gitlab {
                Some(GitlabIssueMeta {
                    repo: backend.repo.clone().unwrap_or_default(),
                    issue_id: Some(record.issue_id),
                    milestone_id: record.milestone_id,
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(state.clone()),
                })
            } else {
                None
            },
            github: if backend.backend == Backend::Github {
                Some(GithubIssueMeta {
                    repo: backend.repo.clone().unwrap_or_default(),
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
            jira: if backend.backend == Backend::Jira {
                let issue_key = extract_jira_key_from_url(&record.url);
                Some(JiraIssueMeta {
                    project_key: crate::adapters::jira::JiraProvider::project_key(
                        backend.repo.as_deref().unwrap_or_default(),
                    )
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

/// Copy fields that exist only locally from `source` onto `target`, so they
/// do not create false diffs when materializing a remote-only document.
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
    backend: &BackendConfig,
) {
    let state = state_from_backend(record);
    issue.frontmatter.title = record.title.clone();
    issue.frontmatter.status = state.clone();
    issue.frontmatter.labels = labels_without_status(&record.labels);
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
    match backend.backend {
        Backend::Github => {
            issue.frontmatter.github = Some(GithubIssueMeta {
                repo: backend.repo.clone().unwrap_or_default(),
                issue_id: Some(record.issue_id),
                node_id: record.node_id.clone(),
                milestone_id: record.milestone_id,
                url: Some(record.url.clone()),
                updated_at: record.updated_at.clone(),
                last_pushed_state: Some(state),
            });
        }
        Backend::Gitlab => {
            issue.frontmatter.gitlab = Some(GitlabIssueMeta {
                repo: backend.repo.clone().unwrap_or_default(),
                issue_id: Some(record.issue_id),
                milestone_id: record.milestone_id,
                url: Some(record.url.clone()),
                updated_at: record.updated_at.clone(),
                last_pushed_state: Some(state),
            });
        }
        Backend::Jira => {
            let issue_key = extract_jira_key_from_url(&record.url);
            issue.frontmatter.jira = Some(JiraIssueMeta {
                project_key: crate::adapters::jira::JiraProvider::project_key(
                    backend.repo.as_deref().unwrap_or_default(),
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
        Backend::Local => {}
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

fn resolve_github_token(backend_name: &str) -> Result<(String, CredentialSource), RiptskError> {
    if let Some(token) = non_empty_env("GITHUB_TOKEN") {
        return Ok((token, CredentialSource::EnvVar("GITHUB_TOKEN")));
    }
    if let Some(token) = non_empty_env("GH_TOKEN") {
        return Ok((token, CredentialSource::EnvVar("GH_TOKEN")));
    }

    match run_gh_auth_token("github.com") {
        Ok(token) => Ok((token, CredentialSource::CliTool("gh auth token"))),
        Err(reason) => Err(RiptskError::Auth(format!(
            "GitHub authentication failed for backend '{backend_name}'\n\n\
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
    backend_name: &str,
    host: &str,
) -> Result<(String, CredentialSource), RiptskError> {
    if let Some(token) = non_empty_env("GITLAB_TOKEN") {
        return Ok((token, CredentialSource::EnvVar("GITLAB_TOKEN")));
    }

    match run_glab_auth_token(host) {
        Ok(token) => Ok((
            token,
            CredentialSource::CliTool("glab auth status --show-token"),
        )),
        Err(reason) => Err(RiptskError::Auth(format!(
            "GitLab authentication failed for backend '{backend_name}' (host: {host})\n\n\
Tried: GITLAB_TOKEN, glab auth status --show-token\n\n\
{reason}\n\n\
To fix, do one of:\n\
  • export GITLAB_TOKEN=<your-token>\n\
  • glab auth login --hostname {host}"
        ))),
    }
}

fn resolve_jira_credentials(
    backend_name: &str,
    host: &str,
) -> Result<(crate::adapters::jira::JiraAuth, CredentialSource), RiptskError> {
    use crate::adapters::jira::JiraAuth;

    // 1. Check JIRA_API_TOKEN + JIRA_EMAIL (Cloud basic auth)
    if let Some(token) = non_empty_env("JIRA_API_TOKEN") {
        if let Some(email) = non_empty_env("JIRA_EMAIL") {
            return Ok((
                JiraAuth::Basic { email, token },
                CredentialSource::EnvVar("JIRA_API_TOKEN+JIRA_EMAIL"),
            ));
        }
        // Token without email = PAT auth (Server/DC)
        return Ok((
            JiraAuth::Pat(token),
            CredentialSource::EnvVar("JIRA_API_TOKEN"),
        ));
    }

    // 2. Try jira-cli-go config + system keychain
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
            return Ok((auth, CredentialSource::CliTool("jira-cli-go keychain")));
        }
        Err(_reason) => {}
    }

    // 3. All failed
    Err(RiptskError::Auth(format!(
        "Jira authentication failed for backend '{backend_name}' (host: {host})\n\n\
Tried: JIRA_API_TOKEN, jira-cli-go keychain\n\n\
To fix, do one of:\n\
  • export JIRA_API_TOKEN=<token> JIRA_EMAIL=<email>\n\
  • brew install ankitpokhrel/jira-cli/jira-cli && jira init"
    )))
}

/// Returns `(login, token, is_bearer)`. `is_bearer` is true for Server/DC PAT auth.
fn run_jira_cli_go_auth(host: &str) -> Result<(String, String, bool), String> {
    // Parse ~/.config/.jira/.config.yml
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

    // Validate the server matches the expected host
    let server_trimmed = server.trim_end_matches('/');
    let host_trimmed = host.trim_end_matches('/');
    if !server_trimmed.eq_ignore_ascii_case(host_trimmed) {
        return Err(format!(
            "jira-cli config server '{server}' does not match expected host '{host}'"
        ));
    }

    // Read API token from system keychain
    let entry =
        keyring::Entry::new("jira-cli", &login).map_err(|e| format!("keyring error: {e}"))?;
    let token = entry
        .get_password()
        .map_err(|e| format!("cannot read jira-cli token from keychain: {e}"))?;

    // Detect auth type: "bearer" means Server/DC PAT, otherwise Cloud basic
    let auth_type_env = std::env::var("JIRA_AUTH_TYPE").ok();
    let is_bearer = config
        .auth_type
        .as_deref()
        .or(auth_type_env.as_deref())
        .is_some_and(|t| t.eq_ignore_ascii_case("bearer"));

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

/// Drop `state_reason` values that are invalid for the target state.
///
/// GitHub allows `completed` / `not_planned` / `duplicate` when closing
/// and `reopened` when opening. Sending a mismatched reason triggers a
/// 422 validation error.
fn sanitize_state_reason(state: &str, reason: Option<&str>) -> Option<String> {
    let reason = reason?;
    let valid = match state {
        "closed" => matches!(reason, "completed" | "not_planned" | "duplicate"),
        "open" => matches!(reason, "reopened"),
        _ => true,
    };
    if valid { Some(reason.to_owned()) } else { None }
}

/// Extract the Jira issue key from a browse URL like `https://host/browse/PROJ-123`.
fn extract_jira_key_from_url(url: &str) -> Option<String> {
    url.rsplit('/').next().map(|s| s.to_owned())
}

fn labels_without_status(labels: &[String]) -> Vec<String> {
    labels
        .iter()
        .filter(|label| !label.starts_with("status::"))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        copy_local_only_fields, non_empty_env, parse_glab_token, resolve_git_auth,
        sanitize_state_reason,
    };
    use crate::domain::issue::{IssueFrontmatter, IssueState, Priority};
    use crate::models::{Backend, BackendConfig};
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct EnvGuard {
        name: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn unset(name: &'static str) -> Self {
            let original = std::env::var(name).ok();
            // SAFETY: these tests serialize environment mutation with ENV_LOCK.
            unsafe {
                std::env::remove_var(name);
            }
            Self { name, original }
        }

        fn set(name: &'static str, value: &str) -> Self {
            let original = std::env::var(name).ok();
            // SAFETY: these tests serialize environment mutation with ENV_LOCK.
            unsafe {
                std::env::set_var(name, value);
            }
            Self { name, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => {
                    // SAFETY: these tests serialize environment mutation with ENV_LOCK.
                    unsafe {
                        std::env::set_var(self.name, value);
                    }
                }
                None => {
                    // SAFETY: these tests serialize environment mutation with ENV_LOCK.
                    unsafe {
                        std::env::remove_var(self.name);
                    }
                }
            }
        }
    }

    #[test]
    fn non_empty_env_returns_none_for_unset_var() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::unset("RIPTSK_TEST_NON_EMPTY_ENV");

        assert_eq!(non_empty_env("RIPTSK_TEST_NON_EMPTY_ENV"), None);
    }

    #[test]
    fn non_empty_env_returns_none_for_empty_string() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::set("RIPTSK_TEST_NON_EMPTY_ENV", "");

        assert_eq!(non_empty_env("RIPTSK_TEST_NON_EMPTY_ENV"), None);
    }

    #[test]
    fn non_empty_env_returns_none_for_whitespace_only() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::set("RIPTSK_TEST_NON_EMPTY_ENV", "   \t  ");

        assert_eq!(non_empty_env("RIPTSK_TEST_NON_EMPTY_ENV"), None);
    }

    #[test]
    fn non_empty_env_returns_some_for_valid_value() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _guard = EnvGuard::set("RIPTSK_TEST_NON_EMPTY_ENV", "token-123");

        assert_eq!(
            non_empty_env("RIPTSK_TEST_NON_EMPTY_ENV"),
            Some("token-123".into())
        );
    }

    #[test]
    fn parse_glab_token_extracts_token_from_multiline_output() {
        let stderr_output = "gitlab.suse.de\n  ✓ Logged in to gitlab.suse.de as user\n  ✓ Token found: glpat-xxxx\n  ✓ REST API Endpoint: https://gitlab.suse.de/api/v4/";

        assert_eq!(parse_glab_token(stderr_output), Some("glpat-xxxx".into()));
    }

    #[test]
    fn parse_glab_token_returns_none_without_token_line() {
        let stderr_output = "gitlab.com\n  ✓ Logged in\n  ✓ API calls: 123";

        assert_eq!(parse_glab_token(stderr_output), None);
    }

    #[test]
    fn parse_glab_token_handles_plain_token_found_line() {
        let stderr_output = "gitlab.com\nToken found: glpat-xxxx";

        assert_eq!(parse_glab_token(stderr_output), Some("glpat-xxxx".into()));
    }

    #[test]
    fn parse_glab_token_falls_back_to_token_prefix() {
        let stderr_output = "gitlab.com\nToken: glpat-xxxx";

        assert_eq!(parse_glab_token(stderr_output), Some("glpat-xxxx".into()));
    }

    #[test]
    fn parse_glab_token_returns_none_for_empty_token_value() {
        let stderr_output = "Token found:  ";

        assert_eq!(parse_glab_token(stderr_output), None);
    }

    #[test]
    fn resolve_git_auth_returns_none_for_local_backend() {
        let backend = BackendConfig {
            name: "local".into(),
            backend: Backend::Local,
            host: None,
            repo: None,
            default_board: None,
            default_org: None,
            path: None,
            vc: None,
            default_issue_type: None,
        };

        assert!(resolve_git_auth(&backend).is_none());
    }

    #[test]
    fn copy_local_only_fields_copies_all_fields() {
        let source = frontmatter_fixture("SRC", "Source title");
        let mut target = frontmatter_fixture("DST", "Target title");
        target.pr_url = None;
        target.pr_number = None;
        target.branch = None;
        target.order = Some(1);
        target.recurring = None;
        target.cycle = None;
        target.board = "personal".into();
        target.org = None;
        target.priority = Some(Priority::Medium);
        let original_title = target.title.clone();
        let original_status = target.status.clone();
        let original_labels = target.labels.clone();

        copy_local_only_fields(&mut target, &source);

        assert_eq!(
            target.pr_url.as_deref(),
            Some("https://example.invalid/pulls/42")
        );
        assert_eq!(target.pr_number, Some(42));
        assert_eq!(target.branch.as_deref(), Some("feature/42"));
        assert_eq!(target.order, Some(7));
        assert_eq!(target.recurring.as_deref(), Some("weekly-42"));
        assert_eq!(target.cycle.as_deref(), Some("2026-W13"));
        assert_eq!(target.board, "ops");
        assert_eq!(target.org.as_deref(), Some("eng"));
        assert_eq!(target.priority, Some(Priority::High));
        assert_eq!(target.title, original_title);
        assert_eq!(target.status, original_status);
        assert_eq!(target.labels, original_labels);
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
    fn sanitize_state_reason_drops_reopened_when_closing() {
        assert_eq!(sanitize_state_reason("closed", Some("reopened")), None);
    }

    #[test]
    fn sanitize_state_reason_allows_completed_when_closing() {
        assert_eq!(
            sanitize_state_reason("closed", Some("completed")),
            Some("completed".into())
        );
    }

    #[test]
    fn sanitize_state_reason_allows_not_planned_when_closing() {
        assert_eq!(
            sanitize_state_reason("closed", Some("not_planned")),
            Some("not_planned".into())
        );
    }

    #[test]
    fn sanitize_state_reason_allows_duplicate_when_closing() {
        assert_eq!(
            sanitize_state_reason("closed", Some("duplicate")),
            Some("duplicate".into())
        );
    }

    #[test]
    fn sanitize_state_reason_drops_completed_when_opening() {
        assert_eq!(sanitize_state_reason("open", Some("completed")), None);
    }

    #[test]
    fn sanitize_state_reason_allows_reopened_when_opening() {
        assert_eq!(
            sanitize_state_reason("open", Some("reopened")),
            Some("reopened".into())
        );
    }

    #[test]
    fn sanitize_state_reason_passes_through_none() {
        assert_eq!(sanitize_state_reason("closed", None), None);
    }

    #[test]
    fn resolve_git_auth_returns_none_for_jira_backend() {
        let backend = BackendConfig {
            name: "jira-test".into(),
            backend: Backend::Jira,
            host: Some("https://test.atlassian.net".into()),
            repo: Some("org/PROJ".into()),
            default_board: None,
            default_org: None,
            path: None,
            vc: None,
            default_issue_type: None,
        };

        assert!(resolve_git_auth(&backend).is_none());
    }

    #[test]
    fn resolve_jira_credentials_with_env_vars() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _token = EnvGuard::set("JIRA_API_TOKEN", "test-token");
        let _email = EnvGuard::set("JIRA_EMAIL", "test@example.com");

        let result = super::resolve_jira_credentials("test-backend", "https://test.atlassian.net");
        assert!(result.is_ok());
        let (auth, source) = result.unwrap();
        match auth {
            crate::adapters::jira::JiraAuth::Basic { email, token } => {
                assert_eq!(email, "test@example.com");
                assert_eq!(token, "test-token");
            }
            _ => panic!("expected Basic auth"),
        }
        assert_eq!(source.to_string(), "JIRA_API_TOKEN+JIRA_EMAIL");
    }

    #[test]
    fn resolve_jira_credentials_pat_without_email() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _token = EnvGuard::set("JIRA_API_TOKEN", "pat-token");
        let _email = EnvGuard::unset("JIRA_EMAIL");

        let result = super::resolve_jira_credentials("test-backend", "https://test.atlassian.net");
        assert!(result.is_ok());
        let (auth, source) = result.unwrap();
        match auth {
            crate::adapters::jira::JiraAuth::Pat(token) => {
                assert_eq!(token, "pat-token");
            }
            _ => panic!("expected PAT auth"),
        }
        assert_eq!(source.to_string(), "JIRA_API_TOKEN");
    }

    #[test]
    fn resolve_jira_credentials_fails_without_any() {
        let _lock = ENV_LOCK.lock().expect("env lock");
        let _token = EnvGuard::unset("JIRA_API_TOKEN");
        let _email = EnvGuard::unset("JIRA_EMAIL");

        let result = super::resolve_jira_credentials("test-backend", "https://test.atlassian.net");
        assert!(result.is_err());
        let error = result.unwrap_err().to_string();
        assert!(error.contains("Jira authentication failed"));
        assert!(error.contains("JIRA_API_TOKEN"));
    }

    #[test]
    fn resolve_vc_for_jira_without_vc_returns_none() {
        let backend = BackendConfig {
            name: "jira-test".into(),
            backend: Backend::Jira,
            host: Some("https://test.atlassian.net".into()),
            repo: Some("org/PROJ".into()),
            default_board: None,
            default_org: None,
            path: None,
            vc: None,
            default_issue_type: None,
        };
        let config = crate::config::Config {
            backends: vec![backend.clone()],
            ..crate::config::default_config()
        };

        let result = super::resolve_vc_for_backend(&backend, &config);
        // Will fail at auth (no credentials), but that's expected in tests.
        // The important thing is the logic path.
        // For a test with no credentials available, the resolve will error.
        // Let's just test the None case with a Jira backend that has no vc field.
        // resolve_vc_for_backend checks backend.vc first, then falls through to
        // backend.backend match. For Jira, the _ arm returns None.
        // But it calls build_version_control first for Github/Gitlab...
        // Actually for Jira without vc, it hits the _ => Ok(None) arm directly.
        // No credential resolution happens.
        match result {
            Ok(None) => {} // Expected: Jira without vc returns None
            Ok(Some(_)) => panic!("expected None for Jira without vc"),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn resolve_vc_with_invalid_reference_errors() {
        let backend = BackendConfig {
            name: "jira-test".into(),
            backend: Backend::Jira,
            host: Some("https://test.atlassian.net".into()),
            repo: Some("org/PROJ".into()),
            default_board: None,
            default_org: None,
            path: None,
            vc: Some("nonexistent".into()),
            default_issue_type: None,
        };
        let config = crate::config::Config {
            backends: vec![backend.clone()],
            ..crate::config::default_config()
        };

        let result = super::resolve_vc_for_backend(&backend, &config);
        match result {
            Err(e) => assert!(e.to_string().contains("does not exist"), "got: {e}"),
            Ok(_) => panic!("expected error for nonexistent vc reference"),
        }
    }

    #[test]
    fn resolve_vc_rejects_jira_as_vc_target() {
        let jira_backend = BackendConfig {
            name: "jira-issues".into(),
            backend: Backend::Jira,
            host: Some("https://test.atlassian.net".into()),
            repo: Some("org/PROJ".into()),
            default_board: None,
            default_org: None,
            path: None,
            vc: Some("jira-other".into()),
            default_issue_type: None,
        };
        let jira_other = BackendConfig {
            name: "jira-other".into(),
            backend: Backend::Jira,
            host: Some("https://other.atlassian.net".into()),
            repo: Some("org/OTHER".into()),
            default_board: None,
            default_org: None,
            path: None,
            vc: None,
            default_issue_type: None,
        };
        let config = crate::config::Config {
            backends: vec![jira_backend.clone(), jira_other],
            ..crate::config::default_config()
        };

        let result = super::resolve_vc_for_backend(&jira_backend, &config);
        match result {
            Err(e) => assert!(
                e.to_string().contains("must be a github or gitlab backend"),
                "got: {e}"
            ),
            Ok(_) => panic!("expected error for Jira as vc target"),
        }
    }
}
