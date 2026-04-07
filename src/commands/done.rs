use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::DialoguerPrompts;
use crate::cli::{DoneArgs, SyncArgs};
use crate::commands::branch::{backend_issue_number, current_repo, cwd_utf8};
use crate::commands::{pr, sync_cmd};
use crate::config::load_config;
use crate::domain::issue::IssueState;
use crate::error::RiptskError;
use crate::models::Backend;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{
    build_provider_for_backend, resolve_git_auth, update_issue_from_backend,
};
use crate::services::id_resolution;
use crate::services::issue_service::now_utc;
use crate::services::view_builder::ViewBuilder;
use crate::storage::{cache, frontmatter, issue_store};

pub async fn run(paths: &AppPaths, args: DoneArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = cwd_utf8();
    let id = match id_resolution::resolve_or_pick_id(
        paths,
        &config,
        &cwd,
        args.id.clone(),
        args.pick,
        &args.scope,
    )? {
        Some(id) => id,
        None => return Ok(()),
    };
    let issue_path = issue_store::find_issue(paths, &id)?;
    let issue =
        crate::commands::issues::load_issue_or_conflict_error(issue_path.as_std_path(), &id)?;

    // Check if this is a Jira-only (no VC) project
    let issue_backend = config
        .backends
        .iter()
        .find(|b| b.name == issue.frontmatter.project)
        .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?;

    let has_vc = match issue_backend.backend {
        Backend::Github | Backend::Gitlab => true,
        Backend::Jira => issue_backend.vc.is_some(),
        Backend::Local => false,
    };

    if !has_vc || issue.frontmatter.branch.is_none() {
        // Issue-only workflow: just mark done locally and sync
        crate::ui::info("No version control backend configured — skipping PR merge.");
        let mut issue =
            frontmatter::load_issue(issue_path.as_std_path()).map_err(RiptskError::Other)?;
        issue.frontmatter.status = IssueState::Done;
        issue.frontmatter.local_updated_at = now_utc();
        issue.frontmatter.branch = None;
        issue.frontmatter.id_slug = None;
        frontmatter::save_issue(issue_path.as_std_path(), &issue).map_err(RiptskError::Other)?;
        maybe_auto_commit(
            &config,
            &CliGit::new(),
            paths.riptsk_repo.as_std_path(),
            &format!(
                "riptsk: done {} - {}",
                issue.frontmatter.id, issue.frontmatter.title
            ),
            &[issue_path.as_std_path()],
        )?;
        sync_cmd::run(
            paths,
            SyncArgs {
                force_pull_ids: vec![id.clone()],
                ..SyncArgs::default()
            },
        )
        .await?;
        ViewBuilder::new(paths, &config)
            .regenerate_all(None)
            .map_err(RiptskError::Other)?;
        return Ok(());
    }

    // Full VC workflow: merge PR, delete branch, close issue
    let branch_name = issue.frontmatter.branch.clone().unwrap();
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
        &DialoguerPrompts,
        &git,
        backend.backend.clone(),
        repo_dir.as_path(),
        repo_name,
        pr_number,
        &issue,
        &merge_opts,
    )
    .await?;

    let closed_record = match backend_issue_number(backend, &issue)
        .ok()
        .zip(Some(repo_name))
    {
        Some((backend_issue_id, repo)) => match provider.get_issue(repo, backend_issue_id).await {
            Ok(record) if record.state.eq_ignore_ascii_case("closed") => Some(record),
            Ok(_) | Err(_) => None,
        },
        None => None,
    };

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
        match git.delete_local_branch(repo_dir.as_path(), &branch_name, true) {
            Ok(()) => crate::ui::success(&format!("deleted local branch: {branch_name}")),
            Err(error) => crate::ui::warn(&format!(
                "failed to delete local branch {branch_name}: {error}"
            )),
        }
    }

    let mut issue =
        crate::commands::issues::load_issue_or_conflict_error(issue_path.as_std_path(), &id)?;
    issue.frontmatter.branch = None;
    issue.frontmatter.id_slug = None;
    frontmatter::save_issue(issue_path.as_std_path(), &issue).map_err(RiptskError::Other)?;
    if let Some(record) = &closed_record {
        let mut issue =
            crate::commands::issues::load_issue_or_conflict_error(issue_path.as_std_path(), &id)?;
        update_issue_from_backend(&mut issue, record, backend);
        issue.frontmatter.branch = None;
        issue.frontmatter.id_slug = None;
        frontmatter::save_issue(issue_path.as_std_path(), &issue).map_err(RiptskError::Other)?;
        cache::seed_backend_state_entry(paths, backend.backend.as_str(), repo_name, record)
            .map_err(RiptskError::Other)?;
    } else {
        let mut issue =
            frontmatter::load_issue(issue_path.as_std_path()).map_err(RiptskError::Other)?;
        issue.frontmatter.status = IssueState::Done;
        issue.frontmatter.local_updated_at = now_utc();
        issue.frontmatter.branch = None;
        issue.frontmatter.id_slug = None;
        frontmatter::save_issue(issue_path.as_std_path(), &issue).map_err(RiptskError::Other)?;
    }
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
    sync_cmd::run(
        paths,
        SyncArgs {
            force_pull_ids: vec![id.clone()],
            ..SyncArgs::default()
        },
    )
    .await?;
    ViewBuilder::new(paths, &config)
        .regenerate_all(None)
        .map_err(RiptskError::Other)?;
    Ok(())
}

fn is_branch_not_found_error(error: &RiptskError) -> bool {
    let message = error.to_string().to_lowercase();
    message.contains("404") || message.contains("not found")
}
