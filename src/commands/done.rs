use crate::adapters::git::{CliGit, GitBackend};
use crate::cli::DoneArgs;
use crate::commands::branch::{current_repo, cwd_utf8, find_issue_for_branch};
use crate::commands::pr;
use crate::config::load_config;
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{build_provider_for_backend, resolve_git_auth};
use crate::services::id_resolution;
use crate::services::issue_service::IssueService;
use crate::storage::{frontmatter, issue_store};

pub async fn run(paths: &AppPaths, args: DoneArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = cwd_utf8();
    let id = if let Some(input) = args.id.clone() {
        id_resolution::resolve_id(paths, &config, &cwd, &input)?
    } else {
        match current_repo()
            .and_then(|repo| CliGit::new().current_branch(repo.as_path()))
            .and_then(|branch| find_issue_for_branch(paths, &branch))
            .and_then(|path| {
                frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)
            }) {
            Ok(issue) => issue.frontmatter.id,
            Err(_) => match id_resolution::require_id(paths, &config, &cwd, None, &args.scope)? {
                Some(id) => id,
                None => return Ok(()),
            },
        }
    };
    let issue_path = issue_store::find_issue(paths, &id)?;
    let mut issue =
        frontmatter::load_issue(issue_path.as_std_path()).map_err(RiptskError::Other)?;
    let branch_name = issue
        .frontmatter
        .branch
        .clone()
        .ok_or_else(|| RiptskError::General("issue has no associated branch".into()))?;
    let backend = pr::resolve_hosted_backend(&config, &issue)?;
    let provider = build_provider_for_backend(backend)?;
    let repo_name = backend.repo.as_deref().unwrap_or_default();
    let pr_number = pr::resolve_pr_number(provider.as_ref(), repo_name, &issue).await?;
    let git = CliGit::with_auth(resolve_git_auth(backend));
    let repo_dir = current_repo()?;

    if git.has_working_tree_changes(repo_dir.as_path())? {
        return Err(RiptskError::General(
            "working tree is dirty; commit or stash changes before running `tsk done`".into(),
        ));
    }

    let merge_opts = pr::MergeOptions {
        merge_method: args.merge_method.unwrap_or_default(),
        auto_merge: args.auto_merge,
        yes: args.yes,
        timeout: args.timeout,
        force_push: args.force_push,
    };
    pr::merge_pr_workflow(
        provider.as_ref(),
        &git,
        backend.backend.clone(),
        repo_dir.as_path(),
        repo_name,
        pr_number,
        &issue,
        &merge_opts,
    )
    .await?;

    let default_branch = provider.default_branch(repo_name).await?;
    git.checkout(repo_dir.as_path(), &default_branch)?;
    git.pull(repo_dir.as_path())?;

    match provider.delete_branch(repo_name, &branch_name).await {
        Ok(()) => crate::ui::success(&format!("deleted remote branch: {branch_name}")),
        Err(error) if is_branch_not_found_error(&error) => {
            crate::ui::info("remote branch not found, continuing with local cleanup");
        }
        Err(error) => return Err(error),
    }

    if git.branch_exists(repo_dir.as_path(), &branch_name)? {
        // Force-delete because squash/rebase merges leave the branch tip
        // unreachable from HEAD, which causes `git branch -d` to fail.
        match git.delete_local_branch(repo_dir.as_path(), &branch_name, true) {
            Ok(()) => crate::ui::success(&format!("deleted local branch: {branch_name}")),
            Err(error) => crate::ui::warn(&format!(
                "failed to delete local branch {branch_name}: {error}"
            )),
        }
    }

    issue.frontmatter.branch = None;
    issue.frontmatter.id_slug = None;
    frontmatter::save_issue(issue_path.as_std_path(), &issue).map_err(RiptskError::Other)?;
    IssueService::new(paths, &config).move_issue(&id, "done")?;
    maybe_auto_commit(
        &config,
        &git,
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: done {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[issue_path.as_std_path()],
    )?;
    Ok(())
}

fn is_branch_not_found_error(error: &RiptskError) -> bool {
    let message = error.to_string().to_lowercase();
    message.contains("404") || message.contains("not found")
}
