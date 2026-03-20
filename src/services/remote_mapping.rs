use crate::adapters::github::GithubProvider;
use crate::adapters::gitlab::GitlabProvider;
use crate::adapters::remote::{RemoteIssueRecord, RemoteIssueUpsert, RemoteProvider};
use crate::config::{RemoteConfig, RemoteType};
use crate::domain::issue::{
    GithubIssueMeta, GitlabIssueMeta, IssueDocument, IssueFrontmatter, IssueState, Priority,
};
use crate::error::TskError;
use crate::services::issue_ids;
use crate::services::issue_service::{generate_slug, now_utc};

pub fn build_provider_for_remote(
    remote: &RemoteConfig,
) -> Result<Box<dyn RemoteProvider>, TskError> {
    match remote.remote_type {
        RemoteType::Github => {
            let token = std::env::var("GITHUB_TOKEN")
                .ok()
                .or_else(|| std::env::var("GH_TOKEN").ok());
            Ok(Box::new(GithubProvider::new(token.as_deref())?))
        }
        RemoteType::Gitlab => {
            let token = std::env::var("GITLAB_TOKEN")
                .map_err(|_| TskError::Unreachable("missing GITLAB_TOKEN".into()))?;
            let host = remote
                .host
                .as_deref()
                .unwrap_or("https://gitlab.com")
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .to_owned();
            Ok(Box::new(GitlabProvider::new(&host, &token)?))
        }
        RemoteType::Local => Err(TskError::Config(format!(
            "remote {} is local-only",
            remote.name
        ))),
    }
}

pub fn issue_to_upsert(doc: &IssueDocument) -> RemoteIssueUpsert {
    RemoteIssueUpsert {
        title: doc.frontmatter.title.clone(),
        body: doc.body.clone(),
        labels: build_state_labels(&doc.frontmatter.state, &doc.frontmatter.labels),
        assignee: doc.frontmatter.assignee.clone(),
    }
}

pub fn remote_to_local(record: &RemoteIssueRecord, remote: &RemoteConfig) -> IssueDocument {
    let state = state_from_remote(record);
    let issue_id = issue_ids::format_id(
        &issue_ids::derive_scope_from_remote(remote),
        record.issue_id,
    );
    IssueDocument {
        frontmatter: IssueFrontmatter {
            id: issue_id.clone(),
            title: record.title.clone(),
            state: state.clone(),
            board: remote
                .default_board
                .clone()
                .unwrap_or_else(|| "personal".into()),
            project: remote.name.clone(),
            org: remote.default_org.clone(),
            priority: Some(Priority::Medium),
            labels: labels_without_status(&record.labels),
            assignee: record.assignee.clone(),
            milestone: None,
            cycle: None,
            order: Some(1),
            gitlab: if remote.remote_type == RemoteType::Gitlab {
                Some(GitlabIssueMeta {
                    repo: remote.repo.clone().unwrap_or_default(),
                    issue_id: Some(record.issue_id),
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(state.clone()),
                })
            } else {
                None
            },
            github: if remote.remote_type == RemoteType::Github {
                Some(GithubIssueMeta {
                    repo: remote.repo.clone().unwrap_or_default(),
                    issue_id: Some(record.issue_id),
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(state.clone()),
                })
            } else {
                None
            },
            local_updated_at: record.updated_at.clone(),
            due: None,
            recurring: None,
            remote_deleted: false,
            conflict: None,
            conflict_role: None,
            conflict_parent: None,
            id_slug: Some(generate_slug(&issue_id, &record.title)),
            branch: None,
            pr_url: None,
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

pub fn update_issue_from_remote(
    issue: &mut IssueDocument,
    record: &RemoteIssueRecord,
    remote: &RemoteConfig,
) {
    let state = state_from_remote(record);
    issue.frontmatter.title = record.title.clone();
    issue.frontmatter.state = state.clone();
    issue.frontmatter.labels = labels_without_status(&record.labels);
    issue.frontmatter.assignee = record.assignee.clone();
    issue.frontmatter.local_updated_at = record.updated_at.clone();
    issue.frontmatter.id_slug = Some(generate_slug(&issue.frontmatter.id, &record.title));
    issue.body = record.body.clone().unwrap_or_default();
    match remote.remote_type {
        RemoteType::Github => {
            issue.frontmatter.github = Some(GithubIssueMeta {
                repo: remote.repo.clone().unwrap_or_default(),
                issue_id: Some(record.issue_id),
                url: Some(record.url.clone()),
                updated_at: record.updated_at.clone(),
                last_pushed_state: Some(state),
            });
        }
        RemoteType::Gitlab => {
            issue.frontmatter.gitlab = Some(GitlabIssueMeta {
                repo: remote.repo.clone().unwrap_or_default(),
                issue_id: Some(record.issue_id),
                url: Some(record.url.clone()),
                updated_at: record.updated_at.clone(),
                last_pushed_state: Some(state),
            });
        }
        RemoteType::Local => {}
    }
}

pub fn remote_state_entry(
    record: &RemoteIssueRecord,
) -> crate::domain::remote_state::RemoteStateEntry {
    crate::domain::remote_state::RemoteStateEntry {
        title: record.title.clone(),
        state: record.state.clone(),
        labels: record.labels.clone(),
        assignee: record.assignee.clone(),
        updated_at: record.updated_at.clone(),
    }
}

pub fn current_timestamp() -> String {
    now_utc()
}

fn state_from_remote(record: &RemoteIssueRecord) -> IssueState {
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
