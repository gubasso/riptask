use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::BranchArgs;
use crate::config::{Config, load_config};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{
    build_version_control, resolve_git_auth, resolve_vc_for_backend,
};
use crate::services::issue_service::generate_branch_slug;
use crate::services::{id_resolution, project_detection};
use crate::storage::{frontmatter, issue_store};

pub(crate) use crate::services::id_resolution::{current_repo, cwd_utf8, find_issue_for_branch};

pub async fn branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptskError> {
    if args.delete || args.force_delete {
        return delete_branch(paths, args).await;
    }
    create_branch(paths, args).await
}

async fn create_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let BranchArgs {
        scope, id, pick, ..
    } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = cwd_utf8();
    let Some(id) = id_resolution::resolve_or_pick_id(paths, &config, &cwd, id, pick, &scope)?
    else {
        return Ok(());
    };
    let slug = create_branch_for_issue(paths, &id).await?;
    println!("{slug}");
    Ok(())
}

pub(crate) async fn create_branch_for_issue(
    paths: &AppPaths,
    id: &str,
) -> Result<String, RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let path = issue_store::find_issue(paths, id)?;
    let mut issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), id)?;

    let backend = config
        .backends
        .iter()
        .find(|b| b.name == issue.frontmatter.project)
        .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?;
    if backend.backend == Backend::Local {
        return Err(RiptskError::Config(
            "cannot create remote branch for local-only project".into(),
        ));
    }

    let issue_number = backend_issue_number(backend, &issue)?;

    let slug = generate_branch_slug(issue_number, &issue.frontmatter.title);

    let repo = current_repo()?;

    // For Jira backends, resolve the VC backend for branch/PR operations
    let (vc_provider, vc_backend, vc_issue_id) = if backend.backend == Backend::Jira {
        let (provider, vc_backend) =
            resolve_vc_for_backend(backend, &config)?.ok_or_else(|| {
                RiptskError::Config(format!(
                    "No version control backend configured for project '{}'. \
                     Add 'vc: <github-or-gitlab-backend>' to the backend config.",
                    backend.name
                ))
            })?;
        // Don't link to a GitHub/GitLab issue — the issue lives in Jira
        (provider, vc_backend.clone(), None)
    } else {
        let provider = build_version_control(backend)?;
        (provider, backend.clone(), Some(issue_number))
    };

    let auth = resolve_git_auth(&vc_backend);
    let git = CliGit::with_auth(auth);
    let repo_name = vc_backend.repo.as_deref().unwrap_or_default();
    let base = vc_provider.default_branch(repo_name).await?;

    match vc_provider
        .create_branch(repo_name, &slug, &base, vc_issue_id)
        .await
    {
        Ok(()) => {}
        Err(ref e) if e.to_string().to_lowercase().contains("already exists") => {
            crate::ui::info("remote branch already exists, continuing with local checkout");
        }
        Err(e) => return Err(e),
    }

    if !git.branch_exists(repo.as_path(), &slug)? {
        git.fetch_and_checkout_tracking(repo.as_path(), &slug)?;
    } else {
        git.checkout(repo.as_path(), &slug)?;
    }

    issue.frontmatter.id_slug = Some(slug.clone());
    issue.frontmatter.branch = Some(slug.clone());
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
    maybe_auto_commit(
        &config,
        &git,
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: branch {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )?;
    Ok(slug)
}

