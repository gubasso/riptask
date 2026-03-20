use crate::config::{RemoteConfig, RemoteType};
use crate::paths::AppPaths;
use crate::storage::{frontmatter, issue_store};
use anyhow::{Result, anyhow};
use std::collections::HashMap;

pub fn derive_scope(remote_type: &RemoteType, repo: Option<&str>, name: &str) -> String {
    let server = match remote_type {
        RemoteType::Github => "GH",
        RemoteType::Gitlab => "GL",
        RemoteType::Local => "LO",
    };

    let (parent, project) = match remote_type {
        RemoteType::Github | RemoteType::Gitlab => {
            let repo_str = repo.unwrap_or(name);
            let owner = repo_str.split('/').next().unwrap_or(name);
            let project_name = repo_str.rsplit('/').next().unwrap_or(name);
            (scope_token(owner), scope_token(project_name))
        }
        RemoteType::Local => {
            let token = scope_token(name);
            (token.clone(), token)
        }
    };

    format!("{server}-{parent}-{project}")
}

pub fn derive_scope_from_remote(remote: &RemoteConfig) -> String {
    derive_scope(&remote.remote_type, remote.repo.as_deref(), &remote.name)
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
        let issue = frontmatter::load_issue(path.as_std_path())?;
        if let Some((candidate_scope, number)) = parse_id(&issue.frontmatter.id)
            && candidate_scope == scope
        {
            max = max.max(number);
        }
    }
    Ok(max + 1)
}

pub fn validate_no_scope_collisions(remotes: &[RemoteConfig]) -> Result<()> {
    let mut scopes = HashMap::new();
    for remote in remotes {
        let scope = derive_scope_from_remote(remote);
        if let Some(existing) = scopes.insert(scope.clone(), remote.name.clone()) {
            return Err(anyhow!(
                "duplicate derived issue scope in riptsk.yaml: {scope} ({existing}, {})",
                remote.name
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
