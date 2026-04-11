use crate::error::{ProjectKeyCollision, ProjectKeyProjectMeta, RiptskError};
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::storage::{frontmatter, issue_store};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub(crate) const MAX_KEY_LEN: usize = 16;
pub(crate) const FALLBACK_KEY: &str = "TSK";

pub fn derive_default_key(backend: &BackendConfig) -> String {
    let source = match backend.backend {
        Backend::Github | Backend::Gitlab | Backend::Jira => backend
            .repo
            .as_deref()
            .and_then(|repo| repo.rsplit('/').next())
            .filter(|value| !value.is_empty())
            .unwrap_or(backend.name.as_str()),
        Backend::Local => backend.name.as_str(),
    };
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

pub fn effective_key(backend: &BackendConfig) -> String {
    match backend.key.as_deref() {
        Some(key) if !key.is_empty() => key.to_owned(),
        _ => derive_default_key(backend),
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

/// Populate `backend.key` on every group member so that all backends pointing
/// at the same logical project resolve to the same effective key at runtime.
///
/// Preference order within a group:
/// 1. Any existing explicit key set on a member.
/// 2. The derived default key of the first member.
///
/// Returns an error if two members of the same group declare conflicting
/// explicit keys.
pub fn normalize_backend_keys(backends: &mut [BackendConfig]) -> Result<(), RiptskError> {
    // Build group -> canonical key using immutable borrows first.
    let mut canonical: HashMap<String, String> = HashMap::new();
    let mut errors: Option<(String, Vec<String>, String)> = None;
    {
        let mut groups: HashMap<String, Vec<&BackendConfig>> = HashMap::new();
        for backend in backends.iter() {
            groups
                .entry(logical_group_identity(backend))
                .or_default()
                .push(backend);
        }
        for (group_id, members) in &groups {
            let explicit: HashSet<String> = members
                .iter()
                .filter_map(|backend| backend.key.as_deref())
                .map(str::to_owned)
                .collect();
            if explicit.len() > 1 {
                let representative_name = members
                    .first()
                    .map(|backend| backend.name.clone())
                    .unwrap_or_default();
                errors = Some((
                    representative_name,
                    explicit.into_iter().collect(),
                    group_id.clone(),
                ));
                break;
            }
            let key = explicit.into_iter().next().unwrap_or_else(|| {
                members
                    .first()
                    .copied()
                    .map(derive_default_key)
                    .unwrap_or_else(|| FALLBACK_KEY.to_owned())
            });
            canonical.insert(group_id.clone(), key);
        }
    }
    if let Some((name, keys, _)) = errors {
        return Err(RiptskError::Config(format!(
            "logical project '{}' declares multiple keys: {}",
            name,
            keys.join(", ")
        )));
    }
    for backend in backends.iter_mut() {
        let group_id = logical_group_identity(backend);
        if let Some(key) = canonical.get(&group_id) {
            backend.key = Some(key.clone());
        }
    }
    Ok(())
}

pub fn validate_no_key_collisions(
    backends: &[BackendConfig],
) -> std::result::Result<(), RiptskError> {
    let mut groups: HashMap<String, Vec<&BackendConfig>> = HashMap::new();
    for backend in backends {
        groups
            .entry(logical_group_identity(backend))
            .or_default()
            .push(backend);
    }

    let mut seen: HashMap<String, &BackendConfig> = HashMap::new();
    for members in groups.values() {
        let representative = members
            .first()
            .copied()
            .ok_or_else(|| RiptskError::Config("empty backend group".into()))?;
        let explicit_keys = members
            .iter()
            .filter_map(|backend| backend.key.as_deref())
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        if explicit_keys.len() > 1 {
            return Err(RiptskError::Config(format!(
                "logical project '{}' declares multiple keys: {}",
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

/// Validate that an explicit `backend.key` value matches the same syntax the
/// interactive registration flow enforces.
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

pub(crate) fn logical_group_identity(backend: &BackendConfig) -> String {
    match backend.backend {
        Backend::Github => format!(
            "github|{}|{}",
            normalize_host(backend, "github.com"),
            backend.repo.as_deref().unwrap_or_default()
        ),
        Backend::Gitlab => format!(
            "gitlab|{}|{}",
            normalize_host(backend, "gitlab.com"),
            backend.repo.as_deref().unwrap_or_default()
        ),
        Backend::Jira if backend.repo.is_some() => format!(
            "jira|{}|{}",
            normalize_host(backend, ""),
            backend.repo.as_deref().unwrap_or_default()
        ),
        _ => format!(
            "local|{}",
            backend.path.as_deref().unwrap_or(backend.name.as_str())
        ),
    }
}

fn normalize_host(backend: &BackendConfig, default_host: &str) -> String {
    backend
        .host
        .as_deref()
        .unwrap_or(default_host)
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned()
}

fn project_meta(backend: &BackendConfig) -> ProjectKeyProjectMeta {
    ProjectKeyProjectMeta {
        name: backend.name.clone(),
        backend: backend.backend.as_str().to_owned(),
        host: backend.host.clone(),
        repo: backend.repo.clone(),
        path: backend.path.clone(),
        existing_key: backend.key.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_KEY_LEN, derive_default_key, effective_key, normalize_backend_keys,
        validate_explicit_key_syntax, validate_no_key_collisions,
    };
    use crate::error::RiptskError;
    use crate::models::{Backend, BackendConfig};

    fn backend(kind: Backend, name: &str, repo: Option<&str>, key: Option<&str>) -> BackendConfig {
        BackendConfig {
            name: name.into(),
            backend: kind,
            host: None,
            repo: repo.map(str::to_owned),
            default_board: None,
            default_org: None,
            path: None,
            vc: None,
            default_issue_type: None,
            key: key.map(str::to_owned),
        }
    }

    #[test]
    fn derive_default_key_uses_github_repo_tail() {
        let backend = backend(Backend::Github, "fallback", Some("owner/my-repo"), None);
        assert_eq!(derive_default_key(&backend), "MYREPO");
    }

    #[test]
    fn derive_default_key_uses_gitlab_repo_tail() {
        let backend = backend(
            Backend::Gitlab,
            "fallback",
            Some("group/sub/project-x"),
            None,
        );
        assert_eq!(derive_default_key(&backend), "PROJECTX");
    }

    #[test]
    fn derive_default_key_uses_jira_repo_tail() {
        let backend = backend(Backend::Jira, "fallback", Some("org/PROJ"), None);
        assert_eq!(derive_default_key(&backend), "PROJ");
    }

    #[test]
    fn derive_default_key_uses_local_name() {
        let backend = backend(Backend::Local, "ice shelf tracker", None, None);
        assert_eq!(derive_default_key(&backend), "ICESHELFTRACKER");
    }

    #[test]
    fn derive_default_key_strips_non_ascii_and_punctuation() {
        let backend = backend(Backend::Local, "🦀 crab-ops", None, None);
        assert_eq!(derive_default_key(&backend), "CRABOPS");
    }

    #[test]
    fn derive_default_key_falls_back_to_tsk() {
        let backend = backend(Backend::Local, "——", None, None);
        assert_eq!(derive_default_key(&backend), "TSK");
    }

    #[test]
    fn derive_default_key_truncates_to_max_key_len() {
        let backend = backend(
            Backend::Github,
            "fallback",
            Some("owner/averylongrepositoryname"),
            None,
        );
        let derived = derive_default_key(&backend);
        assert_eq!(derived.len(), MAX_KEY_LEN);
        assert_eq!(derived, "AVERYLONGREPOSIT");
    }

    #[test]
    fn effective_key_prefers_explicit_override() {
        let backend = backend(
            Backend::Github,
            "fallback",
            Some("owner/repo"),
            Some("OVERRIDE"),
        );
        assert_eq!(effective_key(&backend), "OVERRIDE");
    }

    #[test]
    fn effective_key_empty_override_falls_back_to_derived() {
        let backend = backend(Backend::Github, "fallback", Some("owner/repo"), Some(""));
        assert_eq!(effective_key(&backend), "REPO");
    }

    #[test]
    fn validate_no_key_collisions_allows_same_remote_dupes() {
        let first = backend(Backend::Github, "repo-a", Some("owner/repo"), None);
        let mut second = backend(Backend::Github, "repo-b", Some("owner/repo"), None);
        second.path = Some("/tmp/worktree".into());
        assert!(validate_no_key_collisions(&[first, second]).is_ok());
    }

    #[test]
    fn validate_no_key_collisions_allows_same_remote_explicit_match() {
        let first = backend(Backend::Github, "repo-a", Some("owner/repo"), Some("SAME"));
        let second = backend(Backend::Github, "repo-b", Some("owner/repo"), Some("SAME"));
        assert!(validate_no_key_collisions(&[first, second]).is_ok());
    }

    #[test]
    fn validate_no_key_collisions_rejects_same_remote_explicit_mismatch() {
        let first = backend(Backend::Github, "repo-a", Some("owner/repo"), Some("ONE"));
        let second = backend(Backend::Github, "repo-b", Some("owner/repo"), Some("TWO"));
        let error = validate_no_key_collisions(&[first, second]).expect_err("mismatch");
        assert!(matches!(error, RiptskError::Config(_)));
    }

    #[test]
    fn validate_no_key_collisions_allows_distinct_defaults() {
        let first = backend(Backend::Github, "alpha", Some("owner/alpha"), None);
        let second = backend(Backend::Github, "beta", Some("owner/beta"), None);
        assert!(validate_no_key_collisions(&[first, second]).is_ok());
    }

    #[test]
    fn validate_no_key_collisions_rejects_distinct_project_collisions() {
        let first = backend(Backend::Github, "alpha-one", Some("owner/alpha"), None);
        let second = backend(Backend::Github, "alpha-two", Some("owner2/alpha"), None);
        let error = validate_no_key_collisions(&[first, second]).expect_err("collision");
        match error {
            RiptskError::KeyCollision(collision) => {
                assert_eq!(collision.attempted_key, "ALPHA");
                let names = [
                    collision.conflicting_project.name,
                    collision.new_project.name,
                ];
                assert!(names.contains(&"alpha-one".into()));
                assert!(names.contains(&"alpha-two".into()));
            }
            other => panic!("expected key collision, got {other}"),
        }
    }

    #[test]
    fn validate_no_key_collisions_rejects_distinct_local_collisions() {
        let mut first = backend(Backend::Local, "proj-a", None, None);
        first.path = Some("/a".into());
        let mut second = backend(Backend::Local, "proj-a", None, None);
        second.path = Some("/b".into());
        let error = validate_no_key_collisions(&[first, second]).expect_err("collision");
        assert!(matches!(error, RiptskError::KeyCollision(_)));
    }

    #[test]
    fn validate_no_key_collisions_allows_same_local_path() {
        let mut first = backend(Backend::Local, "a", None, None);
        first.path = Some("/x".into());
        let mut second = backend(Backend::Local, "b", None, None);
        second.path = Some("/x".into());
        assert!(validate_no_key_collisions(&[first, second]).is_ok());
    }

    #[test]
    fn normalize_backend_keys_aligns_same_local_path_siblings() {
        let mut first = backend(Backend::Local, "a", None, None);
        first.path = Some("/x".into());
        let mut second = backend(Backend::Local, "b", None, None);
        second.path = Some("/x".into());
        let mut backends = vec![first, second];
        normalize_backend_keys(&mut backends).expect("normalize");
        assert_eq!(backends[0].key, backends[1].key);
        // The canonical key comes from the first member's derived default.
        assert_eq!(backends[0].key.as_deref(), Some("A"));
    }

    #[test]
    fn normalize_backend_keys_inherits_existing_explicit_key() {
        let mut first = backend(Backend::Local, "a", None, Some("ALPHA"));
        first.path = Some("/x".into());
        let mut second = backend(Backend::Local, "b", None, None);
        second.path = Some("/x".into());
        let mut backends = vec![first, second];
        normalize_backend_keys(&mut backends).expect("normalize");
        assert_eq!(backends[0].key.as_deref(), Some("ALPHA"));
        assert_eq!(backends[1].key.as_deref(), Some("ALPHA"));
    }

    #[test]
    fn normalize_backend_keys_rejects_conflicting_explicit_keys_in_same_group() {
        let mut first = backend(Backend::Local, "a", None, Some("ONE"));
        first.path = Some("/x".into());
        let mut second = backend(Backend::Local, "b", None, Some("TWO"));
        second.path = Some("/x".into());
        let mut backends = vec![first, second];
        let error = normalize_backend_keys(&mut backends).expect_err("conflict");
        assert!(matches!(error, RiptskError::Config(_)));
    }

    #[test]
    fn normalize_backend_keys_leaves_distinct_groups_alone() {
        let first = backend(Backend::Github, "alpha", Some("owner/alpha"), None);
        let second = backend(Backend::Github, "beta", Some("owner/beta"), None);
        let mut backends = vec![first, second];
        normalize_backend_keys(&mut backends).expect("normalize");
        assert_eq!(backends[0].key.as_deref(), Some("ALPHA"));
        assert_eq!(backends[1].key.as_deref(), Some("BETA"));
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
        assert!(validate_explicit_key_syntax("ALPHABETAGAMMADELTA").is_err()); // > MAX_KEY_LEN
        assert!(validate_explicit_key_syntax("A B").is_err());
        // Surrounding whitespace must be rejected so config-loaded keys and
        // interactively-entered keys follow the same rules.
        assert!(validate_explicit_key_syntax(" FOO").is_err());
        assert!(validate_explicit_key_syntax("FOO ").is_err());
        assert!(validate_explicit_key_syntax("\tFOO").is_err());
    }
}
