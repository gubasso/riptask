use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::{BranchArgs, IdArgs};
use crate::config::{Config, load_config};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::build_provider_for_backend;
use crate::services::issue_service::generate_branch_slug;
use crate::services::{id_resolution, project_detection};
use crate::storage::{frontmatter, issue_store};

pub async fn branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptskError> {
    if args.delete || args.force_delete {
        return delete_branch(paths, args).await;
    }
    create_branch(paths, args).await
}

async fn create_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let BranchArgs { scope, id, .. } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = cwd_utf8();
    let Some(id) = id_resolution::require_id(paths, &config, &cwd, id, &scope)? else {
        return Ok(());
    };
    let path = issue_store::find_issue(paths, &id)?;
    let mut issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;

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

    let issue_number = match backend.backend {
        Backend::Github => issue.frontmatter.github.as_ref().and_then(|m| m.issue_id),
        Backend::Gitlab => issue.frontmatter.gitlab.as_ref().and_then(|m| m.issue_id),
        Backend::Local => None,
    }
    .ok_or_else(|| {
        RiptskError::Config(format!(
            "issue {} is missing backend metadata (remote issue ID)",
            issue.frontmatter.id
        ))
    })?;

    let slug = generate_branch_slug(issue_number, &issue.frontmatter.title);

    let repo = current_repo()?;
    let git = CliGit;

    let provider = build_provider_for_backend(backend)?;
    let repo_name = backend.repo.as_deref().unwrap_or_default();
    let base = provider.default_branch(repo_name).await?;

    // Always attempt remote branch creation to ensure the remote state is correct,
    // even if a local branch already exists from a prior partial run.
    // Tolerate "already exists" so reruns are idempotent.
    match provider
        .create_branch(repo_name, &slug, &base, issue_number)
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
    println!("{slug}");
    Ok(())
}

