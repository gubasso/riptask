use crate::adapters::ai::AiBackend;
#[cfg(test)]
use crate::adapters::backend::PrChecksReport;
use crate::adapters::backend::{BackendPrRecord, MergeMethod, PrChecksStatus, VersionControl};
use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::{
    PrArgs, PrCreateArgs, PrEditArgs, PrMergeArgs, PrShowArgs, PrSubcommand, ScopeArgs,
};
use crate::commands::ai::{AI_BACKEND_MISSING, optional_backend};
use crate::commands::branch::{backend_issue_number, current_repo, cwd_utf8};
use crate::config::{Config, load_effective_config};
use crate::domain::issue::{IssueDocument, IssueState};
use crate::error::RiptaskError;
use crate::models::{BackendKind, RepoProject};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{build_hosted_provider, resolve_git_auth};
use crate::services::id_resolution;
use crate::services::issue_service::now_utc;
use crate::storage::{frontmatter, issue_store};
use crate::ui;
use indicatif::ProgressBar;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};
const EMPTY_BRANCH_COMMIT_MESSAGE: &str = "chore: initialize branch for PR\n\n\
                                          Empty commit to allow PR creation on a branch with no changes yet.";
const REPO_INIT_COMMIT_MESSAGE: &str = "chore: initialize repository\n\n\
                                        Empty commit to bootstrap an empty repository so a PR can be created.";
const CHECKS_POLL_INTERVAL: Duration = Duration::from_secs(10);
const REGISTRATION_GRACE: Duration = Duration::from_secs(90);

struct InitialChecksState {
    expected: Vec<String>,
    head_sha: String,
}

pub async fn run(paths: &AppPaths, args: PrArgs) -> Result<(), RiptaskError> {
    // The parent `tsk pr` carries `--ai-prompt` so the no-subcommand shorthand
    // (`tsk pr <ID> --ai-prompt ...`) works. When an explicit subcommand is
    // present, forward the parent-level flag into Create/Edit so flag order
    // (`tsk pr --ai-prompt ... create ...`) does not silently drop the hint.
    // The subcommand-level value, if set, wins.
    let parent_ai_prompt = args.ai_prompt;
    match args.subcommand {
        Some(PrSubcommand::Create(mut sub)) => {
            if sub.ai_prompt.is_none() {
                sub.ai_prompt = parent_ai_prompt;
            }
            create(paths, sub).await
        }
        Some(PrSubcommand::Edit(mut sub)) => {
            if sub.ai_prompt.is_none() {
                sub.ai_prompt = parent_ai_prompt;
            }
            edit(paths, sub).await
        }
        Some(PrSubcommand::Show(args)) => show(paths, args).await,
        Some(PrSubcommand::Merge(args)) => merge(paths, args).await,
        None => {
            create(
                paths,
                PrCreateArgs {
                    scope: args.scope,
                    id: args.id,
                    no_ai: args.no_ai,
                    ai_prompt: parent_ai_prompt,
                },
            )
            .await
        }
    }
}

