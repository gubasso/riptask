use crate::adapters::backend::{BackendIssueRecord, BackendIssueUpsert, BackendProvider};
use crate::adapters::github::GithubProvider;
use crate::adapters::gitlab::GitlabProvider;
use crate::domain::issue::{
    GithubIssueMeta, GitlabIssueMeta, IssueDocument, IssueFrontmatter, IssueState, Priority,
};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::services::issue_ids;
use crate::services::issue_service::{generate_slug, now_utc};
use std::process::Command;

enum CredentialSource {
    EnvVar(&'static str),
    CliTool(&'static str),
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
        Backend::Local => Err(RiptskError::Config(format!(
            "backend {} is local-only",
            backend.name
        ))),
    }
}

pub fn issue_to_upsert(doc: &IssueDocument) -> BackendIssueUpsert {
    BackendIssueUpsert {
        title: doc.frontmatter.title.clone(),
        body: doc.body.clone(),
        state: Some(match doc.frontmatter.state {
            IssueState::Done => "closed".into(),
            _ => "open".into(),
        }),
        state_reason: doc.frontmatter.state_reason.clone(),
        labels: build_state_labels(&doc.frontmatter.state, &doc.frontmatter.labels),
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
            state: state.clone(),
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
            conflict: None,
            conflict_role: None,
            conflict_parent: None,
            id_slug: Some(generate_slug(&issue_id, &record.title)),
            branch: None,
            pr_url: None,
            pr_number: None,
        },
        body: record.body.clone().unwrap_or_default(),
        remote_section: None,
    }
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
    issue.frontmatter.state = state.clone();
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

fn run_cli_token(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = match Command::new(cmd).args(args).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("{cmd} not found on PATH"));
        }
        Err(error) => return Err(format!("{cmd} failed to run: {error}")),
    };

    if !output.status.success() {
        let status = output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".into());
        return Err(format!("{cmd} exited with status {status}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if stdout.is_empty() {
        return Err(format!("{cmd} returned empty output"));
    }

    Ok(stdout)
}

fn parse_glab_token(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let normalized = line
            .trim()
            .trim_start_matches(|ch: char| !ch.is_alphanumeric());
        normalized
            .strip_prefix("Token:")
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

    match run_cli_token("gh", &["auth", "token", "--hostname", "github.com"]) {
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

    let cli_output = run_cli_token(
        "glab",
        &["auth", "status", "--show-token", "--hostname", host],
    );
    match cli_output {
        Ok(stdout) => match parse_glab_token(&stdout) {
            Some(token) => Ok((
                token,
                CredentialSource::CliTool("glab auth status --show-token"),
            )),
            None => Err(RiptskError::Auth(format!(
                "GitLab authentication failed for backend '{backend_name}' (host: {host})\n\n\
Tried: GITLAB_TOKEN, glab auth status --show-token\n\n\
glab returned no Token line\n\n\
To fix, do one of:\n\
  • export GITLAB_TOKEN=<your-token>\n\
  • glab auth login --hostname {host}"
            ))),
        },
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
                _ => IssueState::Todo,
            };
        }
    }
    IssueState::Todo
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
    use super::{non_empty_env, parse_glab_token};
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
        let stdout = "gitlab.com\n  ✓ Logged in\n  ✓ Token: glpat-xxxx\n  ✓ API calls: 123";

        assert_eq!(parse_glab_token(stdout), Some("glpat-xxxx".into()));
    }

    #[test]
    fn parse_glab_token_returns_none_without_token_line() {
        let stdout = "gitlab.com\n  ✓ Logged in\n  ✓ API calls: 123";

        assert_eq!(parse_glab_token(stdout), None);
    }

    #[test]
    fn parse_glab_token_handles_plain_token_line() {
        let stdout = "gitlab.com\nToken: glpat-xxxx";

        assert_eq!(parse_glab_token(stdout), Some("glpat-xxxx".into()));
    }
}
