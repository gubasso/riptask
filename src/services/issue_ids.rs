use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::storage::{frontmatter, issue_store};
use anyhow::{Result, anyhow};
use std::collections::HashMap;

pub fn derive_scope(backend: &Backend, repo: Option<&str>, name: &str) -> String {
    let server = match backend {
        Backend::Github => "GH",
        Backend::Gitlab => "GL",
        Backend::Jira => "JR",
        Backend::Local => "LO",
    };

    let (parent, project) = match backend {
        Backend::Github | Backend::Gitlab | Backend::Jira => {
            let repo_str = repo.unwrap_or(name);
            let owner = repo_str.split('/').next().unwrap_or(name);
            let project_name = repo_str.rsplit('/').next().unwrap_or(name);
            (scope_token(owner), scope_token(project_name))
        }
        Backend::Local => {
            return format!("{server}-{}", scope_token(name));
        }
    };

    format!("{server}-{parent}-{project}")
}

pub fn derive_scope_from_backend(backend: &BackendConfig) -> String {
    derive_scope(&backend.backend, backend.repo.as_deref(), &backend.name)
}

pub fn format_id(scope: &str, number: u64) -> String {
    format!("{scope}--{number}")
}

pub fn parse_id(id: &str) -> Option<(&str, u64)> {
    let (scope, number) = id.rsplit_once("--")?;
    let parsed = number.parse().ok()?;
    Some((scope, parsed))
}

pub fn next_local_sequence(paths: &AppPaths, scope: &str) -> Result<u64> {
    let mut max = 0u64;
    for path in issue_store::list_issues(paths)? {
        let issue = match frontmatter::try_load_issue(path.as_std_path()) {
            frontmatter::IssueLoadResult::Ok(issue) => issue,
            frontmatter::IssueLoadResult::Conflict { .. } => continue,
            frontmatter::IssueLoadResult::Err(error) => return Err(error),
        };
        if let Some((candidate_scope, number)) = parse_id(&issue.frontmatter.id)
            && candidate_scope == scope
        {
            max = max.max(number);
        }
    }
    Ok(max + 1)
}

pub fn validate_no_scope_collisions(backends: &[BackendConfig]) -> Result<()> {
    let mut scopes = HashMap::new();
    for backend in backends {
        let scope = derive_scope_from_backend(backend);
        if let Some(existing) = scopes.insert(scope.clone(), backend.name.clone()) {
            return Err(anyhow!(
                "duplicate derived issue scope in riptsk.yaml: {scope} ({existing}, {})",
                backend.name
            ));
        }
    }
    Ok(())
}

fn scope_token(value: &str) -> String {
    let token = value
        .chars()
        .filter(|character| character.is_ascii_alphabetic())
        .map(|character| character.to_ascii_uppercase())
        .take(3)
        .collect::<String>();
    if token.is_empty() {
        "TSK".into()
    } else {
        token
    }
}