pub(crate) async fn create(paths: &AppPaths, args: PrCreateArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let repo = current_repo()?;
    let current_branch = CliGit::new().current_branch(repo.as_path())?;
    let (path, mut issue) = resolve_issue(paths, &config, &args.scope, args.id.as_deref())?;
    let branch = issue
        .frontmatter
        .branch
        .clone()
        .ok_or_else(|| RiptaskError::General("run `tsk branch` first".into()))?;
    let repo_project = resolve_hosted_repo_project(&config, &issue)?;
    let git = CliGit::with_auth(resolve_git_auth(
        &repo_project.vc_backend,
        &repo_project.name,
    ));
    let has_head_commit = git.has_head_commit(repo.as_path())?;
    // Probe the remote up front: if it has no refs we must route through the
    // bootstrap path regardless of `has_head_commit`. This covers three states
    // that all reach `tsk pr` with an empty remote: (a) unborn HEAD (issue
    // #155's primary case), (b) the user committed locally before pushing,
    // and (c) recovery from a partial bootstrap (e.g. first push failed).
    let remote_empty = !git.remote_has_any_refs(repo.as_path())?;
    if has_head_commit && !remote_empty && current_branch != branch {
        return Err(RiptaskError::General(
            "current branch does not match issue branch".into(),
        ));
    }

    let provider = build_hosted_provider(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let default_branch = ui::spin_on_async("Fetching default branch", async {
        provider.default_branch(repo_name).await
    })
    .await?;

    // Refuse early when the issue branch is the remote's default branch: a PR
    // cannot have the same head and base. This happens on brand-new GitHub
    // repos when the very first ref pushed was the issue branch (GitHub then
    // promotes it to `default_branch`). The user must push a separate base
    // branch on the remote and set it as default before opening a PR.
    if !remote_empty && branch == default_branch {
        return Err(RiptaskError::General(format!(
            "cannot open a PR from branch `{branch}` because it is the remote's default branch \
             (head and base would be identical); push a separate base branch (e.g. `main`) \
             on the remote and set it as the default, then re-run `tsk pr`"
        )));
    }

    let mut skip_ai = false;
    if remote_empty {
        // Require real commits on the issue branch before opening a PR. The
        // user is expected to have committed their first changes between
        // `tsk start` and `tsk pr`; without commits there is nothing to
        // review.
        if !has_head_commit {
            return Err(RiptaskError::General(
                "no commits on the issue branch yet — commit your work first, then re-run `tsk pr`"
                    .into(),
            ));
        }
        if current_branch != branch {
            return Err(RiptaskError::General(format!(
                "current branch `{current_branch}` does not match issue branch `{branch}`; \
                 switch to it (`git switch {branch}`) and re-run"
            )));
        }
        ui::info("remote repository is empty; bootstrapping default branch...");
        bootstrap_empty_remote(&git, repo.as_path(), &default_branch, &branch)?;
        skip_ai = true;
    } else if has_head_commit {
        // Check local metadata first, then remote, for idempotent create
        let local_pr_number = issue.frontmatter.pr_number.or_else(|| {
            issue
                .frontmatter
                .pr_url
                .as_deref()
                .and_then(parse_pr_number_from_url)
        });
        let existing_pr = ui::spin_on_async("Checking for existing PR", async {
            let existing_pr = if let Some(pr_number) = local_pr_number
                && let Ok(existing) = provider.get_pr(repo_name, pr_number).await
                && pr_head_matches(&existing.head, &branch)
                && existing.base == default_branch
            {
                Some(existing)
            } else {
                provider
                    .find_pr_by_branch(repo_name, &branch, &default_branch)
                    .await?
            };
            Ok::<_, RiptaskError>(existing_pr)
        })
        .await?;
        if let Some(existing) = existing_pr {
            tracing::info!(pr_number = existing.number, branch = %branch, "reused existing PR");
            sync_and_commit_pr(&config, &git, paths, &path, &mut issue, &existing)?;
            println!("{}", existing.url);
            return Ok(());
        }

        ui::spin_on("Fetching from remote", || git.fetch(repo.as_path()))?;
        ui::spin_on("Pushing to remote", || {
            git.push_with_upstream(repo.as_path(), &branch)
        })?;

        if repo_project.vc_backend.kind == BackendKind::Github
            && git.commits_ahead_of_base(repo.as_path(), &branch, &default_branch)? == 0
        {
            // Previously we synthesized an empty "chore: initialize branch for PR"
            // commit here so `tsk pr` could open a PR on a freshly branched, not-yet-
            // committed branch. That trick was unsound: at merge time the placeholder
            // was rebase-dropped, and whenever the user's real work was already
            // reachable from `default_branch` (e.g. they created the base branch from
            // the issue branch's HEAD) the drop exposed a zero-diff branch and the
            // user was trapped with a half-rewritten local/remote pair. Refuse early
            // instead — the user must commit before opening a PR.
            return Err(RiptaskError::General(format!(
                "branch `{branch}` has no commits ahead of `{default_branch}`; \
                 commit your work first, then re-run `tsk pr`"
            )));
        }
    } else {
        // !remote_empty && !has_head_commit: remote has refs but local repo
        // has none. This means the user ran `tsk start` before fetching the
        // remote. Push is impossible from an unborn HEAD against a non-empty
        // remote; instruct the user to fetch first.
        return Err(RiptaskError::General(
            "local repository has no commits but the remote is non-empty; \
             run `git fetch` or `git pull` first"
                .into(),
        ));
    }

    // For Jira issues, use the Jira issue key for PR title/body (not a numeric GitHub issue number)
    let (title, mut body) = if let Some(ref jira_meta) = issue.frontmatter.jira {
        let key = jira_meta
            .issue_key
            .as_deref()
            .unwrap_or(&issue.frontmatter.id);
        (
            format!("{key}: {}", issue.frontmatter.title),
            format!("Jira: {key}"),
        )
    } else {
        let issue_number = backend_issue_number(repo_project, &issue)?;
        (
            default_title(issue_number, &issue.frontmatter.title),
            default_body(issue_number),
        )
    };
    let use_ai = config.ai.enabled && !args.no_ai;
    if use_ai && !skip_ai {
        if let Some(ai) = optional_backend(&config) {
            let context =
                build_create_ai_context(&issue, &git, repo.as_path(), &default_branch, &branch)?;
            let ai_input =
                crate::adapters::ai::compose_ai_user_input(&context, args.ai_prompt.as_deref());
            match ui::spin_on("Generating PR description", || {
                ai.generate_pr_description(&ai_input)
            }) {
                Ok(description) if !description.trim().is_empty() => {
                    body = format!("{body}\n\n{}", description.trim());
                }
                Ok(_) => {}
                Err(error) => ui::warn(&format!("AI PR description unavailable: {error}")),
            }
        } else {
            ui::warn(&format!("AI unavailable: {AI_BACKEND_MISSING}"));
        }
    }

    let record = ui::spin_on_async("Creating pull request", async {
        provider
            .create_pr(repo_name, &branch, &default_branch, &title, &body)
            .await
    })
    .await?;
    tracing::info!(pr_number = record.number, branch = %branch, "created pull request");

    issue.frontmatter.pr_url = Some(record.url.clone());
    issue.frontmatter.pr_number = Some(record.number);
    if matches!(
        issue.frontmatter.status,
        IssueState::Backlog | IssueState::Todo
    ) {
        issue.frontmatter.status = IssueState::InProgress;
        issue.frontmatter.local_updated_at = now_utc();
    }
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
    sync_and_commit_pr(&config, &git, paths, &path, &mut issue, &record)?;
    println!("{}", record.url);
    Ok(())
}

fn bootstrap_empty_remote(
    git: &dyn GitBackend,
    repo: &Path,
    default_branch: &str,
    branch: &str,
) -> Result<(), RiptaskError> {
    // Push a fresh orphan empty commit directly as the remote's default
    // branch. No local refs are created, the index is not touched, and the
    // user's working tree is untouched. The resulting remote `default_branch`
    // has unrelated history from the issue branch, which GitHub accepts as a
    // valid PR base; the PR diff is the entire content of the issue branch,
    // which is exactly what the user did.
    git.push_orphan_initial_branch(repo, default_branch, REPO_INIT_COMMIT_MESSAGE)?;
    // Then push the user's actual issue branch. Their real commits become
    // the PR head.
    git.push_with_upstream(repo, branch)?;
    Ok(())
}

async fn show(paths: &AppPaths, args: PrShowArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let (_path, issue) = resolve_issue(paths, &config, &args.scope, args.id.as_deref())?;
    let repo_project = resolve_hosted_repo_project(&config, &issue)?;
    let provider = build_hosted_provider(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let pr_number = resolve_pr_number(provider.as_ref(), repo_name, &issue).await?;
    let record = ui::spin_on_async("Fetching PR details", async {
        provider.get_pr(repo_name, pr_number).await
    })
    .await?;

    if args.json {
        let payload = serde_json::to_string_pretty(&record).map_err(|error| {
            RiptaskError::Other(anyhow::Error::from(error).context("failed to serialize PR JSON"))
        })?;
        println!("{payload}");
    } else {
        println!("#{} [{}] {}", record.number, record.state, record.title);
        println!("{}", record.url);
        println!("{} -> {}", record.head, record.base);
        if !record.body.trim().is_empty() {
            println!();
            println!("{}", record.body);
        }
    }
    Ok(())
}

async fn merge(paths: &AppPaths, args: PrMergeArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let (_path, issue) = resolve_issue(paths, &config, &args.scope, args.id.as_deref())?;
    let repo_project = resolve_hosted_repo_project(&config, &issue)?;
    let git = CliGit::with_auth(resolve_git_auth(
        &repo_project.vc_backend,
        &repo_project.name,
    ));
    let repo_path = current_repo()?;
    let stashed = if git.has_working_tree_changes(repo_path.as_path())? {
        let current_branch = git.current_branch(repo_path.as_path()).unwrap_or_default();
        let msg = format!(
            "tsk pr merge: auto-stash ({} on {}) [{}]",
            issue.frontmatter.id,
            current_branch,
            now_utc()
        );
        ui::info(&format!("stashing uncommitted changes: {msg}"));
        git.stash_push(repo_path.as_path(), &msg)?
    } else {
        false
    };
    let provider = build_hosted_provider(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let pr_number = resolve_pr_number(provider.as_ref(), repo_name, &issue).await?;
    let opts = MergeOptions {
        merge_method: args.merge_method.unwrap_or_default(),
        auto_merge: args.auto_merge,
        yes: args.yes,
        timeout: args.timeout,
        force_push: args.force_push,
    };

    let result = merge_pr_workflow(
        provider.as_ref(),
        &DialoguerPrompts,
        &git,
        repo_project.vc_backend.kind.clone(),
        repo_path.as_path(),
        repo_name,
        pr_number,
        &issue,
        &opts,
    )
    .await;
    if stashed {
        crate::adapters::git::try_stash_pop(&git, repo_path.as_path());
    }
    result.map(|_| ())
}

async fn edit(paths: &AppPaths, args: PrEditArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let (path, mut issue) = resolve_issue(paths, &config, &args.scope, args.id.as_deref())?;
    let repo_project = resolve_hosted_repo_project(&config, &issue)?;
    let provider = build_hosted_provider(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let pr_number = resolve_pr_number(provider.as_ref(), repo_name, &issue).await?;
    let current = ui::spin_on_async("Fetching PR details", async {
        provider.get_pr(repo_name, pr_number).await
    })
    .await?;

    let record = if args.title.is_some() || args.description.is_some() {
        let title = args.title.as_deref().unwrap_or(&current.title);
        let body = args.description.as_deref().unwrap_or(&current.body);
        ui::spin_on_async("Updating pull request", async {
            provider.update_pr(repo_name, pr_number, title, body).await
        })
        .await?
    } else if !config.ai.enabled || args.no_ai {
        let draft = edit_buffer(&current.title, &current.body)?;
        ui::spin_on_async("Updating pull request", async {
            provider
                .update_pr(repo_name, pr_number, &draft.0, &draft.1)
                .await
        })
        .await?
    } else {
        let ai_context = if let Some(branch) = issue.frontmatter.branch.as_deref() {
            match (
                ui::spin_on_async("Fetching default branch", async {
                    provider.default_branch(repo_name).await
                })
                .await,
                current_repo(),
            ) {
                (Ok(default_branch), Ok(repo)) => {
                    let git = CliGit::new();
                    build_update_ai_context(&current, &git, repo.as_path(), &default_branch, branch)
                        .ok()
                }
                _ => None,
            }
        } else {
            None
        };
        let draft_body = if let Some(context) = ai_context {
            match optional_backend(&config) {
                Some(ai) => {
                    let ai_input = crate::adapters::ai::compose_ai_user_input(
                        &context,
                        args.ai_prompt.as_deref(),
                    );
                    match ui::spin_on("Generating PR description update", || {
                        ai.update_pr_description(&ai_input)
                    }) {
                        Ok(description) => description,
                        Err(error) => {
                            ui::warn(&format!("AI PR update unavailable: {error}"));
                            String::new()
                        }
                    }
                }
                None => {
                    if config.ai.enabled {
                        ui::warn(&format!("AI unavailable: {AI_BACKEND_MISSING}"));
                    }
                    String::new()
                }
            }
        } else {
            String::new()
        };
        let draft = if draft_body.is_empty() {
            edit_buffer(&current.title, &current.body)?
        } else if args.yes {
            (current.title.clone(), draft_body.trim().to_owned())
        } else {
            edit_buffer(&current.title, draft_body.trim())?
        };
        ui::spin_on_async("Updating pull request", async {
            provider
                .update_pr(repo_name, pr_number, &draft.0, &draft.1)
                .await
        })
        .await?
    };

    sync_issue_pr_metadata(&path, &mut issue, &record)?;
    ui::success(&format!("updated PR #{}", record.number));
    tracing::info!(pr_number = record.number, "updated pull request");
    Ok(())
}

pub(crate) struct MergeOptions {
    pub merge_method: MergeMethod,
    pub auto_merge: bool,
    pub yes: bool,
    pub timeout: u64,
    pub force_push: bool,
}

/// Outcome of `merge_pr_workflow`, signaling whether callers should proceed
/// with post-merge cleanup (branch deletion, issue close, etc.) or stop.
pub(crate) enum MergeOutcome {
    /// PR was merged in this call or was already merged — cleanup should proceed.
    Completed,
    /// User declined the confirmation prompt — caller must abort all further work.
    Aborted,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn merge_pr_workflow(
    provider: &dyn VersionControl,
    prompts: &dyn PromptBackend,
    git: &dyn GitBackend,
    backend_kind: BackendKind,
    repo_path: &Path,
    repo_name: &str,
    pr_number: u64,
    issue: &IssueDocument,
    opts: &MergeOptions,
) -> Result<MergeOutcome, RiptaskError> {
    let initial_pr = ui::spin_on_async("Fetching PR details", async {
        provider.get_pr(repo_name, pr_number).await
    })
    .await?;
    let mut expected_head_sha: Option<String> = None;

    if initial_pr.merged || initial_pr.state == "merged" {
        ui::info("PR already merged, continuing with cleanup");
        return Ok(MergeOutcome::Completed);
    }

    if !opts.yes && !confirm_merge(prompts, issue, &initial_pr, opts.merge_method)? {
        ui::warn("aborted");
        return Ok(MergeOutcome::Aborted);
    }

    // Cleanup empty bootstrap commit for GitHub before merge
    if backend_kind == BackendKind::Github
        && let Some(branch) = issue.frontmatter.branch.as_deref()
    {
        let default_branch = ui::spin_on_async("Fetching default branch", async {
            provider.default_branch(repo_name).await
        })
        .await?;
        let subject = EMPTY_BRANCH_COMMIT_MESSAGE
            .lines()
            .next()
            .unwrap_or_default();
        if let Some(sha) =
            git.find_commit_by_subject(repo_path, branch, &default_branch, subject)?
        {
            // Pre-check: refuse to destructively rewrite the branch when the
            // placeholder is the ONLY commit ahead of base. Rebasing it away
            // would rewind the local branch to base while leaving the remote
            // ahead — a confusing local/remote drift. Fail fast so the user
            // can close the PR manually.
            let ahead = git.commits_ahead_of_base(repo_path, branch, &default_branch)?;
            if ahead <= 1 {
                return Err(RiptaskError::General(format!(
                    "PR #{pr_number} for branch `{branch}` only contains a bootstrap \
                     commit (no real work ahead of `{default_branch}`); close PR #{pr_number} \
                     on the remote, commit real work, then re-run"
                )));
            }
            // Ensure we are on the issue branch before rewriting history
            let current = git.current_branch(repo_path)?;
            if current != branch {
                git.checkout(repo_path, branch)?;
            }
            ui::info("dropping empty bootstrap commit...");
            git.rebase_drop_commit(repo_path, &sha, branch)?;

            ui::spin_on("Force-pushing rebased branch", || {
                if opts.force_push {
                    git.force_push(repo_path, branch)
                } else {
                    git.force_push_with_lease(repo_path, branch)
                }
            })?;
            expected_head_sha = Some(git.head_sha(repo_path)?);
        }
    }

    // Push to ensure remote has all local commits before checking CI status
    if issue.frontmatter.branch.is_some() {
        ui::spin_on("Pushing to remote", || git.push(repo_path))?;
    }

    if let Some(ref sha) = expected_head_sha {
        wait_for_pr_head_update(
            provider,
            repo_name,
            pr_number,
            sha,
            Duration::from_secs(60),
            CHECKS_POLL_INTERVAL,
        )
        .await?;
    }

    if !opts.auto_merge {
        handle_ci_checks(provider, repo_name, pr_number, opts).await?;
    }
    ui::spin_on_async("Merging pull request", async {
        provider
            .merge_pr(repo_name, pr_number, opts.merge_method, None, None)
            .await
    })
    .await?;
    tracing::info!(pr_number, "merged pull request");

    Ok(MergeOutcome::Completed)
}

fn resolve_issue(
    paths: &AppPaths,
    config: &Config,
    _scope: &ScopeArgs,
    id: Option<&str>,
) -> Result<(camino::Utf8PathBuf, IssueDocument), RiptaskError> {
    let cwd = cwd_utf8();
    // When no ID given, resolve by current branch (scope unused in branch-based resolution)
    let path = if let Some(id) = id {
        let resolved = id_resolution::resolve_id(paths, config, &cwd, id)?;
        issue_store::find_issue(paths, &resolved)?
    } else {
        let resolved = id_resolution::id_for_current_branch(paths)?;
        issue_store::find_issue(paths, &resolved)?
    };
    let id = path.file_stem().unwrap_or_default().to_string();
    let issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
    Ok((path, issue))
}

pub(crate) fn resolve_hosted_repo_project<'a>(
    config: &'a Config,
    issue: &IssueDocument,
) -> Result<&'a RepoProject, RiptaskError> {
    let repo_project = config
        .projects
        .iter()
        .find(|repo_project| repo_project.name == issue.frontmatter.project)
        .ok_or_else(|| RiptaskError::Unregistered(issue.frontmatter.project.clone()))?;
    if repo_project.vc_backend.kind == BackendKind::Local {
        Err(RiptaskError::Config(format!(
            "project {} does not have a hosted backend",
            repo_project.name
        )))
    } else {
        Ok(repo_project)
    }
}

pub(crate) async fn resolve_pr_number(
    provider: &dyn crate::adapters::backend::VersionControl,
    repo_name: &str,
    issue: &IssueDocument,
) -> Result<u64, RiptaskError> {
    if let Some(number) = issue.frontmatter.pr_number {
        return Ok(number);
    }
    if let Some(url) = issue.frontmatter.pr_url.as_deref()
        && let Some(number) = parse_pr_number_from_url(url)
    {
        return Ok(number);
    }
    let branch = issue
        .frontmatter
        .branch
        .as_deref()
        .ok_or_else(|| RiptaskError::General("issue has no associated branch".into()))?;
    let base = provider.default_branch(repo_name).await?;
    if let Some(record) = provider.find_pr_by_branch(repo_name, branch, &base).await? {
        return Ok(record.number);
    }
    Err(RiptaskError::NotFound(format!(
        "no PR found for issue {}",
        issue.frontmatter.id
    )))
}

fn sync_issue_pr_metadata(
    path: &camino::Utf8Path,
    issue: &mut IssueDocument,
    record: &crate::adapters::backend::BackendPrRecord,
) -> Result<(), RiptaskError> {
    let mut changed = false;
    if issue.frontmatter.pr_url.as_deref() != Some(record.url.as_str()) {
        issue.frontmatter.pr_url = Some(record.url.clone());
        changed = true;
    }
    if issue.frontmatter.pr_number != Some(record.number) {
        issue.frontmatter.pr_number = Some(record.number);
        changed = true;
    }
    if changed {
        frontmatter::save_issue(path.as_std_path(), issue).map_err(RiptaskError::Other)?;
    }
    Ok(())
}

fn sync_and_commit_pr(
    config: &Config,
    git: &dyn GitBackend,
    paths: &AppPaths,
    path: &camino::Utf8Path,
    issue: &mut IssueDocument,
    record: &crate::adapters::backend::BackendPrRecord,
) -> Result<(), RiptaskError> {
    sync_issue_pr_metadata(path, issue, record)?;
    maybe_auto_commit(
        config,
        git,
        paths.riptask_repo.as_std_path(),
        &format!(
            "riptask: pr {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )
}

fn confirm_merge(
    prompts: &dyn PromptBackend,
    _issue: &IssueDocument,
    pr: &BackendPrRecord,
    method: MergeMethod,
) -> Result<bool, RiptaskError> {
    ui::info(&pr.url);
    let prompt = format!("merge '{}' via {}?", pr.title, merge_method_label(method));
    prompts.confirm(&prompt, false)
}

async fn handle_ci_checks(
    provider: &dyn VersionControl,
    repo_name: &str,
    pr_number: u64,
    opts: &MergeOptions,
) -> Result<PrChecksStatus, RiptaskError> {
    let report = provider.get_pr_checks_report(repo_name, pr_number).await?;
    for warning in &report.warnings {
        ui::warn(warning);
    }
    if report.expected.is_empty() && matches!(report.status, PrChecksStatus::None) {
        ui::info("no CI checks configured for this PR, proceeding");
        return Ok(PrChecksStatus::None);
    }
    let status = wait_for_checks_inner(
        provider,
        repo_name,
        pr_number,
        Duration::from_secs(opts.timeout),
        REGISTRATION_GRACE,
        CHECKS_POLL_INTERVAL,
        InitialChecksState {
            expected: report.expected,
            head_sha: report.head_sha,
        },
    )
    .await?;
    if status == PrChecksStatus::Failed {
        return Err(RiptaskError::General(format!(
            "checks failed for PR #{pr_number}"
        )));
    }
    Ok(status)
}

async fn wait_for_checks_inner(
    provider: &dyn VersionControl,
    repo: &str,
    pr_number: u64,
    timeout: Duration,
    registration_grace: Duration,
    poll_interval: Duration,
    initial: InitialChecksState,
) -> Result<PrChecksStatus, RiptaskError> {
    let started = Instant::now();
    let spinner = ui::spinner("waiting for checks to register");
    let mut current_head_sha = initial.head_sha;
    let mut expected_set = initial.expected;
    let mut registered = std::collections::BTreeSet::<String>::new();
    let mut grace_started = Instant::now();

    loop {
        let report = match provider.get_pr_checks_report(repo, pr_number).await {
            Ok(report) => report,
            Err(e) => {
                finish_spinner(&spinner);
                return Err(e);
            }
        };

        for warning in &report.warnings {
            ui::warn(warning);
        }

        if report.head_sha != current_head_sha {
            current_head_sha = report.head_sha.clone();
            let next_expected = report.expected.clone();
            let next_expected_set = next_expected
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            let known_registered = report
                .registered
                .iter()
                .filter(|name| next_expected_set.contains(*name))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            // Reset grace whenever the new head still has expected checks that
            // have not registered on it yet. Comparing against the previous
            // head's registrations (the old behavior) misses the common case of
            // a rebase / new push where the expected names are unchanged but
            // none of them have registered on the new SHA yet.
            let missing_on_new_head = next_expected_set
                .iter()
                .any(|name| !known_registered.contains(name));
            expected_set = next_expected;
            registered = known_registered;
            if expected_set.is_empty() && matches!(report.status, PrChecksStatus::None) {
                finish_spinner(&spinner);
                ui::info("PR head changed; no CI checks on new head, proceeding");
                return Ok(PrChecksStatus::None);
            }
            if missing_on_new_head {
                grace_started = Instant::now();
            }
        }

        let expected_names = expected_set
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let report_registered = report
            .registered
            .iter()
            .filter(|name| expected_names.contains(*name))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        registered.extend(report_registered);

        let missing = expected_names
            .difference(&registered)
            .cloned()
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            update_spinner(
                &spinner,
                &format!("registering: {}/{}", registered.len(), expected_set.len()),
            );
            if grace_started.elapsed() >= registration_grace {
                finish_spinner(&spinner);
                return Err(RiptaskError::General(format!(
                    "required checks did not register: [{}]",
                    missing.join(", ")
                )));
            }
        } else {
            let finished = report
                .items
                .iter()
                .filter(|item| expected_names.contains(&item.name))
                .filter(|item| {
                    matches!(item.state, PrChecksStatus::Passed | PrChecksStatus::Failed)
                })
                .count();
            update_spinner(
                &spinner,
                &format!("checks: {}/{}", finished, expected_set.len()),
            );

            if report.items.iter().any(|item| {
                expected_names.contains(&item.name) && item.state == PrChecksStatus::Failed
            }) || report.status == PrChecksStatus::Failed
            {
                finish_spinner(&spinner);
                let elapsed = ui::format_elapsed(started.elapsed());
                ui::error(&format!("checks failed ({elapsed})"));
                return Ok(PrChecksStatus::Failed);
            }

            if report.status == PrChecksStatus::Passed {
                finish_spinner(&spinner);
                let elapsed = ui::format_elapsed(started.elapsed());
                ui::success(&format!("checks passed ({elapsed})"));
                return Ok(PrChecksStatus::Passed);
            }

            if expected_set.is_empty() && matches!(report.status, PrChecksStatus::None) {
                finish_spinner(&spinner);
                ui::info("no CI checks configured for this PR, proceeding");
                return Ok(PrChecksStatus::None);
            }
        }

        if started.elapsed() >= timeout {
            finish_spinner(&spinner);
            return Err(RiptaskError::General(format!(
                "timed out after {}s waiting for checks on PR #{pr_number}",
                timeout.as_secs()
            )));
        }

        if poll_interval.is_zero() {
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(poll_interval).await;
        }
    }
}

#[cfg(test)]
fn report_from_status(
    status: PrChecksStatus,
    head_sha: &str,
    expected: &[String],
    registered: &[String],
) -> PrChecksReport {
    let items = expected
        .iter()
        .chain(registered.iter())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|name| PrChecksReportItemState {
            name,
            state: status,
            url: None,
        })
        .collect::<Vec<_>>();
    PrChecksReport {
        head_sha: head_sha.to_owned(),
        expected: expected.to_vec(),
        registered: registered.to_vec(),
        items: items
            .into_iter()
            .map(|item| crate::adapters::backend::PrCheckItem {
                name: item.name,
                state: item.state,
                url: item.url,
            })
            .collect(),
        status,
        warnings: Vec::new(),
    }
}

#[cfg(test)]
struct PrChecksReportItemState {
    name: String,
    state: PrChecksStatus,
    url: Option<String>,
}

async fn wait_for_pr_head_update(
    provider: &dyn VersionControl,
    repo: &str,
    pr_number: u64,
    expected_sha: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<(), RiptaskError> {
    let started = Instant::now();
    loop {
        let pr = provider.get_pr(repo, pr_number).await?;
        if pr.head_sha.as_deref() == Some(expected_sha) {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            return Err(RiptaskError::General(format!(
                "timed out waiting for PR #{pr_number} head to update after force push"
            )));
        }
        tokio::time::sleep(poll_interval).await;
    }
}

fn update_spinner(spinner: &Option<ProgressBar>, msg: &str) {
    match spinner {
        Some(pb) => pb.set_message(msg.to_owned()),
        None => ui::info(msg),
    }
}

fn finish_spinner(spinner: &Option<ProgressBar>) {
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }
}

pub(crate) fn merge_method_label(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Merge => "merge",
        MergeMethod::Squash => "squash",
        MergeMethod::Rebase => "rebase",
    }
}

pub(crate) fn pr_checks_status_label(status: PrChecksStatus) -> &'static str {
    match status {
        PrChecksStatus::None => "none",
        PrChecksStatus::Pending => "pending",
        PrChecksStatus::Passed => "passed",
        PrChecksStatus::Failed => "failed",
    }
}

fn default_title(issue_number: u64, issue_title: &str) -> String {
    format!("Solves issue \"#{issue_number} {issue_title}\"")
}

fn default_body(issue_number: u64) -> String {
    format!("Closes #{issue_number}")
}

fn build_create_ai_context(
    issue: &IssueDocument,
    git: &dyn GitBackend,
    repo: &Path,
    base: &str,
    branch: &str,
) -> Result<String, RiptaskError> {
    Ok(format!(
        "Issue: {} {}\n\nIssue body:\n{}\n\nCommits:\n{}\n\nDiff:\n{}",
        issue.frontmatter.id,
        issue.frontmatter.title,
        issue.body,
        git.log_between(repo, base, branch)?,
        git.diff_between(repo, base, branch)?,
    ))
}

fn build_update_ai_context(
    current: &crate::adapters::backend::BackendPrRecord,
    git: &dyn GitBackend,
    repo: &Path,
    base: &str,
    branch: &str,
) -> Result<String, RiptaskError> {
    Ok(format!(
        "Current PR title: {}\n\nCurrent PR body:\n{}\n\nCommits:\n{}\n\nDiff:\n{}",
        current.title,
        current.body,
        git.log_between(repo, base, branch)?,
        git.diff_between(repo, base, branch)?,
    ))
}

fn edit_buffer(title: &str, body: &str) -> Result<(String, String), RiptaskError> {
    let mut file = tempfile::NamedTempFile::new().map_err(RiptaskError::Io)?;
    write!(file, "{}", render_editor_buffer(title, body)).map_err(RiptaskError::Io)?;
    open_in_editor(file.path())?;
    let content = fs::read_to_string(file.path())?;
    parse_editor_buffer(&content)
}

fn render_editor_buffer(title: &str, body: &str) -> String {
    if body.is_empty() {
        format!("{title}\n")
    } else {
        format!("{title}\n\n{body}")
    }
}

fn parse_editor_buffer(content: &str) -> Result<(String, String), RiptaskError> {
    let normalized = content.replace("\r\n", "\n");
    let mut parts = normalized.splitn(2, '\n');
    let title = parts.next().unwrap_or_default().trim().to_owned();
    if title.is_empty() {
        return Err(RiptaskError::General("PR title cannot be empty".into()));
    }
    let rest = parts.next().unwrap_or_default();
    let body = if rest.is_empty() {
        String::new()
    } else if let Some(body) = rest.strip_prefix('\n') {
        body.to_owned()
    } else {
        return Err(RiptaskError::General(
            "expected a blank line between title and body".into(),
        ));
    };
    Ok((title, body))
}

fn open_in_editor(path: &Path) -> Result<(), RiptaskError> {
    let status = crate::services::editor::open_in_editor(path)?;
    if status.success() {
        Ok(())
    } else {
        Err(RiptaskError::General("editor exited unsuccessfully".into()))
    }
}

/// Check if a PR head ref matches a branch name. GitHub formats head as
/// `owner:branch`, GitLab uses just `branch`.
pub(crate) fn pr_head_matches(head: &str, branch: &str) -> bool {
    // GitHub: "owner:branch_name" — compare after the colon
    if let Some((_owner, head_branch)) = head.split_once(':') {
        head_branch == branch
    } else {
        head == branch
    }
}

pub(crate) fn parse_pr_number_from_url(url: &str) -> Option<u64> {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|segment| segment.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{
        InitialChecksState, MergeOptions, REPO_INIT_COMMIT_MESSAGE, bootstrap_empty_remote,
        default_body, default_title, handle_ci_checks, merge_method_label, parse_editor_buffer,
        parse_pr_number_from_url, pr_checks_status_label, pr_head_matches, report_from_status,
        wait_for_checks_inner, wait_for_pr_head_update,
    };
    use crate::adapters::backend::{
        BackendPrRecord, BranchCreateOutcome, MergeMethod, PrChecksReport, PrChecksStatus,
        VersionControl,
    };
    use crate::adapters::git::GitBackend;
    use crate::error::RiptaskError;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct FakeChecksProvider {
        reports: Arc<Mutex<VecDeque<PrChecksReport>>>,
        polls: Arc<Mutex<usize>>,
        head_shas: Arc<Mutex<VecDeque<String>>>,
    }

    impl FakeChecksProvider {
        fn new(reports: Vec<PrChecksReport>) -> Self {
            Self {
                reports: Arc::new(Mutex::new(VecDeque::from(reports))),
                polls: Arc::new(Mutex::new(0)),
                head_shas: Arc::new(Mutex::new(VecDeque::new())),
            }
        }

        fn with_head_shas(mut self, shas: Vec<String>) -> Self {
            self.head_shas = Arc::new(Mutex::new(VecDeque::from(shas)));
            self
        }

        fn poll_count(&self) -> usize {
            *self.polls.lock().expect("lock")
        }
    }

    #[async_trait]
    impl VersionControl for FakeChecksProvider {
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

        async fn get_pr(&self, _repo: &str, number: u64) -> Result<BackendPrRecord, RiptaskError> {
            let mut head_shas = self.head_shas.lock().expect("lock");
            let head_sha = if head_shas.len() > 1 {
                Some(head_shas.pop_front().expect("head_sha"))
            } else {
                head_shas.front().cloned()
            };
            Ok(BackendPrRecord {
                number,
                title: "Test PR".into(),
                body: String::new(),
                url: "https://example.com/pr/42".into(),
                state: "open".into(),
                head: "test-branch".into(),
                head_sha,
                base: "main".into(),
                node_id: None,
                merged: false,
                updated_at: String::new(),
            })
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
            unimplemented!()
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

        async fn get_pr_checks_report(
            &self,
            _repo: &str,
            _number: u64,
        ) -> Result<PrChecksReport, RiptaskError> {
            *self.polls.lock().expect("lock") += 1;
            let mut reports = self.reports.lock().expect("lock");
            let report = if reports.len() > 1 {
                reports.pop_front().expect("report")
            } else {
                reports.front().cloned().expect("report")
            };
            Ok(report)
        }

        async fn create_branch(
            &self,
            _repo: &str,
            _branch_name: &str,
            _base_ref: &str,
            _issue_id: Option<u64>,
        ) -> Result<BranchCreateOutcome, RiptaskError> {
            unimplemented!()
        }

        async fn default_branch(&self, _repo: &str) -> Result<String, RiptaskError> {
            unimplemented!()
        }

        async fn delete_branch(&self, _repo: &str, _branch_name: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
    }

    fn merge_options(yes: bool, timeout: u64) -> MergeOptions {
        MergeOptions {
            merge_method: MergeMethod::Rebase,
            auto_merge: false,
            yes,
            timeout,
            force_push: false,
        }
    }

    #[derive(Clone, Default)]
    struct FakeGit {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl FakeGit {
        fn record(&self, call: impl Into<String>) {
            self.calls.lock().expect("lock").push(call.into());
        }
    }

    impl GitBackend for FakeGit {
        fn init(&self, _path: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn add(&self, _repo: &Path, _files: &[&Path]) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn commit(&self, _repo: &Path, _message: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn repo_root(&self, _cwd: &Path) -> Result<std::path::PathBuf, RiptaskError> {
            unimplemented!()
        }
        fn merge_file(
            &self,
            _local: &Path,
            _base: &Path,
            _remote: &Path,
        ) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn has_changes(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn has_uncommitted_changes(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn pull(&self, _repo: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn push(&self, _repo: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn checkout(&self, _repo: &Path, _branch: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn create_branch(&self, _repo: &Path, _name: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn ensure_on_local_branch(&self, _repo: &Path, _slug: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn branch_exists(&self, _repo: &Path, _name: &str) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn fetch_and_checkout_tracking(
            &self,
            _repo: &Path,
            _branch: &str,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn push_with_upstream(&self, _repo: &Path, branch: &str) -> Result<(), RiptaskError> {
            self.record(format!("push_with_upstream:{branch}"));
            Ok(())
        }
        fn has_working_tree_changes(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn diff_names(&self, _repo: &Path) -> Result<Vec<String>, RiptaskError> {
            unimplemented!()
        }
        fn current_branch(&self, _repo: &Path) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn remote_url(&self, _repo: &Path, _remote: &str) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn delete_local_branch(
            &self,
            _repo: &Path,
            _branch: &str,
            _force: bool,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn has_staged_changes(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn stage_all(&self, _repo: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn is_branch_merged(
            &self,
            _repo: &Path,
            _branch: &str,
            _base: &str,
        ) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn fetch(&self, _repo: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn commits_ahead_of_base(
            &self,
            _repo: &Path,
            _branch: &str,
            _base: &str,
        ) -> Result<u64, RiptaskError> {
            unimplemented!()
        }
        fn create_empty_commit(&self, _repo: &Path, _message: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn push_orphan_initial_branch(
            &self,
            _repo: &Path,
            ref_name: &str,
            message: &str,
        ) -> Result<(), RiptaskError> {
            self.record(format!("push_orphan_initial_branch:{ref_name}:{message}"));
            Ok(())
        }
        fn find_commit_by_subject(
            &self,
            _repo: &Path,
            _branch: &str,
            _base: &str,
            _subject: &str,
        ) -> Result<Option<String>, RiptaskError> {
            unimplemented!()
        }
        fn rebase_drop_commit(
            &self,
            _repo: &Path,
            _commit_sha: &str,
            _branch: &str,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn force_push_with_lease(&self, _repo: &Path, _branch: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn force_push(&self, _repo: &Path, _branch: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn log_between(
            &self,
            _repo: &Path,
            _base: &str,
            _head: &str,
        ) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn diff_between(
            &self,
            _repo: &Path,
            _base: &str,
            _head: &str,
        ) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn working_tree_diff(&self, _repo: &Path) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn staged_diff(&self, _repo: &Path) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn head_sha(&self, _repo: &Path) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn has_head_commit(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn remote_has_any_refs(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn clone_with_reference(
            &self,
            _reference_repo: &Path,
            _remote_url: &str,
            _target_dir: &Path,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn stash_push(&self, _repo: &Path, _message: &str) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn stash_pop(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
    }

    #[test]
    fn parses_github_pr_number_from_url() {
        assert_eq!(
            parse_pr_number_from_url("https://github.com/owner/repo/pull/123"),
            Some(123)
        );
    }

    #[test]
    fn parses_gitlab_mr_number_from_url() {
        assert_eq!(
            parse_pr_number_from_url("https://gitlab.com/group/project/-/merge_requests/456"),
            Some(456)
        );
    }

    #[test]
    fn builds_default_title_and_body() {
        assert_eq!(
            default_title(42, "Implement feature"),
            "Solves issue \"#42 Implement feature\""
        );
        assert_eq!(default_body(42), "Closes #42");
    }

    #[test]
    fn parses_editor_buffer_with_body() {
        let parsed =
            parse_editor_buffer("PR Title\n\nLine one\nLine two").expect("parse editor buffer");
        assert_eq!(parsed.0, "PR Title");
        assert_eq!(parsed.1, "Line one\nLine two");
    }

    #[test]
    fn parses_editor_buffer_without_body() {
        let parsed = parse_editor_buffer("PR Title\n").expect("parse editor buffer");
        assert_eq!(parsed.0, "PR Title");
        assert!(parsed.1.is_empty());
    }

    #[test]
    fn pr_head_matches_github_format() {
        assert!(pr_head_matches("owner:42-fix-bug", "42-fix-bug"));
        assert!(!pr_head_matches("owner:42-fix-bug", "fix-bug"));
    }

    #[test]
    fn pr_head_matches_gitlab_format() {
        assert!(pr_head_matches("42-fix-bug", "42-fix-bug"));
        assert!(!pr_head_matches("42-fix-bug", "fix-bug"));
    }

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

    #[tokio::test(flavor = "current_thread")]
    async fn empty_expected_skips_immediately() {
        let provider = FakeChecksProvider::new(vec![report_from_status(
            PrChecksStatus::None,
            "sha-1",
            &[],
            &[],
        )]);
        let started = Instant::now();

        let result = handle_ci_checks(&provider, "owner/repo", 42, &merge_options(false, 1)).await;

        assert_eq!(result.expect("skip"), PrChecksStatus::None);
        assert!(started.elapsed() < Duration::from_millis(10));
        assert_eq!(provider.poll_count(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn checks_register_late_then_pass() {
        let provider = FakeChecksProvider::new(vec![
            report_from_status(PrChecksStatus::Pending, "sha-1", &["ci".into()], &[]),
            report_from_status(
                PrChecksStatus::Pending,
                "sha-1",
                &["ci".into()],
                &["ci".into()],
            ),
            report_from_status(
                PrChecksStatus::Passed,
                "sha-1",
                &["ci".into()],
                &["ci".into()],
            ),
        ]);
        let result = wait_for_checks_inner(
            &provider,
            "owner/repo",
            42,
            Duration::from_millis(200),
            Duration::from_millis(30),
            Duration::from_millis(10),
            InitialChecksState {
                expected: vec!["ci".into()],
                head_sha: "sha-1".into(),
            },
        )
        .await;

        assert_eq!(result.expect("checks should pass"), PrChecksStatus::Passed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_required_check_fails_after_registration_grace() {
        let provider = FakeChecksProvider::new(vec![report_from_status(
            PrChecksStatus::Pending,
            "sha-1",
            &["ci".into()],
            &[],
        )]);

        let result = wait_for_checks_inner(
            &provider,
            "owner/repo",
            42,
            Duration::from_millis(100),
            Duration::from_millis(10),
            Duration::from_millis(10),
            InitialChecksState {
                expected: vec!["ci".into()],
                head_sha: "sha-1".into(),
            },
        )
        .await;

        let error = result.expect_err("checks should fail registration");
        assert!(
            error
                .to_string()
                .contains("required checks did not register"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn head_sha_change_restarts_registration_and_can_skip_new_head() {
        let provider = FakeChecksProvider::new(vec![
            report_from_status(PrChecksStatus::Pending, "sha-1", &["ci".into()], &[]),
            report_from_status(PrChecksStatus::None, "sha-2", &[], &[]),
        ]);

        let result = wait_for_checks_inner(
            &provider,
            "owner/repo",
            42,
            Duration::from_millis(100),
            Duration::from_millis(25),
            Duration::from_millis(10),
            InitialChecksState {
                expected: vec!["ci".into()],
                head_sha: "sha-1".into(),
            },
        )
        .await;

        assert_eq!(result.expect("no checks"), PrChecksStatus::None);
    }

    // Regression: after a force-push the expected check names typically stay
    // the same, but the new SHA has zero registrations until CI re-fires.
    // The grace window must reset so we give the new head its own grace
    // period instead of failing on "did not register" because the old
    // head's grace has already expired.
    //
    // Timing is calibrated so the *old* (pre-fix) code would observe
    // elapsed_since(grace_started) >= grace while ci is still unregistered
    // on sha-2, while the *new* (post-fix) code resets grace on the SHA
    // drift and stays within the grace until ci registers. The window has
    // some slack so that scheduling jitter does not flip the outcome.
    #[tokio::test(flavor = "current_thread")]
    async fn head_sha_change_with_same_expected_resets_grace() {
        // Poll cadence relative to t=0 (loop start):
        //   poll 1 ~  0ms  sha-1, ci registered, pending
        //   poll 2 ~ 40ms  sha-2 drift, nothing registered yet
        //   poll 3 ~ 80ms  sha-2, nothing registered yet
        //   poll 4 ~120ms  sha-2, ci registered, pending
        //   poll 5 ~160ms  sha-2, passed
        //
        // grace = 70ms:
        //   OLD code: grace_started stays at t=0. At poll 3 (t~80ms) the
        //   missing set is still {ci} and 80ms >= 70ms -> fails with
        //   "required checks did not register".
        //   NEW code: grace_started resets at poll 2 (t~40ms). At poll 3
        //   (t~80ms) elapsed_since_reset is ~40ms < 70ms, so the loop
        //   continues; at poll 4 ci registers (missing empty); poll 5 passes.
        let provider = FakeChecksProvider::new(vec![
            report_from_status(
                PrChecksStatus::Pending,
                "sha-1",
                &["ci".into()],
                &["ci".into()],
            ),
            report_from_status(PrChecksStatus::Pending, "sha-2", &["ci".into()], &[]),
            report_from_status(PrChecksStatus::Pending, "sha-2", &["ci".into()], &[]),
            report_from_status(
                PrChecksStatus::Pending,
                "sha-2",
                &["ci".into()],
                &["ci".into()],
            ),
            report_from_status(
                PrChecksStatus::Passed,
                "sha-2",
                &["ci".into()],
                &["ci".into()],
            ),
        ]);

        let result = wait_for_checks_inner(
            &provider,
            "owner/repo",
            42,
            Duration::from_millis(2_000),
            Duration::from_millis(70),
            Duration::from_millis(40),
            InitialChecksState {
                expected: vec!["ci".into()],
                head_sha: "sha-1".into(),
            },
        )
        .await;

        assert_eq!(
            result.expect("grace should reset on SHA drift and checks should pass"),
            PrChecksStatus::Passed,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pr_head_update_waits_for_matching_sha() {
        let provider = FakeChecksProvider::new(vec![report_from_status(
            PrChecksStatus::None,
            "old-sha",
            &[],
            &[],
        )])
        .with_head_shas(vec![
            "old-sha".into(),
            "older-sha".into(),
            "expected-sha".into(),
        ]);

        let result = wait_for_pr_head_update(
            &provider,
            "owner/repo",
            42,
            "expected-sha",
            Duration::from_millis(50),
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_ok(), "expected success, got {result:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pr_head_update_times_out_on_stale_sha() {
        let provider = FakeChecksProvider::new(vec![report_from_status(
            PrChecksStatus::None,
            "old-sha",
            &[],
            &[],
        )])
        .with_head_shas(vec!["old-sha".into()]);

        let result = wait_for_pr_head_update(
            &provider,
            "owner/repo",
            42,
            "expected-sha",
            Duration::from_millis(30),
            Duration::from_millis(10),
        )
        .await;

        let error = result.expect_err("expected timeout");
        assert!(
            error
                .to_string()
                .contains("timed out waiting for PR #42 head to update after force push"),
            "unexpected error: {error}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pr_head_update_passes_immediately_when_sha_matches() {
        let provider = FakeChecksProvider::new(vec![report_from_status(
            PrChecksStatus::None,
            "expected-sha",
            &[],
            &[],
        )])
        .with_head_shas(vec!["expected-sha".into()]);

        let started = Instant::now();
        let result = wait_for_pr_head_update(
            &provider,
            "owner/repo",
            42,
            "expected-sha",
            Duration::from_millis(50),
            Duration::from_millis(10),
        )
        .await;

        assert!(result.is_ok(), "expected immediate success, got {result:?}");
        assert!(
            started.elapsed() < Duration::from_millis(10),
            "expected immediate return"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn handle_ci_checks_waits_when_expected_checks_exist() {
        let provider = FakeChecksProvider::new(vec![
            report_from_status(PrChecksStatus::Pending, "sha-1", &["ci".into()], &[]),
            report_from_status(
                PrChecksStatus::Pending,
                "sha-1",
                &["ci".into()],
                &["ci".into()],
            ),
            report_from_status(
                PrChecksStatus::Passed,
                "sha-1",
                &["ci".into()],
                &["ci".into()],
            ),
        ]);

        let result = handle_ci_checks(&provider, "owner/repo", 42, &merge_options(false, 1)).await;

        assert_eq!(result.expect("checks should pass"), PrChecksStatus::Passed);
        assert!(provider.poll_count() > 1);
    }

    #[test]
    fn bootstrap_empty_remote_pushes_orphan_then_branch() {
        let git = FakeGit::default();
        bootstrap_empty_remote(&git, Path::new("."), "main", "123-test").expect("bootstrap");
        let expected = vec![
            format!("push_orphan_initial_branch:main:{REPO_INIT_COMMIT_MESSAGE}"),
            "push_with_upstream:123-test".to_string(),
        ];
        assert_eq!(
            git.calls.lock().expect("lock").as_slice(),
            expected.as_slice()
        );
    }

    #[test]
    fn bootstrap_empty_remote_uses_remote_default_branch_name() {
        // The bootstrap pushes to whatever the remote's configured default
        // branch is, NOT the local symbolic-ref. This means the user can be
        // on `1-foo` locally and the remote default can be `develop` and
        // bootstrap still creates `refs/heads/develop` on the remote.
        let git = FakeGit::default();
        bootstrap_empty_remote(&git, Path::new("."), "develop", "1-foo").expect("bootstrap");
        let expected = vec![
            format!("push_orphan_initial_branch:develop:{REPO_INIT_COMMIT_MESSAGE}"),
            "push_with_upstream:1-foo".to_string(),
        ];
        assert_eq!(
            git.calls.lock().expect("lock").as_slice(),
            expected.as_slice()
        );
    }
}
