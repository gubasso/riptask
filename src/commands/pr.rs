use crate::adapters::ai::AiBackend;
use crate::adapters::backend::{BackendPrRecord, MergeMethod, PrChecksStatus, VersionControl};
use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::{
    PrArgs, PrCreateArgs, PrEditArgs, PrMergeArgs, PrShowArgs, PrSubcommand, ScopeArgs,
};
use crate::commands::ai::{AI_BACKEND_MISSING, optional_backend};
use crate::commands::branch::{backend_issue_number, current_repo, cwd_utf8};
use crate::config::{Config, load_config};
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
const CHECKS_POLL_INTERVAL: Duration = Duration::from_secs(10);

pub async fn run(paths: &AppPaths, args: PrArgs) -> Result<(), RiptaskError> {
    match args.subcommand {
        Some(PrSubcommand::Create(args)) => create(paths, args).await,
        Some(PrSubcommand::Edit(args)) => edit(paths, args).await,
        Some(PrSubcommand::Show(args)) => show(paths, args).await,
        Some(PrSubcommand::Merge(args)) => merge(paths, args).await,
        None => {
            create(
                paths,
                PrCreateArgs {
                    scope: args.scope,
                    id: args.id,
                    no_ai: args.no_ai,
                },
            )
            .await
        }
    }
}

