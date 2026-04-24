use crate::adapters::backend::VersionControl;
use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::DialoguerPrompts;
use crate::cli::{DoneArgs, SyncArgs};
use crate::commands::branch::{backend_issue_number, current_repo, cwd_utf8};
use crate::commands::{pr, sync_cmd};
use crate::config::load_config;
use crate::domain::issue::{IssueDocument, IssueState};
use crate::error::RiptaskError;
use crate::models::BackendKind;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{
    build_hosted_provider, resolve_git_auth, update_issue_from_backend,
};
use crate::services::id_resolution;
use crate::services::issue_service::now_utc;
use crate::services::view_builder::ViewBuilder;
use crate::storage::{cache, frontmatter, issue_store};

async fn ensure_pr_number<F, Fut>(
    provider: &dyn VersionControl,
    repo_name: &str,
    issue_path: &camino::Utf8Path,
    id: &str,
    issue: &IssueDocument,
    create_pr: F,
) -> Result<(IssueDocument, u64), RiptaskError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), RiptaskError>>,
{
    if let Some(number) = remote_pr_number_for_issue(provider, repo_name, issue).await? {
        return Ok((issue.clone(), number));
    }
    crate::ui::info("no PR found, creating one...");
    create_pr().await?;
    let reloaded =
        crate::commands::issues::load_issue_or_conflict_error(issue_path.as_std_path(), id)?;
    let number = pr::resolve_pr_number(provider, repo_name, &reloaded).await?;
    Ok((reloaded, number))
}

/// Returns the PR number for `issue` only if validated against the remote.
/// Local frontmatter metadata is treated as a hint and re-checked via `get_pr`;
/// if validation fails, falls back to `find_pr_by_branch`. Returns `Ok(None)`
/// if no PR currently exists for the issue's branch on the remote.
async fn remote_pr_number_for_issue(
    provider: &dyn VersionControl,
    repo_name: &str,
    issue: &IssueDocument,
) -> Result<Option<u64>, RiptaskError> {
    let Some(branch) = issue.frontmatter.branch.as_deref() else {
        return Ok(None);
    };
    let base = provider.default_branch(repo_name).await?;
    let local_hint = issue.frontmatter.pr_number.or_else(|| {
        issue
            .frontmatter
            .pr_url
            .as_deref()
            .and_then(pr::parse_pr_number_from_url)
    });
    if let Some(number) = local_hint
        && let Ok(record) = provider.get_pr(repo_name, number).await
        && pr::pr_head_matches(&record.head, branch)
        && record.base == base
    {
        return Ok(Some(record.number));
    }
    Ok(provider
        .find_pr_by_branch(repo_name, branch, &base)
        .await?
        .map(|record| record.number))
}

