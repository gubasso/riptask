use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::BranchArgs;
use crate::config::{Config, load_config};
use crate::error::RiptaskError;
use crate::models::{BackendKind, RepoProject};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{build_version_control, resolve_git_auth};
use crate::services::issue_service::generate_branch_slug;
use crate::services::{id_resolution, project_detection};
use crate::storage::{frontmatter, issue_store};

pub(crate) use crate::services::id_resolution::{current_repo, cwd_utf8, find_issue_for_branch};

pub async fn branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptaskError> {
    if args.delete || args.force_delete {
        return delete_branch(paths, args).await;
    }
    create_branch(paths, args).await
}

async fn create_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptaskError> {
    paths.require_initialized()?;
    let BranchArgs {
        scope, id, pick, ..
    } = args;
    let config = load_config(paths.config_path().as_std_path())?;
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
) -> Result<String, RiptaskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
    let path = issue_store::find_issue(paths, id)?;
    let mut issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), id)?;

    let repo_project = config
        .projects
        .iter()
        .find(|rp| rp.name == issue.frontmatter.project)
        .ok_or_else(|| RiptaskError::Unregistered(issue.frontmatter.project.clone()))?;
    if repo_project.vc_backend.kind == BackendKind::Local {
        return Err(RiptaskError::Config(
            "cannot create remote branch for local-only project".into(),
        ));
    }

    let issue_number = backend_issue_number(repo_project, &issue)?;

    let slug = generate_branch_slug(issue_number, &issue.frontmatter.title);

    let repo = current_repo()?;

    // For Jira backends, resolve the VC backend for branch/PR operations
    let vc_provider = build_version_control(&repo_project.vc_backend, &repo_project.name)?;
    let vc_issue_id = if repo_project.tasks_backend.kind == BackendKind::Jira {
        None
    } else {
        Some(issue_number)
    };

    let auth = resolve_git_auth(&repo_project.vc_backend, &repo_project.name);
    let git = CliGit::with_auth(auth);
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let base = crate::ui::spin_on_async("Fetching default branch", async {
        vc_provider.default_branch(repo_name).await
    })
    .await?;

    let branch_spinner = crate::ui::spinner("Creating remote branch".to_owned());
    let create_branch_result = vc_provider
        .create_branch(repo_name, &slug, &base, vc_issue_id)
        .await;
    if let Some(ref pb) = branch_spinner {
        pb.finish_and_clear();
    }
    match create_branch_result {
        Ok(()) => {}
        Err(ref e) if e.to_string().to_lowercase().contains("already exists") => {
            crate::ui::info("remote branch already exists, continuing with local checkout");
        }
        Err(e) => return Err(e),
    }
    tracing::info!(branch = %slug, backend = %repo_project.name, "ensured remote branch exists");

    if !git.branch_exists(repo.as_path(), &slug)? {
        crate::ui::spin_on("Checking out branch", || {
            git.fetch_and_checkout_tracking(repo.as_path(), &slug)
        })?;
    } else {
        git.checkout(repo.as_path(), &slug)?;
    }

    issue.frontmatter.id_slug = Some(slug.clone());
    issue.frontmatter.branch = Some(slug.clone());
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
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

async fn delete_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptaskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
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
        return Err(RiptaskError::General(format!(
            "cannot delete protected branch: {target_branch}"
        )));
    }

    let deleting_current = target_branch == current;

    let issue_path = match find_issue_for_branch(paths, &target_branch) {
        Ok(path) => Some(path),
        Err(RiptaskError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let (repo_project, default_branch) =
        resolve_backend_and_default(&config, &cwd, issue_path.as_ref()).await?;
    let auth = resolve_git_auth(&repo_project.vc_backend, &repo_project.name);
    let git = CliGit::with_auth(auth);
    let provider = build_version_control(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();

    let fetch_spinner = crate::ui::spinner("Fetching remote branches".to_owned());
    let fetch_result = git.fetch(repo.as_path());
    if let Some(ref pb) = fetch_spinner {
        pb.finish_and_clear();
    }
    let _ = fetch_result;

    if !force {
        let local_exists = git.branch_exists(repo.as_path(), &target_branch)?;
        if !local_exists {
            return Err(RiptaskError::General(format!(
                "branch '{}' not found locally; cannot verify merge status for safe delete. Use -D to force",
                target_branch
            )));
        }
        let base_ref = format!("origin/{default_branch}");
        if !git.is_branch_merged(repo.as_path(), &target_branch, &base_ref)? {
            return Err(RiptaskError::General(format!(
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

    let delete_spinner = crate::ui::spinner("Deleting remote branch".to_owned());
    let delete_result = provider.delete_branch(repo_name, &target_branch).await;
    if let Some(ref pb) = delete_spinner {
        pb.finish_and_clear();
    }
    match delete_result {
        Ok(()) => crate::ui::success(&format!("deleted remote branch: {target_branch}")),
        Err(ref e)
            if e.to_string().contains("404")
                || e.to_string().to_lowercase().contains("not found") =>
        {
            crate::ui::info("remote branch not found, continuing with local cleanup");
        }
        Err(e) => return Err(e),
    }
    tracing::info!(branch = %target_branch, backend = %repo_project.name, "remote branch deletion finished");

    if deleting_current && let Err(e) = git.checkout(repo.as_path(), &default_branch) {
        return Err(RiptaskError::General(format!(
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
        frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
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
) -> Result<String, RiptaskError> {
    if input.chars().all(|c| c.is_ascii_digit()) || input.contains("--") {
        let id = id_resolution::resolve_id(paths, config, cwd, input)?;
        let path = issue_store::find_issue(paths, &id)?;
        let issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        if let Some(branch) = issue.frontmatter.branch {
            return Ok(branch);
        }
        return Err(RiptaskError::General(format!(
            "issue {id} has no associated branch"
        )));
    }
    Ok(input.to_owned())
}

async fn resolve_backend_and_default(
    config: &Config,
    cwd: &camino::Utf8Path,
    issue_path: Option<&camino::Utf8PathBuf>,
) -> Result<(RepoProject, String), RiptaskError> {
    let repo_project = if let Some(path) = issue_path {
        let id = path.file_stem().unwrap_or_default().to_string();
        let issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        config
            .projects
            .iter()
            .find(|rp| rp.name == issue.frontmatter.project)
            .ok_or_else(|| RiptaskError::Unregistered(issue.frontmatter.project.clone()))?
            .clone()
    } else {
        project_detection::detect_from_cwd(cwd, config)?
            .cloned()
            .ok_or_else(|| {
                RiptaskError::Config("cannot determine project for branch deletion".into())
            })?
    };

    let provider = build_version_control(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let default = provider.default_branch(repo_name).await?;
    Ok((repo_project, default))
}

pub(crate) fn backend_issue_number(
    repo_project: &RepoProject,
    issue: &crate::domain::issue::IssueDocument,
) -> Result<u64, RiptaskError> {
    match repo_project.tasks_backend.kind {
        BackendKind::Github => issue
            .frontmatter
            .github
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Gitlab => issue
            .frontmatter
            .gitlab
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Jira => issue
            .frontmatter
            .jira
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Local => None,
    }
    .ok_or_else(|| {
        RiptaskError::Config(format!(
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
