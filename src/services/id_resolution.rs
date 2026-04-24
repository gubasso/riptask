use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::picker::{FzfPicker, IssueDisplayMode, Picker, format_issue_plain};
use crate::cli::{LsArgs, ScopeArgs};
use crate::config::Config;
use crate::error::{RiptaskError, StoreError};
use crate::paths::AppPaths;
use crate::services::{issue_ids, issue_service::IssueService, project_detection};
use crate::storage::{frontmatter, issue_store};
use camino::Utf8Path;

pub fn resolve_id(
    paths: &AppPaths,
    config: &Config,
    cwd: &Utf8Path,
    input: &str,
) -> Result<String, RiptaskError> {
    if input.contains("--") {
        return Ok(input.to_owned());
    }

    if !input.chars().all(|character| character.is_ascii_digit()) {
        return Ok(input.to_owned());
    }

    let number = input
        .parse::<u64>()
        .map_err(|_| RiptaskError::General(format!("invalid numeric issue ID: {input}")))?;
    let mut detected_candidate = None;

    if let Some(backend) = project_detection::detect_from_cwd(cwd, config)? {
        let scope = issue_ids::effective_key(backend);
        let full_id = issue_ids::format_id(&scope, number);
        if crate::storage::issue_store::find_issue(paths, &full_id).is_ok() {
            return Ok(full_id);
        }
        detected_candidate = Some(full_id);
    }

    let suffix = format!("--{number}");
    let mut matches = crate::storage::issue_store::list_issues(paths)?
        .into_iter()
        .filter_map(|path| {
            path.file_stem()
                .map(str::to_owned)
                .filter(|id| id.ends_with(&suffix))
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();

    match matches.len() {
        1 => Ok(matches.pop().unwrap_or_default()),
        0 => Err(
            StoreError::FileNotFound(detected_candidate.unwrap_or_else(|| input.to_owned())).into(),
        ),
        _ => Err(RiptaskError::General(format!(
            "ambiguous numeric ID {number}: matches {}",
            matches.join(", ")
        ))),
    }
}

pub fn require_id(
    paths: &AppPaths,
    config: &Config,
    cwd: &Utf8Path,
    id: Option<String>,
    scope_args: &ScopeArgs,
) -> Result<Option<String>, RiptaskError> {
    if let Some(input) = id {
        return resolve_id(paths, config, cwd, &input).map(Some);
    }

    let scope =
        crate::scope::resolve_scope(&scope_args.projects, scope_args.all_projects, cwd, config)?;
    let issues = IssueService::new(paths, config)
        .list_matching(&LsArgs::default(), &scope)?
        .documents;
    let mode = if scope_args.all_projects {
        IssueDisplayMode::AllProjects
    } else {
        IssueDisplayMode::PerProject
    };
    let issues_dir = paths.issues_dir();
    let picker = FzfPicker {
        fzf_opts: config.ui.fzf_opts.clone(),
    };
    match picker.pick_issue(&issues, "issue> ", Some(issues_dir.as_str()), &mode) {
        Ok(Some(id)) => Ok(Some(id)),
        Ok(None) => Ok(None),
        Err(e) => {
            // fzf not installed — print available issues to stderr as fallback
            if !issues.is_empty() {
                eprintln!("Available issues:");
                for issue in &issues {
                    eprintln!("  {}", format_issue_plain(issue));
                }
            }
            Err(e)
        }
    }
}

pub fn resolve_or_pick_id(
    paths: &AppPaths,
    config: &Config,
    cwd: &Utf8Path,
    id: Option<String>,
    pick: bool,
    scope_args: &ScopeArgs,
) -> Result<Option<String>, RiptaskError> {
    if let Some(input) = id {
        return resolve_id(paths, config, cwd, &input).map(Some);
    }

    if pick {
        return require_id(paths, config, cwd, None, scope_args);
    }

    match id_for_current_branch(paths) {
        Ok(id) => Ok(Some(id)),
        Err(_) => require_id(paths, config, cwd, None, scope_args),
    }
}

pub(crate) fn current_repo() -> Result<std::path::PathBuf, RiptaskError> {
    std::env::current_dir().map_err(RiptaskError::from)
}

pub(crate) fn cwd_utf8() -> camino::Utf8PathBuf {
    camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    )
}

pub(crate) fn find_issue_for_branch(
    paths: &AppPaths,
    branch: &str,
) -> Result<camino::Utf8PathBuf, RiptaskError> {
    for path in issue_store::list_issues(paths)? {
        match frontmatter::try_load_issue(path.as_std_path()) {
            frontmatter::IssueLoadResult::Ok(issue) => {
                if issue.frontmatter.branch.as_deref() == Some(branch)
                    || issue.frontmatter.id_slug.as_deref() == Some(branch)
                {
                    return Ok(path);
                }
            }
            frontmatter::IssueLoadResult::Conflict { id, .. } => {
                for backup in [
                    issue_store::local_backup_path(paths, &id),
                    issue_store::remote_backup_path(paths, &id),
                ] {
                    if !backup.exists() {
                        continue;
                    }
                    if let frontmatter::IssueLoadResult::Ok(issue) =
                        frontmatter::try_load_issue(backup.as_std_path())
                        && (issue.frontmatter.branch.as_deref() == Some(branch)
                            || issue.frontmatter.id_slug.as_deref() == Some(branch))
                    {
                        return Ok(path.clone());
                    }
                }
            }
            frontmatter::IssueLoadResult::Err(error) => return Err(RiptaskError::Other(error)),
        }
    }
    Err(RiptaskError::NotFound(format!(
        "no issue found for branch {branch}"
    )))
}

/// Resolve the issue id associated with a specific git branch, if any.
///
/// Matches `frontmatter.branch` or `frontmatter.id_slug` via
/// `find_issue_for_branch`. Returns `RiptaskError::NotFound` when no issue is
/// associated with `branch`.
pub(crate) fn id_for_branch(paths: &AppPaths, branch: &str) -> Result<String, RiptaskError> {
    let path = find_issue_for_branch(paths, branch)?;
    Ok(path.file_stem().unwrap_or_default().to_string())
}

/// Resolve the issue id associated with the current git branch in the process
/// cwd.
pub fn id_for_current_branch(paths: &AppPaths) -> Result<String, RiptaskError> {
    let repo = current_repo()?;
    let branch = CliGit::new().current_branch(repo.as_path())?;
    id_for_branch(paths, &branch)
}

#[cfg(test)]
mod tests {
    use super::{id_for_branch, resolve_id};
    use crate::config::{Config, default_config};
    use crate::error::RiptaskError;
    use crate::models::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};
    use crate::paths::AppPaths;
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    #[test]
    fn resolve_id_full_id_passes_through() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).expect("utf8 cwd");
        let paths = app_paths(temp.path());
        let config = default_config();

        let resolved =
            resolve_id(&paths, &config, &cwd, "GH-GUB-DEV--61").expect("resolve full id");

        assert_eq!(resolved, "GH-GUB-DEV--61");
    }

    #[test]
    fn resolve_id_numeric_uses_detected_project() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("worktree")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("worktree dir");
        let paths = app_paths(temp.path());
        touch_issue(&paths, "GH-GUB-DEV--61");
        touch_issue(&paths, "GL-FOO-BAR--61");
        let config = config_with_projects(vec![RepoProject {
            name: "dev-tools".into(),
            vc_backend: VCBackendSpec {
                kind: BackendKind::Github,
                host: None,
                repo: Some("GubCorp/dev-tools".into()),
                path: Some(cwd.to_string()),
            },
            tasks_backend: TasksBackendSpec {
                kind: BackendKind::Github,
                host: None,
                repo: Some("GubCorp/dev-tools".into()),
                jira_project: None,
                default_issue_type: None,
                path: None,
            },
            default_board: Some("personal".into()),
            default_org: None,
            key: Some("GH-GUB-DEV".into()),
            repo_project_label: None,
        }]);

        let resolved = resolve_id(&paths, &config, &cwd, "61").expect("resolve numeric id");

        assert_eq!(resolved, "GH-GUB-DEV--61");
    }

    #[test]
    fn resolve_id_numeric_without_detected_project_uses_unique_suffix_match() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("outside")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("outside dir");
        let paths = app_paths(temp.path());
        touch_issue(&paths, "GH-GUB-DEV--61");
        touch_issue(&paths, "GL-FOO-BAR--7");
        let config = default_config();

        let resolved = resolve_id(&paths, &config, &cwd, "61").expect("resolve numeric id");

        assert_eq!(resolved, "GH-GUB-DEV--61");
    }

    #[test]
    fn resolve_id_numeric_ambiguous_match_errors() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("outside")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("outside dir");
        let paths = app_paths(temp.path());
        touch_issue(&paths, "GH-GUB-DEV--61");
        touch_issue(&paths, "GL-FOO-BAR--61");
        let config = default_config();

        let error = resolve_id(&paths, &config, &cwd, "61").expect_err("ambiguous id");

        assert_eq!(
            error.to_string(),
            "ambiguous numeric ID 61: matches GH-GUB-DEV--61, GL-FOO-BAR--61"
        );
    }

    #[test]
    fn resolve_id_numeric_without_matches_errors() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("outside")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("outside dir");
        let paths = app_paths(temp.path());
        let config = default_config();

        let error = resolve_id(&paths, &config, &cwd, "61").expect_err("missing id");

        assert_eq!(error.to_string(), "issue file not found: 61");
    }

    #[test]
    fn id_for_branch_returns_id_when_frontmatter_branch_matches() {
        let temp = tempdir().expect("temp dir");
        let paths = app_paths(temp.path());
        write_issue_with_branch(&paths, "GH-GUB-DEV--61", Some("feature/hello"), None);

        let id = id_for_branch(&paths, "feature/hello").expect("branch match");

        assert_eq!(id, "GH-GUB-DEV--61");
    }

    #[test]
    fn id_for_branch_returns_id_when_id_slug_matches() {
        let temp = tempdir().expect("temp dir");
        let paths = app_paths(temp.path());
        write_issue_with_branch(&paths, "GH-GUB-DEV--61", None, Some("feature/hello"));

        let id = id_for_branch(&paths, "feature/hello").expect("id_slug match");

        assert_eq!(id, "GH-GUB-DEV--61");
    }

    #[test]
    fn id_for_branch_returns_not_found_for_unmapped_branch() {
        let temp = tempdir().expect("temp dir");
        let paths = app_paths(temp.path());
        write_issue_with_branch(&paths, "GH-GUB-DEV--61", Some("feature/hello"), None);

        let error = id_for_branch(&paths, "feature/missing").expect_err("missing branch");

        assert!(matches!(error, RiptaskError::NotFound(_)));
    }

    fn app_paths(root: &std::path::Path) -> AppPaths {
        let repo = root.join("repo");
        let cache = root.join("cache");
        std::fs::create_dir_all(repo.join("issues")).expect("issues dir");
        std::fs::create_dir_all(repo.join("templates")).expect("templates dir");
        AppPaths {
            riptask_repo: repo.to_string_lossy().as_ref().into(),
            cache_root: cache.to_string_lossy().as_ref().into(),
            state_root: root.join("state").to_string_lossy().as_ref().into(),
        }
    }

    fn touch_issue(paths: &AppPaths, id: &str) {
        std::fs::write(paths.issues_dir().join(format!("{id}.md")), "").expect("write issue");
    }

    fn write_issue_with_branch(
        paths: &AppPaths,
        id: &str,
        branch: Option<&str>,
        id_slug: Option<&str>,
    ) {
        let branch = branch
            .map(|value| format!("branch: {value}\n"))
            .unwrap_or_default();
        let id_slug = id_slug
            .map(|value| format!("id-slug: {value}\n"))
            .unwrap_or_default();
        let contents = format!(
            "---\nid: {id}\ntitle: Test issue\nstatus: todo\nboard: personal\nproject: dev-tools\norg: ~\npriority: medium\nlabels: []\nassignees: []\nmilestone: ~\ncycle: ~\norder: 1\n{branch}{id_slug}github: ~\ngitlab: ~\njira: ~\nlocal_updated_at: 2026-03-13T11:05:00Z\ndue: ~\nrecurring: ~\nremote_deleted: false\n---\n\nTest body.\n"
        );
        std::fs::write(paths.issues_dir().join(format!("{id}.md")), contents).expect("write issue");
    }

    fn config_with_projects(projects: Vec<RepoProject>) -> Config {
        let mut config = default_config();
        config.projects = projects;
        config
    }
}
