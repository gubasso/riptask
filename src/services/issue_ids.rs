use crate::error::{ProjectKeyCollision, ProjectKeyProjectMeta, RiptskError};
use crate::models::{BackendKind, RepoProject};
use crate::paths::AppPaths;
use crate::storage::{frontmatter, issue_store};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub(crate) const MAX_KEY_LEN: usize = 16;
pub(crate) const FALLBACK_KEY: &str = "TSK";

pub fn derive_default_key(repo_project: &RepoProject) -> String {
    let source = repo_project
        .tasks_backend
        .jira_project
        .as_deref()
        .and_then(|jira_project| jira_project.rsplit('/').next())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            repo_project
                .vc_backend
                .repo
                .as_deref()
                .and_then(|repo| repo.rsplit('/').next())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(repo_project.name.as_str());
    sanitize_key_candidate(source)
}

pub(crate) fn sanitize_key_candidate(raw: &str) -> String {
    let mut buffer = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            for upper in character.to_uppercase() {
                buffer.push(upper);
            }
        }
    }
    if buffer.is_empty() {
        return FALLBACK_KEY.to_owned();
    }
    if buffer.len() > MAX_KEY_LEN {
        buffer.truncate(MAX_KEY_LEN);
    }
    buffer
}

pub fn effective_key(repo_project: &RepoProject) -> String {
    match repo_project.key.as_deref() {
        Some(key) if !key.is_empty() => key.to_owned(),
        _ => derive_default_key(repo_project),
    }
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

/// Populate `repo_project.key` on every group member so that all RepoProjects pointing
/// at the same logical unit resolve to the same effective key at runtime.
pub fn normalize_backend_keys(repo_projects: &mut [RepoProject]) -> Result<(), RiptskError> {
    let mut canonical: HashMap<String, String> = HashMap::new();
    let mut errors: Option<(String, Vec<String>)> = None;
    {
        let mut groups: HashMap<String, Vec<&RepoProject>> = HashMap::new();
        for repo_project in repo_projects.iter() {
            groups
                .entry(logical_group_identity(repo_project))
                .or_default()
                .push(repo_project);
        }
        for members in groups.values() {
            let explicit: HashSet<String> = members
                .iter()
                .filter_map(|repo_project| repo_project.key.as_deref())
                .map(str::to_owned)
                .collect();
            if explicit.len() > 1 {
                let representative_name = members
                    .first()
                    .map(|repo_project| repo_project.name.clone())
                    .unwrap_or_default();
                errors = Some((representative_name, explicit.into_iter().collect()));
                break;
            }
            let key = explicit.into_iter().next().unwrap_or_else(|| {
                members
                    .first()
                    .copied()
                    .map(derive_default_key)
                    .unwrap_or_else(|| FALLBACK_KEY.to_owned())
            });
            canonical.insert(logical_group_identity(members[0]), key);
        }
    }
    if let Some((name, keys)) = errors {
        return Err(RiptskError::Config(format!(
            "logical RepoProject '{}' declares multiple keys: {}",
            name,
            keys.join(", ")
        )));
    }
    for repo_project in repo_projects.iter_mut() {
        let group_id = logical_group_identity(repo_project);
        if let Some(key) = canonical.get(&group_id) {
            repo_project.key = Some(key.clone());
        }
    }
    Ok(())
}

pub fn validate_no_key_collisions(
    repo_projects: &[RepoProject],
) -> std::result::Result<(), RiptskError> {
    let mut groups: HashMap<String, Vec<&RepoProject>> = HashMap::new();
    for repo_project in repo_projects {
        groups
            .entry(logical_group_identity(repo_project))
            .or_default()
            .push(repo_project);
    }

    let mut seen: HashMap<String, &RepoProject> = HashMap::new();
    for members in groups.values() {
        let representative = members
            .first()
            .copied()
            .ok_or_else(|| RiptskError::Config("empty RepoProject group".into()))?;
        let explicit_keys = members
            .iter()
            .filter_map(|repo_project| repo_project.key.as_deref())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        if explicit_keys.len() > 1 {
            return Err(RiptskError::Config(format!(
                "logical RepoProject '{}' declares multiple keys: {}",
                representative.name,
                explicit_keys.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
        let key = explicit_keys
            .into_iter()
            .next()
            .unwrap_or_else(|| effective_key(representative));
        if let Some(existing) = seen.insert(key.clone(), representative) {
            return Err(RiptskError::KeyCollision(Box::new(ProjectKeyCollision {
                attempted_key: key,
                new_project: project_meta(representative),
                conflicting_project: project_meta(existing),
            })));
        }
    }
    Ok(())
}

pub fn validate_explicit_key_syntax(key: &str) -> Result<(), String> {
    if key != key.trim() {
        return Err(format!(
            "key '{key}' must not contain leading or trailing whitespace"
        ));
    }
    let trimmed = key;
    if trimmed.is_empty() {
        return Err("key cannot be empty".into());
    }
    if trimmed.len() > MAX_KEY_LEN {
        return Err(format!(
            "key '{trimmed}' is longer than {MAX_KEY_LEN} characters"
        ));
    }
    if trimmed.contains("--") {
        return Err(format!("key '{trimmed}' must not contain '--'"));
    }
    if !trimmed.chars().all(|character| {
        character.is_ascii_uppercase() || character.is_ascii_digit() || character == '-'
    }) {
        return Err(format!("key '{trimmed}' must match [A-Z0-9-]+"));
    }
    if trimmed.starts_with('-') || trimmed.ends_with('-') {
        return Err(format!("key '{trimmed}' must not start or end with '-'"));
    }
    Ok(())
}

pub(crate) fn logical_group_identity(repo_project: &RepoProject) -> String {
    if repo_project.tasks_backend.kind == BackendKind::Jira
        && repo_project.tasks_backend.jira_project.is_some()
    {
        return format!(
            "jira|{}|{}",
            normalize_host(repo_project.tasks_backend.host.as_deref(), ""),
            repo_project
                .tasks_backend
                .jira_project
                .as_deref()
                .unwrap_or_default()
        );
    }

    match repo_project.vc_backend.kind {
        BackendKind::Github => format!(
            "github|{}|{}",
            normalize_host(repo_project.vc_backend.host.as_deref(), "github.com"),
            repo_project.vc_backend.repo.as_deref().unwrap_or_default()
        ),
        BackendKind::Gitlab => format!(
            "gitlab|{}|{}",
            normalize_host(repo_project.vc_backend.host.as_deref(), "gitlab.com"),
            repo_project.vc_backend.repo.as_deref().unwrap_or_default()
        ),
        _ => format!(
            "local|{}",
            repo_project
                .vc_backend
                .path
                .as_deref()
                .or(repo_project.tasks_backend.path.as_deref())
                .unwrap_or(repo_project.name.as_str())
        ),
    }
}

fn normalize_host(host: Option<&str>, default_host: &str) -> String {
    host.unwrap_or(default_host)
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned()
}

fn project_meta(repo_project: &RepoProject) -> ProjectKeyProjectMeta {
    ProjectKeyProjectMeta {
        name: repo_project.name.clone(),
        backend: repo_project.tasks_backend.kind.as_str().to_owned(),
        host: repo_project
            .tasks_backend
            .host
            .clone()
            .or_else(|| repo_project.vc_backend.host.clone()),
        repo: repo_project
            .tasks_backend
            .repo
            .clone()
            .or_else(|| repo_project.tasks_backend.jira_project.clone())
            .or_else(|| repo_project.vc_backend.repo.clone()),
        path: repo_project
            .vc_backend
            .path
            .clone()
            .or_else(|| repo_project.tasks_backend.path.clone()),
        existing_key: repo_project.key.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_KEY_LEN, derive_default_key, effective_key, normalize_backend_keys,
        validate_explicit_key_syntax, validate_no_key_collisions,
    };
    use crate::error::RiptskError;
    use crate::models::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};

    fn repo_project(
        vc_kind: BackendKind,
        tasks_kind: BackendKind,
        name: &str,
        vc_repo: Option<&str>,
        jira_project: Option<&str>,
        key: Option<&str>,
    ) -> RepoProject {
        RepoProject {
            name: name.into(),
            vc_backend: VCBackendSpec {
                kind: vc_kind,
                host: None,
                repo: vc_repo.map(str::to_owned),
                path: None,
            },
            tasks_backend: TasksBackendSpec {
                kind: tasks_kind,
                host: None,
                repo: vc_repo.map(str::to_owned),
                jira_project: jira_project.map(str::to_owned),
                default_issue_type: None,
                path: None,
            },
            default_board: None,
            default_org: None,
            key: key.map(str::to_owned),
            repo_project_label: None,
        }
    }

    #[test]
    fn derive_default_key_uses_github_repo_tail() {
        let repo_project = repo_project(
            BackendKind::Github,
            BackendKind::Github,
            "fallback",
            Some("owner/my-repo"),
            None,
            None,
        );
        assert_eq!(derive_default_key(&repo_project), "MYREPO");
    }

    #[test]
    fn derive_default_key_uses_jira_project_tail() {
        let repo_project = repo_project(
            BackendKind::Local,
            BackendKind::Jira,
            "fallback",
            None,
            Some("org/PROJ"),
            None,
        );
        assert_eq!(derive_default_key(&repo_project), "PROJ");
    }

    #[test]
    fn derive_default_key_uses_local_name() {
        let repo_project = repo_project(
            BackendKind::Local,
            BackendKind::Local,
            "ice shelf tracker",
            None,
            None,
            None,
        );
        assert_eq!(derive_default_key(&repo_project), "ICESHELFTRACKER");
    }

    #[test]
    fn derive_default_key_strips_non_ascii_and_punctuation() {
        let repo_project = repo_project(
            BackendKind::Local,
            BackendKind::Local,
            "🦀 crab-ops",
            None,
            None,
            None,
        );
        assert_eq!(derive_default_key(&repo_project), "CRABOPS");
    }

    #[test]
    fn derive_default_key_falls_back_to_tsk() {
        let repo_project = repo_project(
            BackendKind::Local,
            BackendKind::Local,
            "——",
            None,
            None,
            None,
        );
        assert_eq!(derive_default_key(&repo_project), "TSK");
    }

    #[test]
    fn derive_default_key_truncates_to_max_key_len() {
        let repo_project = repo_project(
            BackendKind::Github,
            BackendKind::Github,
            "fallback",
            Some("owner/averylongrepositoryname"),
            None,
            None,
        );
        let derived = derive_default_key(&repo_project);
        assert_eq!(derived.len(), MAX_KEY_LEN);
        assert_eq!(derived, "AVERYLONGREPOSIT");
    }

    #[test]
    fn effective_key_prefers_explicit_override() {
        let repo_project = repo_project(
            BackendKind::Github,
            BackendKind::Github,
            "fallback",
            Some("owner/repo"),
            None,
            Some("OVERRIDE"),
        );
        assert_eq!(effective_key(&repo_project), "OVERRIDE");
    }

    #[test]
    fn effective_key_empty_override_falls_back_to_derived() {
        let repo_project = repo_project(
            BackendKind::Github,
            BackendKind::Github,
            "fallback",
            Some("owner/repo"),
            None,
            Some(""),
        );
        assert_eq!(effective_key(&repo_project), "REPO");
    }

    #[test]
    fn validate_no_key_collisions_allows_same_remote_dupes() {
        let first = repo_project(
            BackendKind::Github,
            BackendKind::Github,
            "repo-a",
            Some("owner/repo"),
            None,
            None,
        );
        let second = repo_project(
            BackendKind::Github,
            BackendKind::Github,
            "repo-b",
            Some("owner/repo"),
            None,
            None,
        );
        assert!(validate_no_key_collisions(&[first, second]).is_ok());
    }

    #[test]
    fn validate_no_key_collisions_rejects_distinct_project_collisions() {
        let first = repo_project(
            BackendKind::Github,
            BackendKind::Github,
            "alpha-one",
            Some("owner/alpha"),
            None,
            None,
        );
        let second = repo_project(
            BackendKind::Github,
            BackendKind::Github,
            "alpha-two",
            Some("other/alpha"),
            None,
            None,
        );
        let error = validate_no_key_collisions(&[first, second]).expect_err("collision");
        assert!(matches!(error, RiptskError::KeyCollision(_)));
    }

    #[test]
    fn normalize_backend_keys_aligns_same_jira_project_siblings() {
        let first = repo_project(
            BackendKind::Local,
            BackendKind::Jira,
            "a",
            None,
            Some("org/PROJ"),
            None,
        );
        let second = repo_project(
            BackendKind::Local,
            BackendKind::Jira,
            "b",
            None,
            Some("org/PROJ"),
            None,
        );
        let mut repo_projects = vec![first, second];
        normalize_backend_keys(&mut repo_projects).expect("normalize");
        assert_eq!(repo_projects[0].key, repo_projects[1].key);
    }

    #[test]
    fn validate_explicit_key_syntax_accepts_canonical_keys() {
        assert!(validate_explicit_key_syntax("ALPHA").is_ok());
        assert!(validate_explicit_key_syntax("GH-GUB-DEV").is_ok());
        assert!(validate_explicit_key_syntax("LO-PEN").is_ok());
        assert!(validate_explicit_key_syntax("PROJ9").is_ok());
    }

    #[test]
    fn validate_explicit_key_syntax_rejects_bad_inputs() {
        assert!(validate_explicit_key_syntax("").is_err());
        assert!(validate_explicit_key_syntax("lower").is_err());
        assert!(validate_explicit_key_syntax("A--B").is_err());
        assert!(validate_explicit_key_syntax("-ALPHA").is_err());
        assert!(validate_explicit_key_syntax("ALPHA-").is_err());
        assert!(validate_explicit_key_syntax("ALPHABETAGAMMADELTA").is_err());
        assert!(validate_explicit_key_syntax("A B").is_err());
        assert!(validate_explicit_key_syntax(" FOO").is_err());
        assert!(validate_explicit_key_syntax("FOO ").is_err());
        assert!(validate_explicit_key_syntax("\tFOO").is_err());
    }
}
