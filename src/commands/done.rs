use crate::adapters::backend::{BackendPrRecord, BackendProvider, MergeMethod, PrChecksStatus};
use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::DoneArgs;
use crate::commands::branch::{current_repo, cwd_utf8, find_issue_for_branch};
use crate::commands::pr;
use crate::config::load_config;
use crate::domain::issue::IssueDocument;
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{build_provider_for_backend, resolve_git_auth};
use crate::services::id_resolution;
use crate::services::issue_service::IssueService;
use crate::storage::{frontmatter, issue_store};
use std::time::{Duration, Instant};

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

    let initial_pr = provider.get_pr(repo_name, pr_number).await?;
    let method = args.merge_method.unwrap_or_default();

    if initial_pr.merged || initial_pr.state == "merged" {
        crate::ui::info("PR already merged, continuing with cleanup");
    } else {
        if !args.yes
            && !confirm_done(
                &DialoguerPrompts,
                &issue,
                &initial_pr,
                method,
                args.auto_merge,
            )?
        {
            crate::ui::warn("aborted");
            return Ok(());
        }

        if args.auto_merge {
            provider
                .enable_auto_merge(repo_name, pr_number, method)
                .await?;
            crate::ui::info("Auto-merge enabled, waiting for checks...");
            let _ = wait_for_merge(provider.as_ref(), repo_name, pr_number, args.timeout).await?;
        } else if let Err(error) = provider
            .merge_pr(repo_name, pr_number, method, None, None)
            .await
        {
            if looks_like_checks_blocker(&error.to_string()) {
                return Err(RiptskError::General(format!(
                    "{error}. Try `tsk done {} --auto-merge`.",
                    issue.frontmatter.id
                )));
            }
            return Err(error);
        }
    }

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

fn confirm_done(
    prompts: &dyn PromptBackend,
    issue: &IssueDocument,
    pr: &BackendPrRecord,
    method: MergeMethod,
    auto_merge: bool,
) -> Result<bool, RiptskError> {
    let mode = if auto_merge {
        "enable auto-merge for"
    } else {
        "merge"
    };
    let prompt = format!(
        "{} '{}' via {}?\n{}",
        mode,
        issue.frontmatter.title,
        merge_method_label(method),
        pr.url
    );
    prompts.confirm(&prompt, false)
}

async fn wait_for_merge(
    provider: &dyn BackendProvider,
    repo: &str,
    pr_number: u64,
    timeout_secs: u64,
) -> Result<BackendPrRecord, RiptskError> {
    let started = Instant::now();
    loop {
        let pr = provider.get_pr(repo, pr_number).await?;
        if pr.merged || pr.state == "merged" {
            return Ok(pr);
        }

        let checks = provider.get_pr_checks_status(repo, pr_number).await?;
        crate::ui::info(&format!(
            "checks status: {}",
            pr_checks_status_label(checks)
        ));
        if checks == PrChecksStatus::Failed {
            return Err(RiptskError::General(format!(
                "checks failed for PR #{pr_number}"
            )));
        }
        if started.elapsed() >= Duration::from_secs(timeout_secs) {
            return Err(RiptskError::General(format!(
                "timed out after {timeout_secs} seconds waiting for PR #{pr_number} to merge"
            )));
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

fn merge_method_label(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

fn pr_checks_status_label(status: PrChecksStatus) -> &'static str {
    match status {
        PrChecksStatus::None => "none",
        PrChecksStatus::Pending => "pending",
        PrChecksStatus::Passed => "passed",
        PrChecksStatus::Failed => "failed",
    }
}

fn looks_like_checks_blocker(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("status check")
        || lower.contains("required check")
        || lower.contains("check pending")
        || lower.contains("pipeline")
}

fn is_branch_not_found_error(error: &RiptskError) -> bool {
    let message = error.to_string().to_lowercase();
    message.contains("404") || message.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::{looks_like_checks_blocker, merge_method_label, pr_checks_status_label};
    use crate::adapters::backend::{MergeMethod, PrChecksStatus};

    #[test]
    fn labels_merge_method_values() {
        assert_eq!(merge_method_label(MergeMethod::Merge), "merge");
        assert_eq!(merge_method_label(MergeMethod::Squash), "squash");
        assert_eq!(merge_method_label(MergeMethod::Rebase), "rebase");
    }

    #[test]
    fn labels_check_status_values() {
        assert_eq!(pr_checks_status_label(PrChecksStatus::None), "none");
        assert_eq!(pr_checks_status_label(PrChecksStatus::Pending), "pending");
        assert_eq!(pr_checks_status_label(PrChecksStatus::Passed), "passed");
        assert_eq!(pr_checks_status_label(PrChecksStatus::Failed), "failed");
    }

    #[test]
    fn detects_check_related_merge_errors() {
        assert!(looks_like_checks_blocker(
            "Required status check \"ci\" is expected."
        ));
        assert!(looks_like_checks_blocker(
            "pipeline must succeed before merge"
        ));
        assert!(!looks_like_checks_blocker("merge conflict"));
    }
}