pub async fn run(paths: &AppPaths, args: DoneArgs) -> Result<(), RiptaskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
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
    let repo_project = config
        .projects
        .iter()
        .find(|rp| rp.name == issue.frontmatter.project)
        .ok_or_else(|| RiptaskError::Unregistered(issue.frontmatter.project.clone()))?;

    let has_vc = repo_project.vc_backend.kind != BackendKind::Local;

    if !has_vc || issue.frontmatter.branch.is_none() {
        // Issue-only workflow: just mark done locally and sync
        crate::ui::info("No version control backend configured — skipping PR merge.");
        let mut issue =
            frontmatter::load_issue(issue_path.as_std_path()).map_err(RiptaskError::Other)?;
        issue.frontmatter.status = IssueState::Done;
        issue.frontmatter.local_updated_at = now_utc();
        issue.frontmatter.branch = None;
        issue.frontmatter.id_slug = None;
        frontmatter::save_issue(issue_path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
        maybe_auto_commit(
            &config,
            &CliGit::new(),
            paths.riptask_repo.as_std_path(),
            &format!(
                "riptask: done {} - {}",
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
        crate::ui::spin_on("Regenerating views", || {
            ViewBuilder::new(paths, &config)
                .regenerate_all(None)
                .map_err(RiptaskError::Other)
        })?;
        return Ok(());
    }

    // Full VC workflow: merge PR, delete branch, close issue
    let branch_name = issue.frontmatter.branch.clone().unwrap();
    let repo_project = pr::resolve_hosted_repo_project(&config, &issue)?;
    let provider = build_hosted_provider(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let git = CliGit::with_auth(resolve_git_auth(
        &repo_project.vc_backend,
        &repo_project.name,
    ));
    let repo_dir = current_repo()?;
    let original_branch = git.current_branch(repo_dir.as_path()).ok();

    let stashed = if git.has_working_tree_changes(repo_dir.as_path())? {
        let current_branch = git.current_branch(repo_dir.as_path()).unwrap_or_default();
        let msg = format!(
            "tsk done: auto-stash ({} on {}) [{}]",
            id,
            current_branch,
            now_utc()
        );
        crate::ui::info(&format!("stashing uncommitted changes: {msg}"));
        git.stash_push(repo_dir.as_path(), &msg)?
    } else {
        false
    };

    let result = async {
        let (issue, pr_number) = ensure_pr_number(
            provider.as_ref(),
            repo_name,
            issue_path.as_path(),
            &id,
            &issue,
            || async {
                // pr::create requires the working tree to be on the issue branch.
                // Switch to it on demand so `tsk done` works from any branch.
                if git.current_branch(repo_dir.as_path())? != branch_name {
                    git.checkout(repo_dir.as_path(), &branch_name)?;
                }
                pr::create(
                    paths,
                    crate::cli::PrCreateArgs {
                        scope: args.scope.clone(),
                        id: Some(id.clone()),
                        no_ai: false,
                    },
                )
                .await
            },
        )
        .await?;
        let merge_opts = pr::MergeOptions {
            merge_method: args.merge_method.unwrap_or_default(),
            auto_merge: args.auto_merge,
            yes: args.yes,
            timeout: args.timeout,
            force_push: args.force_push,
        };
        let outcome = pr::merge_pr_workflow(
            provider.as_ref(),
            &DialoguerPrompts,
            &git,
            repo_project.vc_backend.kind.clone(),
            repo_dir.as_path(),
            repo_name,
            pr_number,
            &issue,
            &merge_opts,
        )
        .await?;
        if matches!(outcome, pr::MergeOutcome::Aborted) {
            // User declined the merge confirmation; skip all post-merge cleanup
            // (branch deletion, issue close, sync, view regen).
            return Ok(());
        }

        let closed_record = match backend_issue_number(repo_project, &issue)
            .ok()
            .zip(Some(repo_name))
        {
            Some((backend_issue_id, repo)) => {
                let issue_spinner = crate::ui::spinner("Fetching issue status".to_owned());
                let issue_result = provider.get_issue(repo, backend_issue_id).await;
                if let Some(ref pb) = issue_spinner {
                    pb.finish_and_clear();
                }
                match issue_result {
                    Ok(record) if record.state.eq_ignore_ascii_case("closed") => Some(record),
                    Ok(_) | Err(_) => None,
                }
            }
            None => None,
        };

        let default_branch = crate::ui::spin_on_async("Fetching default branch", async {
            provider.default_branch(repo_name).await
        })
        .await?;
        crate::ui::spin_on("Switching to default branch", || {
            git.checkout(repo_dir.as_path(), &default_branch)
        })?;
        crate::ui::spin_on("Pulling latest changes", || git.pull(repo_dir.as_path()))?;

        let delete_spinner = crate::ui::spinner("Deleting remote branch".to_owned());
        let delete_result = provider.delete_branch(repo_name, &branch_name).await;
        if let Some(ref pb) = delete_spinner {
            pb.finish_and_clear();
        }
        match delete_result {
            Ok(()) => crate::ui::success(&format!("deleted remote branch: {branch_name}")),
            Err(error) if is_branch_not_found_error(&error) => {
                crate::ui::info("remote branch not found, continuing with local cleanup");
            }
            Err(error) => return Err(error),
        }
        tracing::info!(branch = %branch_name, "finished remote branch cleanup");

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
        frontmatter::save_issue(issue_path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
        if let Some(record) = &closed_record {
            let mut issue = crate::commands::issues::load_issue_or_conflict_error(
                issue_path.as_std_path(),
                &id,
            )?;
            update_issue_from_backend(&mut issue, record, repo_project);
            issue.frontmatter.branch = None;
            issue.frontmatter.id_slug = None;
            frontmatter::save_issue(issue_path.as_std_path(), &issue)
                .map_err(RiptaskError::Other)?;
            cache::seed_backend_state_entry(
                paths,
                repo_project.tasks_backend.kind.as_str(),
                repo_name,
                record,
            )
            .map_err(RiptaskError::Other)?;
        } else {
            let mut issue =
                frontmatter::load_issue(issue_path.as_std_path()).map_err(RiptaskError::Other)?;
            issue.frontmatter.status = IssueState::Done;
            issue.frontmatter.local_updated_at = now_utc();
            issue.frontmatter.branch = None;
            issue.frontmatter.id_slug = None;
            frontmatter::save_issue(issue_path.as_std_path(), &issue)
                .map_err(RiptaskError::Other)?;
        }
        maybe_auto_commit(
            &config,
            &git,
            paths.riptask_repo.as_std_path(),
            &format!(
                "riptask: done {} - {}",
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
        crate::ui::spin_on("Regenerating views", || {
            ViewBuilder::new(paths, &config)
                .regenerate_all(None)
                .map_err(RiptaskError::Other)
        })?;
        tracing::info!(issue_id = %id, "completed done workflow");
        Ok(())
    }
    .await;
    // On failure, restore the caller's original branch before popping the stash so that
    // any stashed changes land back on the branch the user invoked `tsk done` from.
    if result.is_err()
        && let Some(orig) = original_branch.as_deref()
        && git.current_branch(repo_dir.as_path()).ok().as_deref() != Some(orig)
        && let Err(error) = git.checkout(repo_dir.as_path(), orig)
    {
        crate::ui::warn(&format!(
            "failed to restore original branch {orig}: {error}"
        ));
    }
    if stashed {
        crate::adapters::git::try_stash_pop(&git, repo_dir.as_path());
    }
    result
}

fn is_branch_not_found_error(error: &RiptaskError) -> bool {
    let message = error.to_string().to_lowercase();
    message.contains("404") || message.contains("not found")
}

#[cfg(test)]
mod tests {
    use super::ensure_pr_number;
    use crate::adapters::backend::{
        BackendPrRecord, CiPresence, MergeMethod, PrChecksStatus, VersionControl,
    };
    use crate::domain::issue::IssueDocument;
    use crate::error::RiptaskError;
    use crate::storage::frontmatter;
    use async_trait::async_trait;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct FakeProvider {
        default_branch: String,
        pr_number: Arc<Mutex<Option<u64>>>,
        branch_error: Arc<Mutex<Option<String>>>,
    }

    impl FakeProvider {
        fn with_pr_number(pr_number: Option<u64>) -> Self {
            Self {
                default_branch: "main".into(),
                pr_number: Arc::new(Mutex::new(pr_number)),
                branch_error: Arc::new(Mutex::new(None)),
            }
        }

        fn with_branch_error(error: &str) -> Self {
            Self {
                default_branch: "main".into(),
                pr_number: Arc::new(Mutex::new(None)),
                branch_error: Arc::new(Mutex::new(Some(error.into()))),
            }
        }

        fn set_pr_number(&self, pr_number: Option<u64>) {
            *self.pr_number.lock().expect("lock") = pr_number;
        }
    }

    #[async_trait]
    impl VersionControl for FakeProvider {
        async fn create_pr(
            &self,
            _repo: &str,
            _head: &str,
            _base: &str,
            _title: &str,
            _body: &str,
        ) -> Result<BackendPrRecord, RiptaskError> {
            unimplemented!()
        }

        async fn get_pr(&self, _repo: &str, _number: u64) -> Result<BackendPrRecord, RiptaskError> {
            unimplemented!()
        }

        async fn update_pr(
            &self,
            _repo: &str,
            _number: u64,
            _title: &str,
            _body: &str,
        ) -> Result<BackendPrRecord, RiptaskError> {
            unimplemented!()
        }

        async fn find_pr_by_branch(
            &self,
            _repo: &str,
            _head: &str,
            _base: &str,
        ) -> Result<Option<BackendPrRecord>, RiptaskError> {
            if let Some(error) = self.branch_error.lock().expect("lock").clone() {
                return Err(RiptaskError::General(error));
            }
            Ok(self
                .pr_number
                .lock()
                .expect("lock")
                .map(|number| BackendPrRecord {
                    number,
                    title: "Test PR".into(),
                    body: String::new(),
                    url: format!("https://example.com/pr/{number}"),
                    state: "open".into(),
                    head: "feature/test".into(),
                    head_sha: None,
                    base: self.default_branch.clone(),
                    node_id: None,
                    merged: false,
                    updated_at: String::new(),
                }))
        }

        async fn merge_pr(
            &self,
            _repo: &str,
            _number: u64,
            _method: MergeMethod,
            _commit_title: Option<&str>,
            _commit_message: Option<&str>,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }

        async fn get_pr_checks_status(
            &self,
            _repo: &str,
            _number: u64,
        ) -> Result<PrChecksStatus, RiptaskError> {
            unimplemented!()
        }

        async fn get_ci_presence(&self, _repo: &str) -> Result<CiPresence, RiptaskError> {
            unimplemented!()
        }

        async fn create_branch(
            &self,
            _repo: &str,
            _branch_name: &str,
            _base_ref: &str,
            _issue_id: Option<u64>,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }

        async fn default_branch(&self, _repo: &str) -> Result<String, RiptaskError> {
            Ok(self.default_branch.clone())
        }

        async fn delete_branch(&self, _repo: &str, _branch_name: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
    }

    fn test_issue() -> IssueDocument {
        frontmatter::parse_issue_str(
            "test",
            "---\n\
id: TEST--1\n\
title: Test issue\n\
status: todo\n\
board: personal\n\
project: demo\n\
local_updated_at: \"2026-04-07T00:00:00Z\"\n\
labels: []\n\
remote_deleted: false\n\
branch: feature/test\n\
---\n\
body\n",
        )
        .expect("parse issue")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_pr_number_returns_existing_pr_without_creating() {
        let provider = FakeProvider::with_pr_number(Some(17));
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let issue_path = camino::Utf8PathBuf::from_path_buf(temp_dir.path().join("TEST--1.md"))
            .expect("utf8 path");
        let issue = test_issue();
        frontmatter::save_issue(issue_path.as_std_path(), &issue).expect("save issue");
        let create_calls = Arc::new(Mutex::new(0usize));
        let create_calls_for_closure = Arc::clone(&create_calls);

        let (reloaded, pr_number) = ensure_pr_number(
            &provider,
            "owner/repo",
            issue_path.as_path(),
            "TEST--1",
            &issue,
            move || {
                let create_calls = Arc::clone(&create_calls_for_closure);
                async move {
                    *create_calls.lock().expect("lock") += 1;
                    Ok(())
                }
            },
        )
        .await
        .expect("existing pr");

        assert_eq!(pr_number, 17);
        assert_eq!(reloaded.frontmatter.id, issue.frontmatter.id);
        assert_eq!(*create_calls.lock().expect("lock"), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_pr_number_creates_and_reloads_issue_when_missing() {
        let provider = FakeProvider::with_pr_number(None);
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let issue_path = camino::Utf8PathBuf::from_path_buf(temp_dir.path().join("TEST--1.md"))
            .expect("utf8 path");
        let issue = test_issue();
        frontmatter::save_issue(issue_path.as_std_path(), &issue).expect("save issue");
        let create_calls = Arc::new(Mutex::new(0usize));
        let create_calls_for_closure = Arc::clone(&create_calls);
        let provider_for_closure = provider.clone();
        let issue_path_for_closure = issue_path.clone();

        let (reloaded, pr_number) = ensure_pr_number(
            &provider,
            "owner/repo",
            issue_path.as_path(),
            "TEST--1",
            &issue,
            move || {
                let create_calls = Arc::clone(&create_calls_for_closure);
                let provider = provider_for_closure.clone();
                let issue_path = issue_path_for_closure.clone();
                async move {
                    *create_calls.lock().expect("lock") += 1;
                    let mut updated =
                        frontmatter::load_issue(issue_path.as_std_path()).expect("load issue");
                    updated.frontmatter.pr_number = Some(42);
                    updated.frontmatter.pr_url = Some("https://example.com/pr/42".into());
                    frontmatter::save_issue(issue_path.as_std_path(), &updated)
                        .expect("save issue");
                    provider.set_pr_number(Some(42));
                    Ok(())
                }
            },
        )
        .await
        .expect("created pr");

        assert_eq!(pr_number, 42);
        assert_eq!(reloaded.frontmatter.pr_number, Some(42));
        assert_eq!(
            reloaded.frontmatter.pr_url.as_deref(),
            Some("https://example.com/pr/42")
        );
        assert_eq!(*create_calls.lock().expect("lock"), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ensure_pr_number_propagates_non_not_found_errors() {
        let provider = FakeProvider::with_branch_error("backend exploded");
        let temp_dir = tempfile::TempDir::new().expect("temp dir");
        let issue_path = camino::Utf8PathBuf::from_path_buf(temp_dir.path().join("TEST--1.md"))
            .expect("utf8 path");
        let issue = test_issue();
        frontmatter::save_issue(issue_path.as_std_path(), &issue).expect("save issue");
        let create_calls = Arc::new(Mutex::new(0usize));
        let create_calls_for_closure = Arc::clone(&create_calls);

        let error = ensure_pr_number(
            &provider,
            "owner/repo",
            issue_path.as_path(),
            "TEST--1",
            &issue,
            move || {
                let create_calls = Arc::clone(&create_calls_for_closure);
                async move {
                    *create_calls.lock().expect("lock") += 1;
                    Ok(())
                }
            },
        )
        .await
        .expect_err("expected provider error");

        match error {
            RiptaskError::General(message) => assert_eq!(message, "backend exploded"),
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(*create_calls.lock().expect("lock"), 0);
    }
}