pub(crate) async fn create(paths: &AppPaths, args: PrCreateArgs) -> Result<(), RiptaskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
    let repo = current_repo()?;
    let current_branch = CliGit::new().current_branch(repo.as_path())?;
    let (path, mut issue) = resolve_issue(paths, &config, &args.scope, args.id.as_deref())?;
    let branch = issue
        .frontmatter
        .branch
        .clone()
        .ok_or_else(|| RiptaskError::General("run `tsk branch` first".into()))?;
    if current_branch != branch {
        return Err(RiptaskError::General(
            "current branch does not match issue branch".into(),
        ));
    }

    let repo_project = resolve_hosted_repo_project(&config, &issue)?;
    let git = CliGit::with_auth(resolve_git_auth(
        &repo_project.vc_backend,
        &repo_project.name,
    ));
    let provider = build_hosted_provider(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let default_branch = ui::spin_on_async("Fetching default branch", async {
        provider.default_branch(repo_name).await
    })
    .await?;

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

    let mut skip_ai = false;
    if repo_project.vc_backend.kind == BackendKind::Github
        && git.commits_ahead_of_base(repo.as_path(), &branch, &default_branch)? == 0
    {
        if git.has_staged_changes(repo.as_path())? {
            return Err(RiptaskError::General(
                "branch has no commits but has staged changes; commit or unstage them first".into(),
            ));
        }
        ui::info("branch has no commits; creating empty commit for PR...");
        git.create_empty_commit(repo.as_path(), EMPTY_BRANCH_COMMIT_MESSAGE)?;
        ui::spin_on("Pushing to remote", || {
            git.push_with_upstream(repo.as_path(), &branch)
        })?;
        skip_ai = true;
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
            match ui::spin_on("Generating PR description", || {
                ai.generate_pr_description(&context)
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

async fn show(paths: &AppPaths, args: PrShowArgs) -> Result<(), RiptaskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
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
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
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
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
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
                    match ui::spin_on("Generating PR description update", || {
                        ai.update_pr_description(&context)
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
            // Ensure we are on the issue branch before rewriting history
            let current = git.current_branch(repo_path)?;
            if current != branch {
                git.checkout(repo_path, branch)?;
            }
            ui::info("dropping empty bootstrap commit...");
            git.rebase_drop_commit(repo_path, &sha, branch)?;

            if git.commits_ahead_of_base(repo_path, branch, &default_branch)? == 0 {
                return Err(RiptaskError::General(
                    "branch has no real commits; nothing to merge".into(),
                ));
            }

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
        handle_ci_checks(
            provider,
            prompts,
            repo_path,
            repo_name,
            pr_number,
            &backend_kind,
            opts,
        )
        .await?;
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
            "riptsk: pr {} - {}",
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

fn has_local_ci(repo_path: &Path, backend_kind: &BackendKind) -> bool {
    match backend_kind {
        BackendKind::Github => has_local_github_ci(repo_path),
        BackendKind::Gitlab => has_local_gitlab_ci(repo_path),
        BackendKind::Jira => false,
        BackendKind::Local => false,
    }
}

fn has_local_github_ci(repo_path: &Path) -> bool {
    !local_github_workflow_names(repo_path).is_empty()
}

fn has_local_gitlab_ci(repo_path: &Path) -> bool {
    repo_path.join(".gitlab-ci.yml").is_file()
}

fn local_github_workflow_names(repo_path: &Path) -> Vec<String> {
    let workflow_dir = repo_path.join(".github").join("workflows");
    let Ok(entries) = fs::read_dir(workflow_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml" | "yaml")
            )
        })
        .filter_map(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().to_string())
        })
        .collect();
    names.sort();
    names
}

async fn handle_ci_checks(
    provider: &dyn VersionControl,
    prompts: &dyn PromptBackend,
    repo_path: &Path,
    repo_name: &str,
    pr_number: u64,
    backend_kind: &BackendKind,
    opts: &MergeOptions,
) -> Result<PrChecksStatus, RiptaskError> {
    let has_local = has_local_ci(repo_path, backend_kind);
    let presence = provider.get_ci_presence(repo_name).await?;
    match (has_local, presence.has_remote_ci) {
        (false, false) => {
            ui::info("no CI configured, proceeding");
            Ok(PrChecksStatus::None)
        }
        (false, true) => if opts.yes {
            ui::warn("remote CI detected without local CI config; waiting for checks");
            wait_for_checks(provider, repo_name, pr_number, opts.timeout).await
        } else if prompts.confirm(
            "remote CI detected but no local CI config found. Wait for remote CI before merging?",
            true,
        )? {
            wait_for_checks(provider, repo_name, pr_number, opts.timeout).await
        } else {
            ui::warn("bypassing remote CI checks");
            Ok(PrChecksStatus::None)
        },
        (true, false) | (true, true) => {
            wait_for_checks(provider, repo_name, pr_number, opts.timeout).await
        }
    }
}

async fn wait_for_checks(
    provider: &dyn VersionControl,
    repo: &str,
    pr_number: u64,
    timeout_secs: u64,
) -> Result<PrChecksStatus, RiptaskError> {
    wait_for_checks_inner(
        provider,
        repo,
        pr_number,
        Duration::from_secs(timeout_secs),
        CHECKS_POLL_INTERVAL,
    )
    .await
}

async fn wait_for_checks_inner(
    provider: &dyn VersionControl,
    repo: &str,
    pr_number: u64,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<PrChecksStatus, RiptaskError> {
    let started = Instant::now();
    let spinner = ui::spinner("waiting for checks to register");
    let mut saw_checks = false;

    loop {
        let status = match provider.get_pr_checks_status(repo, pr_number).await {
            Ok(s) => s,
            Err(e) => {
                finish_spinner(&spinner);
                return Err(e);
            }
        };

        match status {
            PrChecksStatus::Passed => {
                finish_spinner(&spinner);
                let elapsed = ui::format_elapsed(started.elapsed());
                ui::success(&format!("checks passed ({elapsed})"));
                return Ok(status);
            }
            PrChecksStatus::Failed => {
                finish_spinner(&spinner);
                let elapsed = ui::format_elapsed(started.elapsed());
                ui::error(&format!("checks failed ({elapsed})"));
                return Err(RiptaskError::General(format!(
                    "checks failed for PR #{pr_number}"
                )));
            }
            PrChecksStatus::Pending => {
                saw_checks = true;
                update_spinner(&spinner, "checks: pending");
            }
            PrChecksStatus::None => {
                if saw_checks {
                    update_spinner(&spinner, "checks: waiting for checks to reappear");
                } else {
                    update_spinner(&spinner, "checks: waiting for checks to register");
                }
            }
        }
        if started.elapsed() >= timeout {
            finish_spinner(&spinner);
            if saw_checks {
                return Err(RiptaskError::General(format!(
                    "timed out after {}s waiting for checks on PR #{pr_number}",
                    timeout.as_secs()
                )));
            }
            ui::info("no CI checks detected, proceeding");
            return Ok(PrChecksStatus::None);
        }
        tokio::time::sleep(poll_interval).await;
    }
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
        MergeOptions, default_body, default_title, handle_ci_checks, merge_method_label,
        parse_editor_buffer, parse_pr_number_from_url, pr_checks_status_label, pr_head_matches,
        wait_for_checks_inner, wait_for_pr_head_update,
    };
    use crate::adapters::backend::{
        BackendPrRecord, CiPresence, MergeMethod, PrChecksStatus, VersionControl,
    };
    use crate::adapters::prompts::PromptBackend;
    use crate::error::RiptaskError;
    use crate::models::BackendKind;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    struct FakeChecksProvider {
        statuses: Arc<Mutex<VecDeque<PrChecksStatus>>>,
        ci_presence: CiPresence,
        polls: Arc<Mutex<usize>>,
        head_shas: Arc<Mutex<VecDeque<String>>>,
    }

    impl FakeChecksProvider {
        fn new(statuses: Vec<PrChecksStatus>) -> Self {
            Self {
                statuses: Arc::new(Mutex::new(VecDeque::from(statuses))),
                ci_presence: CiPresence {
                    has_remote_ci: false,
                    remote_workflow_names: Vec::new(),
                },
                polls: Arc::new(Mutex::new(0)),
                head_shas: Arc::new(Mutex::new(VecDeque::new())),
            }
        }

        fn with_ci_presence(mut self, ci_presence: CiPresence) -> Self {
            self.ci_presence = ci_presence;
            self
        }

        fn with_head_shas(mut self, shas: Vec<String>) -> Self {
            self.head_shas = Arc::new(Mutex::new(VecDeque::from(shas)));
            self
        }

        fn poll_count(&self) -> usize {
            *self.polls.lock().expect("lock")
        }
    }

    #[derive(Clone)]
    struct FakePrompts {
        confirms: Arc<Mutex<VecDeque<bool>>>,
        confirm_calls: Arc<Mutex<usize>>,
    }

    impl FakePrompts {
        fn new(confirms: Vec<bool>) -> Self {
            Self {
                confirms: Arc::new(Mutex::new(VecDeque::from(confirms))),
                confirm_calls: Arc::new(Mutex::new(0)),
            }
        }

        fn confirm_calls(&self) -> usize {
            *self.confirm_calls.lock().expect("lock")
        }
    }

    impl PromptBackend for FakePrompts {
        fn input(&self, _prompt: &str, _default: Option<&str>) -> Result<String, RiptaskError> {
            unimplemented!()
        }

        fn confirm(&self, _prompt: &str, _default: bool) -> Result<bool, RiptaskError> {
            *self.confirm_calls.lock().expect("lock") += 1;
            self.confirms
                .lock()
                .expect("lock")
                .pop_front()
                .ok_or_else(|| RiptaskError::General("missing confirm response".into()))
        }

        fn select(
            &self,
            _prompt: &str,
            _items: &[String],
            _default: usize,
        ) -> Result<String, RiptaskError> {
            unimplemented!()
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

        async fn get_pr_checks_status(
            &self,
            _repo: &str,
            _number: u64,
        ) -> Result<PrChecksStatus, RiptaskError> {
            *self.polls.lock().expect("lock") += 1;
            let mut statuses = self.statuses.lock().expect("lock");
            let status = if statuses.len() > 1 {
                statuses.pop_front().expect("status")
            } else {
                statuses.front().copied().expect("status")
            };
            Ok(status)
        }

        async fn get_ci_presence(&self, _repo: &str) -> Result<CiPresence, RiptaskError> {
            Ok(self.ci_presence.clone())
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
    async fn no_checks_exits_after_timeout() {
        let provider = FakeChecksProvider::new(vec![PrChecksStatus::None]);
        let poll = Duration::from_millis(10);
        let timeout = Duration::from_millis(50);
        let started = Instant::now();

        let result = wait_for_checks_inner(&provider, "owner/repo", 42, timeout, poll).await;

        let elapsed = started.elapsed();
        assert_eq!(result.expect("no checks"), PrChecksStatus::None);
        assert!(
            elapsed >= timeout,
            "elapsed {elapsed:?} should be at least {timeout:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn checks_register_late_then_pass() {
        let provider = FakeChecksProvider::new(vec![
            PrChecksStatus::None,
            PrChecksStatus::None,
            PrChecksStatus::Pending,
            PrChecksStatus::Passed,
        ]);
        let result = wait_for_checks_inner(
            &provider,
            "owner/repo",
            42,
            Duration::from_millis(200),
            Duration::from_millis(10),
        )
        .await;

        assert_eq!(result.expect("checks should pass"), PrChecksStatus::Passed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn checks_disappear_after_seen_times_out() {
        let provider = FakeChecksProvider::new(vec![PrChecksStatus::Pending, PrChecksStatus::None]);
        let timeout = Duration::from_millis(200);
        let started = Instant::now();

        let result = wait_for_checks_inner(
            &provider,
            "owner/repo",
            42,
            timeout,
            Duration::from_millis(10),
        )
        .await;

        let elapsed = started.elapsed();
        let error = result.expect_err("checks should time out");
        assert!(
            error.to_string().contains("timed out"),
            "unexpected error: {error}"
        );
        assert!(
            elapsed >= timeout,
            "elapsed {elapsed:?} should be at least timeout {timeout:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn timeout_without_checks_proceeds() {
        let provider = FakeChecksProvider::new(vec![PrChecksStatus::None]);
        let timeout = Duration::from_millis(30);
        let started = Instant::now();

        let result = wait_for_checks_inner(
            &provider,
            "owner/repo",
            42,
            timeout,
            Duration::from_millis(10),
        )
        .await;

        let elapsed = started.elapsed();
        assert_eq!(result.expect("no checks"), PrChecksStatus::None);
        assert!(
            elapsed >= timeout,
            "elapsed {elapsed:?} should be at least timeout {timeout:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pr_head_update_waits_for_matching_sha() {
        let provider = FakeChecksProvider::new(vec![PrChecksStatus::None]).with_head_shas(vec![
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
        let provider = FakeChecksProvider::new(vec![PrChecksStatus::None])
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
        let provider = FakeChecksProvider::new(vec![PrChecksStatus::None])
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
    async fn no_ci_skips_immediately() {
        let provider =
            FakeChecksProvider::new(vec![PrChecksStatus::Pending]).with_ci_presence(CiPresence {
                has_remote_ci: false,
                remote_workflow_names: Vec::new(),
            });
        let prompts = FakePrompts::new(vec![]);
        let repo = tempfile::tempdir().expect("tempdir");

        let result = handle_ci_checks(
            &provider,
            &prompts,
            repo.path(),
            "owner/repo",
            42,
            &BackendKind::Github,
            &merge_options(false, 1),
        )
        .await;

        assert_eq!(result.expect("skip"), PrChecksStatus::None);
        assert_eq!(provider.poll_count(), 0);
        assert_eq!(prompts.confirm_calls(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn local_ci_no_remote_waits() {
        let provider =
            FakeChecksProvider::new(vec![PrChecksStatus::Pending, PrChecksStatus::Passed])
                .with_ci_presence(CiPresence {
                    has_remote_ci: false,
                    remote_workflow_names: Vec::new(),
                });
        let prompts = FakePrompts::new(vec![]);
        let repo = tempfile::tempdir().expect("tempdir");
        let workflow_dir = repo.path().join(".github").join("workflows");
        fs::create_dir_all(&workflow_dir).expect("create workflow dir");
        fs::write(workflow_dir.join("ci.yml"), "name: CI\n").expect("write workflow");

        let result = handle_ci_checks(
            &provider,
            &prompts,
            repo.path(),
            "owner/repo",
            42,
            &BackendKind::Github,
            &merge_options(false, 1),
        )
        .await;

        assert_eq!(result.expect("checks should pass"), PrChecksStatus::Passed);
        assert!(provider.poll_count() > 0);
        assert_eq!(prompts.confirm_calls(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_ci_no_local_prompts_wait() {
        let provider =
            FakeChecksProvider::new(vec![PrChecksStatus::Pending, PrChecksStatus::Passed])
                .with_ci_presence(CiPresence {
                    has_remote_ci: true,
                    remote_workflow_names: vec!["ci".into()],
                });
        let prompts = FakePrompts::new(vec![true]);
        let repo = tempfile::tempdir().expect("tempdir");

        let result = handle_ci_checks(
            &provider,
            &prompts,
            repo.path(),
            "owner/repo",
            42,
            &BackendKind::Github,
            &merge_options(false, 1),
        )
        .await;

        assert_eq!(result.expect("checks should pass"), PrChecksStatus::Passed);
        assert!(provider.poll_count() > 0);
        assert_eq!(prompts.confirm_calls(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn remote_ci_no_local_prompts_bypass() {
        let provider =
            FakeChecksProvider::new(vec![PrChecksStatus::Pending]).with_ci_presence(CiPresence {
                has_remote_ci: true,
                remote_workflow_names: vec!["ci".into()],
            });
        let prompts = FakePrompts::new(vec![false]);
        let repo = tempfile::tempdir().expect("tempdir");

        let result = handle_ci_checks(
            &provider,
            &prompts,
            repo.path(),
            "owner/repo",
            42,
            &BackendKind::Github,
            &merge_options(false, 1),
        )
        .await;

        assert_eq!(result.expect("bypass"), PrChecksStatus::None);
        assert_eq!(provider.poll_count(), 0);
        assert_eq!(prompts.confirm_calls(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn both_ci_waits_normally() {
        let provider =
            FakeChecksProvider::new(vec![PrChecksStatus::Pending, PrChecksStatus::Passed])
                .with_ci_presence(CiPresence {
                    has_remote_ci: true,
                    remote_workflow_names: vec!["ci".into()],
                });
        let prompts = FakePrompts::new(vec![]);
        let repo = tempfile::tempdir().expect("tempdir");
        let workflow_dir = repo.path().join(".github").join("workflows");
        fs::create_dir_all(&workflow_dir).expect("create workflow dir");
        fs::write(workflow_dir.join("ci.yml"), "name: CI\n").expect("write workflow");

        let result = handle_ci_checks(
            &provider,
            &prompts,
            repo.path(),
            "owner/repo",
            42,
            &BackendKind::Github,
            &merge_options(false, 1),
        )
        .await;

        assert_eq!(result.expect("checks should pass"), PrChecksStatus::Passed);
        assert!(provider.poll_count() > 0);
        assert_eq!(prompts.confirm_calls(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn yes_flag_skips_prompt() {
        let provider =
            FakeChecksProvider::new(vec![PrChecksStatus::Pending, PrChecksStatus::Passed])
                .with_ci_presence(CiPresence {
                    has_remote_ci: true,
                    remote_workflow_names: vec!["ci".into()],
                });
        let prompts = FakePrompts::new(vec![]);
        let repo = tempfile::tempdir().expect("tempdir");

        let result = handle_ci_checks(
            &provider,
            &prompts,
            repo.path(),
            "owner/repo",
            42,
            &BackendKind::Github,
            &merge_options(true, 1),
        )
        .await;

        assert_eq!(result.expect("checks should pass"), PrChecksStatus::Passed);
        assert!(provider.poll_count() > 0);
        assert_eq!(prompts.confirm_calls(), 0);
    }
}