async fn delete_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = cwd_utf8();
    let repo = current_repo()?;
    let current = CliGit::new().current_branch(repo.as_path())?;
    let force = args.force_delete;

    let target_branch = if let Some(ref input) = args.id {
        resolve_delete_target(paths, &config, &cwd, input)?
    } else {
        if is_protected_branch(&current) {
            crate::ui::warn(&format!("refusing to delete protected branch: {current}"));
            return Ok(());
        }
        current.clone()
    };

    if args.id.is_some() && is_protected_branch(&target_branch) {
        return Err(RiptskError::General(format!(
            "cannot delete protected branch: {target_branch}"
        )));
    }

    let deleting_current = target_branch == current;

    let issue_path = match find_issue_for_branch(paths, &target_branch) {
        Ok(path) => Some(path),
        Err(RiptskError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let (vc_backend, default_branch) =
        resolve_backend_and_default(&config, &cwd, issue_path.as_ref()).await?;
    let auth = resolve_git_auth(&vc_backend);
    let git = CliGit::with_auth(auth);
    let provider = build_version_control(&vc_backend)?;
    let repo_name = vc_backend.repo.as_deref().unwrap_or_default();

    let _ = git.fetch(repo.as_path());

    if !force {
        let local_exists = git.branch_exists(repo.as_path(), &target_branch)?;
        if !local_exists {
            return Err(RiptskError::General(format!(
                "branch '{}' not found locally; cannot verify merge status for safe delete. Use -D to force",
                target_branch
            )));
        }
        let base_ref = format!("origin/{default_branch}");
        if !git.is_branch_merged(repo.as_path(), &target_branch, &base_ref)? {
            return Err(RiptskError::General(format!(
                "branch '{}' is not fully merged into '{}'; use -D to force",
                target_branch, default_branch
            )));
        }
    }

    if deleting_current && !args.yes {
        let prompt =
            format!("Delete current branch '{target_branch}' and switch to '{default_branch}'?");
        if !DialoguerPrompts.confirm(&prompt, false)? {
            crate::ui::warn("aborted");
            return Ok(());
        }
    }

    match provider.delete_branch(repo_name, &target_branch).await {
        Ok(()) => crate::ui::success(&format!("deleted remote branch: {target_branch}")),
        Err(ref e)
            if e.to_string().contains("404")
                || e.to_string().to_lowercase().contains("not found") =>
        {
            crate::ui::info("remote branch not found, continuing with local cleanup");
        }
        Err(e) => return Err(e),
    }

    if deleting_current && let Err(e) = git.checkout(repo.as_path(), &default_branch) {
        return Err(RiptskError::General(format!(
            "remote branch deleted but failed to switch to '{default_branch}': {e}. \
             Recover with: git checkout -b {default_branch} origin/{default_branch}"
        )));
    }

    if git.branch_exists(repo.as_path(), &target_branch)? {
        git.delete_local_branch(repo.as_path(), &target_branch, force)?;
        crate::ui::success(&format!("deleted local branch: {target_branch}"));
    }

    if let Some(path) = issue_path {
        let id = path.file_stem().unwrap_or_default().to_string();
        let mut issue =
            crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        issue.frontmatter.branch = None;
        issue.frontmatter.id_slug = None;
        frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
        maybe_auto_commit(
            &config,
            &git,
            paths.riptsk_repo.as_std_path(),
            &format!(
                "riptsk: delete branch {} - {}",
                issue.frontmatter.id, issue.frontmatter.title
            ),
            &[path.as_std_path()],
        )?;
    }

    Ok(())
}

fn resolve_delete_target(
    paths: &AppPaths,
    config: &Config,
    cwd: &camino::Utf8Path,
    input: &str,
) -> Result<String, RiptskError> {
    if input.chars().all(|c| c.is_ascii_digit()) || input.contains("--") {
        let id = id_resolution::resolve_id(paths, config, cwd, input)?;
        let path = issue_store::find_issue(paths, &id)?;
        let issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        if let Some(branch) = issue.frontmatter.branch {
            return Ok(branch);
        }
        return Err(RiptskError::General(format!(
            "issue {id} has no associated branch"
        )));
    }
    Ok(input.to_owned())
}

async fn resolve_backend_and_default(
    config: &Config,
    cwd: &camino::Utf8Path,
    issue_path: Option<&camino::Utf8PathBuf>,
) -> Result<(BackendConfig, String), RiptskError> {
    let backend = if let Some(path) = issue_path {
        let id = path.file_stem().unwrap_or_default().to_string();
        let issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        config
            .backends
            .iter()
            .find(|b| b.name == issue.frontmatter.project)
            .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?
            .clone()
    } else {
        project_detection::detect_from_cwd(cwd, config)?.ok_or_else(|| {
            RiptskError::Config("cannot determine project for branch deletion".into())
        })?
    };

    // For Jira backends, resolve the VC backend
    let vc_backend = if backend.backend == Backend::Jira {
        resolve_vc_for_backend(&backend, config)?
            .map(|(_, vc_b)| vc_b.clone())
            .ok_or_else(|| {
                RiptskError::Config(format!(
                    "No version control backend for project '{}'. Add 'vc' to the backend config.",
                    backend.name
                ))
            })?
    } else {
        backend
    };

    let provider = build_version_control(&vc_backend)?;
    let repo_name = vc_backend.repo.as_deref().unwrap_or_default();
    let default = provider.default_branch(repo_name).await?;
    Ok((vc_backend, default))
}

pub(crate) fn backend_issue_number(
    backend: &BackendConfig,
    issue: &crate::domain::issue::IssueDocument,
) -> Result<u64, RiptskError> {
    match backend.backend {
        Backend::Github => issue
            .frontmatter
            .github
            .as_ref()
            .and_then(|meta| meta.issue_id),
        Backend::Gitlab => issue
            .frontmatter
            .gitlab
            .as_ref()
            .and_then(|meta| meta.issue_id),
        Backend::Jira => issue
            .frontmatter
            .jira
            .as_ref()
            .and_then(|meta| meta.issue_id),
        Backend::Local => None,
    }
    .ok_or_else(|| {
        RiptskError::Config(format!(
            "issue {} is missing backend metadata",
            issue.frontmatter.id
        ))
    })
}

pub(crate) fn is_protected_branch(branch: &str) -> bool {
    matches!(
        branch,
        "main" | "master" | "develop" | "devel" | "dev" | "trunk"
    )
}