async fn delete_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = cwd_utf8();
    let repo = current_repo()?;
    let git = CliGit;
    let current = git.current_branch(repo.as_path())?;
    let force = args.force_delete;

    // 1. Resolve target branch
    let target_branch = if let Some(ref input) = args.id {
        resolve_delete_target(paths, &config, &cwd, input)?
    } else {
        if is_protected_branch(&current) {
            crate::ui::warn(&format!("refusing to delete protected branch: {current}"));
            return Ok(());
        }
        current.clone()
    };

    // 2. Validate target is not protected (explicit target case)
    if args.id.is_some() && is_protected_branch(&target_branch) {
        return Err(RiptskError::General(format!(
            "cannot delete protected branch: {target_branch}"
        )));
    }

    let deleting_current = target_branch == current;

    // 3. Resolve backend & default branch
    let issue_path = match find_issue_for_branch(paths, &target_branch) {
        Ok(path) => Some(path),
        Err(RiptskError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let (backend, default_branch) =
        resolve_backend_and_default(&config, &cwd, issue_path.as_ref()).await?;
    let provider = build_provider_for_backend(&backend)?;
    let repo_name = backend.repo.as_deref().unwrap_or_default();

    // 4. Fetch to ensure local refs are current (best-effort)
    let _ = git.fetch(repo.as_path());

    // 5. Safe-delete merge check (-d)
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

    // 6. Confirmation for current-branch deletion
    if deleting_current && !args.yes {
        let prompt =
            format!("Delete current branch '{target_branch}' and switch to '{default_branch}'?");
        if !DialoguerPrompts.confirm(&prompt, false)? {
            crate::ui::warn("aborted");
            return Ok(());
        }
    }

    // 7. Delete remote first (SoT)
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

    // 8. If deleting current branch, switch to default first
    if deleting_current && let Err(e) = git.checkout(repo.as_path(), &default_branch) {
        return Err(RiptskError::General(format!(
            "remote branch deleted but failed to switch to '{default_branch}': {e}. \
             Recover with: git checkout -b {default_branch} origin/{default_branch}"
        )));
    }

    // 9. Delete local branch
    if git.branch_exists(repo.as_path(), &target_branch)? {
        git.delete_local_branch(repo.as_path(), &target_branch, force)?;
        crate::ui::success(&format!("deleted local branch: {target_branch}"));
    }

    // 10. Clear issue metadata if linked
    if let Some(path) = issue_path {
        let mut issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
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
    // If input looks like an issue ID (numeric or contains "--"), resolve via issue
    if input.chars().all(|c| c.is_ascii_digit()) || input.contains("--") {
        let id = id_resolution::resolve_id(paths, config, cwd, input)?;
        let path = issue_store::find_issue(paths, &id)?;
        let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
        if let Some(branch) = issue.frontmatter.branch {
            return Ok(branch);
        }
        return Err(RiptskError::General(format!(
            "issue {id} has no associated branch"
        )));
    }
    // Otherwise treat as literal branch name
    Ok(input.to_owned())
}

async fn resolve_backend_and_default(
    config: &Config,
    cwd: &camino::Utf8Path,
    issue_path: Option<&camino::Utf8PathBuf>,
) -> Result<(BackendConfig, String), RiptskError> {
    if let Some(path) = issue_path {
        let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
        let backend = config
            .backends
            .iter()
            .find(|b| b.name == issue.frontmatter.project)
            .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?
            .clone();
        let provider = build_provider_for_backend(&backend)?;
        let repo_name = backend.repo.as_deref().unwrap_or_default();
        let default = provider.default_branch(repo_name).await?;
        return Ok((backend, default));
    }
    // Fall back to project detection from cwd
    let backend = project_detection::detect_from_cwd(cwd, config)?.ok_or_else(|| {
        RiptskError::Config("cannot determine project for branch deletion".into())
    })?;
    let provider = build_provider_for_backend(&backend)?;
    let repo_name = backend.repo.as_deref().unwrap_or_default();
    let default = provider.default_branch(repo_name).await?;
    Ok((backend, default))
}

fn is_protected_branch(branch: &str) -> bool {
    matches!(
        branch,
        "main" | "master" | "develop" | "devel" | "dev" | "trunk"
    )
}

pub async fn pr(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let IdArgs { id, .. } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = cwd_utf8();
    let repo = current_repo()?;
    let git = CliGit;
    let current_branch = git.current_branch(repo.as_path())?;
    if is_protected_branch(&current_branch) {
        return Err(RiptskError::General(format!(
            "protected branch: {current_branch}"
        )));
    }

    let path = if let Some(id) = id {
        let id = id_resolution::resolve_id(paths, &config, &cwd, &id)?;
        issue_store::find_issue(paths, &id)?
    } else {
        find_issue_for_branch(paths, &current_branch)?
    };
    let mut issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    if issue.frontmatter.pr_url.is_some() {
        return Err(RiptskError::General(format!(
            "PR already exists for {}",
            issue.frontmatter.id
        )));
    }
    if let Some(branch) = issue.frontmatter.branch.as_ref() {
        if branch != &current_branch {
            return Err(RiptskError::General(
                "current branch does not match issue branch".into(),
            ));
        }
    } else {
        issue.frontmatter.branch = Some(current_branch.clone());
    }

    let backend = config
        .backends
        .iter()
        .find(|backend| backend.name == issue.frontmatter.project)
        .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?;
    if !matches!(backend.backend, Backend::Github | Backend::Gitlab) {
        return Err(RiptskError::Config(format!(
            "project {} does not have a hosted backend",
            backend.name
        )));
    }
    let provider = build_provider_for_backend(backend)?;
    let repo_name = backend.repo.as_deref().unwrap_or_default();
    let base = provider.default_branch(repo_name).await?;
    let issue_number = match backend.backend {
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
        Backend::Local => None,
    }
    .ok_or_else(|| {
        RiptskError::Config(format!(
            "issue {} is missing backend metadata",
            issue.frontmatter.id
        ))
    })?;
    let title = format!("Resolve \"{}\"", issue.frontmatter.title);
    let body = format!("Closes #{issue_number}");
    let pr_url = provider
        .create_pr(repo_name, &current_branch, &base, &title, &body)
        .await?;
    issue.frontmatter.pr_url = Some(pr_url.clone());
    if issue.frontmatter.state == crate::domain::issue::IssueState::Todo {
        issue.frontmatter.state = crate::domain::issue::IssueState::InProgress;
        issue.frontmatter.local_updated_at = crate::services::issue_service::now_utc();
    }
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
    maybe_auto_commit(
        &config,
        &git,
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: pr {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )?;
    println!("{pr_url}");
    Ok(())
}

fn current_repo() -> Result<std::path::PathBuf, RiptskError> {
    std::env::current_dir().map_err(RiptskError::from)
}

fn cwd_utf8() -> camino::Utf8PathBuf {
    camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    )
}

fn find_issue_for_branch(
    paths: &AppPaths,
    branch: &str,
) -> Result<camino::Utf8PathBuf, RiptskError> {
    for path in issue_store::list_issues(paths)? {
        let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
        if issue.frontmatter.branch.as_deref() == Some(branch)
            || issue.frontmatter.id_slug.as_deref() == Some(branch)
        {
            return Ok(path);
        }
    }
    Err(RiptskError::NotFound(format!(
        "no issue found for branch {branch}"
    )))
}
