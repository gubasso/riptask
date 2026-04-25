use crate::cli::{
    ResolveArgs, SessionArgs, SessionStartArgs, SessionSubcommand, StoreCommitArgs, SyncArgs,
    SyncPullPushArgs, SyncSubcommand,
};
use crate::config::{Config, load_effective_config};
use crate::domain::session::SessionState;
use crate::error::RiptaskError;
use crate::models::{BackendKind, RepoProject};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::build_issue_tracker;
use crate::services::id_resolution;
use crate::services::sync_engine::SyncEngine;
use crate::storage::{frontmatter, issue_store, session as session_store};
use crate::{adapters::git::CliGit, adapters::git::GitBackend};
use anyhow::Context;
use console::style;
use std::collections::HashSet;
use std::io::IsTerminal;

pub async fn run(paths: &AppPaths, args: SyncArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    match args.subcommand.clone() {
        Some(SyncSubcommand::Pull(subargs)) => pull(paths, &args, &subargs).await,
        Some(SyncSubcommand::Push(subargs)) => push(paths, &args, &subargs).await,
        Some(SyncSubcommand::Status) => status(paths, &args),
        Some(SyncSubcommand::Resolve(resolve_args)) => resolve(paths, resolve_args),
        None => {
            // Bare `tsk sync` = pull then push (matches Bash behavior)
            pull(paths, &args, &SyncPullPushArgs::default()).await?;
            push(paths, &args, &SyncPullPushArgs::default()).await
        }
    }
}

async fn pull(
    paths: &AppPaths,
    args: &SyncArgs,
    subargs: &SyncPullPushArgs,
) -> Result<(), RiptaskError> {
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let engine = SyncEngine::new(paths, &config);
    for repo_project in resolve_sync_projects(args, &config)? {
        let provider = build_issue_tracker(repo_project)?;
        let pull_ids = resolve_pull_filter_ids(&subargs.ids, repo_project);
        let force_ids = resolve_pull_filter_ids(&args.force_pull_ids, repo_project);
        let summary = crate::ui::spin_on_async(
            &format!("Pulling issues from {}", repo_project.name),
            engine.pull(
                provider.as_ref(),
                repo_project,
                args.force,
                (!pull_ids.is_empty()).then_some(&pull_ids),
                (!force_ids.is_empty()).then_some(&force_ids),
            ),
        )
        .await?;
        tracing::info!(
            repo_project = %repo_project.name,
            created = summary.created.len(),
            updated = summary.updated.len(),
            deleted = summary.deleted.len(),
            conflicts = summary.conflicts.len(),
            "pull completed"
        );
        if std::io::stderr().is_terminal() {
            eprintln!(
                "pull  {} created  {} updated  {} deleted  {} conflicts",
                style(summary.created.len()).green(),
                style(summary.updated.len()).cyan(),
                style(summary.deleted.len()).yellow(),
                style(summary.conflicts.len()).red(),
            );
        } else {
            eprintln!(
                "pull: {} created, {} updated, {} deleted, {} conflicts",
                summary.created.len(),
                summary.updated.len(),
                summary.deleted.len(),
                summary.conflicts.len()
            );
        }
        for id in &summary.conflicts {
            if std::io::stderr().is_terminal() {
                eprintln!("{} {id}", style("CONFLICT").red().bold());
            } else {
                eprintln!("CONFLICT {id}");
            }
        }
    }
    Ok(())
}

async fn push(
    paths: &AppPaths,
    args: &SyncArgs,
    subargs: &SyncPullPushArgs,
) -> Result<(), RiptaskError> {
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let engine = SyncEngine::new(paths, &config);
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let resolved_ids = subargs
        .ids
        .iter()
        .map(|id| id_resolution::resolve_id(paths, &config, &cwd, id))
        .collect::<Result<Vec<_>, _>>()?;
    for repo_project in resolve_sync_projects(args, &config)? {
        let provider = build_issue_tracker(repo_project)?;
        let issue_paths = collect_push_paths(paths, repo_project, &resolved_ids)?;
        let summary = if resolved_ids.is_empty() {
            crate::ui::spin_on_async(
                &format!("Pushing issues to {}", repo_project.name),
                engine.push(provider.as_ref(), repo_project),
            )
            .await?
        } else {
            crate::ui::spin_on_async(
                &format!("Pushing issues to {}", repo_project.name),
                engine.push_issues(provider.as_ref(), repo_project, &issue_paths),
            )
            .await?
        };
        tracing::info!(
            repo_project = %repo_project.name,
            created = summary.created.len(),
            updated = summary.updated.len(),
            deleted = summary.deleted.len(),
            skipped = summary.skipped.len(),
            "push completed"
        );
        if std::io::stderr().is_terminal() {
            eprintln!(
                "push  {} created  {} updated  {} deleted  {} skipped",
                style(summary.created.len()).green(),
                style(summary.updated.len()).cyan(),
                style(summary.deleted.len()).yellow(),
                style(summary.skipped.len()).dim(),
            );
        } else {
            eprintln!(
                "push: {} created, {} updated, {} deleted, {} skipped",
                summary.created.len(),
                summary.updated.len(),
                summary.deleted.len(),
                summary.skipped.len()
            );
        }

        let mut file_refs = issue_paths
            .iter()
            .map(|path| path.as_std_path())
            .collect::<Vec<_>>();
        let config_path = paths.system_config_path();
        file_refs.push(config_path.as_std_path());
        maybe_auto_commit(
            &config,
            &crate::adapters::git::CliGit::new(),
            paths.riptask_repo.as_std_path(),
            &format!("riptask: push local issues to {}", repo_project.name),
            &file_refs,
        )?;
    }
    Ok(())
}

fn status(paths: &AppPaths, args: &SyncArgs) -> Result<(), RiptaskError> {
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let repo_projects = resolve_sync_projects(args, &config)?;
    let project_names = repo_projects
        .iter()
        .map(|repo_project| repo_project.name.as_str())
        .collect::<HashSet<_>>();
    let summary = SyncEngine::new(paths, &config).status()?;
    let tty = std::io::stdout().is_terminal();
    for id in summary.conflicts {
        if issue_matches_project_scope(paths, &id, &project_names)? {
            if tty {
                println!("{} {id}", style("CONFLICT").red().bold());
            } else {
                println!("CONFLICT {id}");
            }
        }
    }
    for id in summary.creates {
        if issue_matches_project_scope(paths, &id, &project_names)? {
            if tty {
                println!("{}   {id}", style("CREATE").green());
            } else {
                println!("CREATE {id}");
            }
        }
    }
    for id in summary.pushes {
        if issue_matches_project_scope(paths, &id, &project_names)? {
            if tty {
                println!("{}     {id}", style("PUSH").cyan());
            } else {
                println!("PUSH {id}");
            }
        }
    }
    for key in summary.deletes {
        if deleted_key_matches_project_scope(&key, &repo_projects) {
            if tty {
                println!("{}   {key}", style("DELETE").yellow());
            } else {
                println!("DELETE {key}");
            }
        }
    }
    Ok(())
}

fn resolve_sync_projects<'a>(
    args: &SyncArgs,
    config: &'a Config,
) -> Result<Vec<&'a RepoProject>, RiptaskError> {
    if let Some(name) = args.backend.as_deref() {
        return Ok(hosted_projects(config)
            .into_iter()
            .filter(|repo_project| repo_project.name == name)
            .collect());
    }
    if args.scope.all_projects {
        return Ok(hosted_projects(config));
    }
    if !args.scope.projects.is_empty() {
        return args
            .scope
            .projects
            .iter()
            .map(|project| {
                config
                    .projects
                    .iter()
                    .find(|repo_project| repo_project.name == *project)
                    .ok_or_else(|| RiptaskError::Unregistered(project.clone()))
            })
            .map(|result| {
                result.and_then(|repo_project| {
                    if matches!(
                        repo_project.tasks_backend.kind,
                        BackendKind::Github | BackendKind::Gitlab | BackendKind::Jira
                    ) {
                        Ok(repo_project)
                    } else {
                        Err(RiptaskError::Config(format!(
                            "sync target must be github, gitlab, or jira: {}",
                            repo_project.name
                        )))
                    }
                })
            })
            .collect();
    }

    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    if let Ok(Some(repo_project)) =
        crate::services::project_detection::detect_from_cwd(&cwd, config)
        && let Some(candidate) = config.projects.iter().find(|candidate| {
            candidate.name == repo_project.name
                && matches!(
                    candidate.tasks_backend.kind,
                    BackendKind::Github | BackendKind::Gitlab | BackendKind::Jira
                )
        })
    {
        return Ok(vec![candidate]);
    }

    Err(RiptaskError::Config(
        "could not detect project from current directory; use -p <project>, --backend <name>, or -a to target all projects".into(),
    ))
}

fn issue_matches_project_scope(
    paths: &AppPaths,
    id: &str,
    project_names: &HashSet<&str>,
) -> Result<bool, RiptaskError> {
    let path = crate::storage::issue_store::find_issue(paths, id)?;
    match crate::storage::frontmatter::try_load_issue(path.as_std_path()) {
        crate::storage::frontmatter::IssueLoadResult::Ok(issue) => {
            Ok(project_names.contains(issue.frontmatter.project.as_str()))
        }
        crate::storage::frontmatter::IssueLoadResult::Conflict { id, .. } => {
            for backup in [
                crate::storage::issue_store::local_backup_path(paths, &id),
                crate::storage::issue_store::remote_backup_path(paths, &id),
            ] {
                if !backup.exists() {
                    continue;
                }
                if let crate::storage::frontmatter::IssueLoadResult::Ok(issue) =
                    crate::storage::frontmatter::try_load_issue(backup.as_std_path())
                {
                    return Ok(project_names.contains(issue.frontmatter.project.as_str()));
                }
            }
            Ok(false)
        }
        crate::storage::frontmatter::IssueLoadResult::Err(error) => Err(RiptaskError::Other(error)),
    }
}

fn hosted_projects(config: &Config) -> Vec<&RepoProject> {
    config
        .projects
        .iter()
        .filter(|repo_project| {
            matches!(
                repo_project.tasks_backend.kind,
                BackendKind::Github | BackendKind::Gitlab | BackendKind::Jira
            )
        })
        .collect()
}

fn resolve_pull_filter_ids(ids: &[String], repo_project: &RepoProject) -> HashSet<String> {
    ids.iter()
        .map(|id| {
            if id.contains("--") {
                return id.clone();
            }
            if id.chars().all(|character| character.is_ascii_digit()) {
                let number = id.parse::<u64>().unwrap_or_default();
                return crate::services::issue_ids::format_id(
                    &crate::services::issue_ids::effective_key(repo_project),
                    number,
                );
            }
            id.clone()
        })
        .collect()
}

fn collect_push_paths(
    paths: &AppPaths,
    repo_project: &RepoProject,
    ids: &[String],
) -> Result<Vec<camino::Utf8PathBuf>, RiptaskError> {
    if !ids.is_empty() {
        let mut paths_to_push = Vec::new();
        for path in ids
            .iter()
            .map(|id| issue_store::find_issue(paths, id))
            .collect::<Result<Vec<_>, _>>()?
        {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { id, .. } => {
                    eprintln!("skipping {id}: unresolved sync conflict (tsk sync resolve {id})");
                    continue;
                }
                frontmatter::IssueLoadResult::Err(error) => return Err(RiptaskError::Other(error)),
            };
            if issue.frontmatter.project == repo_project.name {
                paths_to_push.push(path);
            }
        }
        return Ok(paths_to_push);
    }

    let mut paths_to_push = Vec::new();
    for path in issue_store::list_issues(paths)? {
        let issue = match frontmatter::try_load_issue(path.as_std_path()) {
            frontmatter::IssueLoadResult::Ok(issue) => issue,
            frontmatter::IssueLoadResult::Conflict { .. } => continue,
            frontmatter::IssueLoadResult::Err(error) => return Err(RiptaskError::Other(error)),
        };
        if issue.frontmatter.project != repo_project.name {
            continue;
        }
        paths_to_push.push(path);
    }
    Ok(paths_to_push)
}

fn deleted_key_matches_project_scope(key: &str, repo_projects: &[&RepoProject]) -> bool {
    let Some((provider, repo, _issue_id)) = parse_backend_state_key(key) else {
        return false;
    };
    repo_projects.iter().any(|repo_project| {
        provider_name(repo_project) == provider && sync_repo(repo_project).as_deref() == Some(repo)
    })
}

fn parse_backend_state_key(key: &str) -> Option<(&str, &str, u64)> {
    let (provider, rest) = key.split_once(':')?;
    let (repo, issue_id) = rest.rsplit_once(':')?;
    Some((provider, repo, issue_id.parse().ok()?))
}

fn provider_name(repo_project: &RepoProject) -> &'static str {
    match repo_project.tasks_backend.kind {
        BackendKind::Github => "github",
        BackendKind::Gitlab => "gitlab",
        BackendKind::Jira => "jira",
        BackendKind::Local => "local",
    }
}

fn sync_repo(repo_project: &RepoProject) -> Option<String> {
    match repo_project.tasks_backend.kind {
        BackendKind::Github | BackendKind::Gitlab => repo_project.tasks_backend.repo.clone(),
        BackendKind::Jira => repo_project.tasks_backend.jira_project.clone(),
        BackendKind::Local => None,
    }
}

pub fn resolve(paths: &AppPaths, args: ResolveArgs) -> Result<(), RiptaskError> {
    if args.take_remote && args.take_local {
        return Err(RiptaskError::General(
            "cannot specify both --take-remote and --take-local".into(),
        ));
    }
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let id = id_resolution::resolve_id(paths, &config, &cwd, &args.id)?;
    let path = crate::storage::issue_store::find_issue(paths, &id)?;
    let local_backup = crate::storage::issue_store::local_backup_path(paths, &id);
    let remote_backup = crate::storage::issue_store::remote_backup_path(paths, &id);
    if args.take_remote {
        if !remote_backup.exists() {
            return Err(RiptaskError::Conflict(format!("no remote backup for {id}")));
        }
        std::fs::copy(&remote_backup, &path)?;
        crate::storage::issue_store::delete_conflict_backups(paths, &id)?;
    } else if args.take_local {
        if !local_backup.exists() {
            return Err(RiptaskError::Conflict(format!("no local backup for {id}")));
        }
        std::fs::copy(&local_backup, &path)?;
        crate::storage::issue_store::delete_conflict_backups(paths, &id)?;
        bump_local_updated_at(&path)?;
    } else {
        let content = std::fs::read_to_string(&path)?;
        if crate::storage::frontmatter::has_conflict_markers(&content) {
            return Err(RiptaskError::Conflict(format!(
                "issue {id} still has unresolved conflict markers"
            )));
        }
        // Validate the file parses before deleting backups
        crate::storage::frontmatter::load_issue(path.as_std_path()).map_err(|_| {
            RiptaskError::Conflict(format!(
                "issue {id} has invalid frontmatter after conflict resolution; backups preserved"
            ))
        })?;
        crate::storage::issue_store::delete_conflict_backups(paths, &id)?;
        bump_local_updated_at(&path)?;
    }
    crate::services::view_builder::ViewBuilder::new(paths, &config)
        .regenerate_all(None)
        .map_err(RiptaskError::Other)?;
    Ok(())
}

fn bump_local_updated_at(path: &camino::Utf8PathBuf) -> Result<(), RiptaskError> {
    let mut issue =
        crate::storage::frontmatter::load_issue(path.as_std_path()).map_err(RiptaskError::Other)?;
    issue.frontmatter.local_updated_at = crate::services::issue_service::now_utc();
    crate::storage::frontmatter::save_issue(path.as_std_path(), &issue)
        .map_err(RiptaskError::Other)?;
    Ok(())
}

pub fn session(paths: &AppPaths, args: SessionArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    match args.subcommand {
        SessionSubcommand::Start(args) => session_start(paths, args),
        SessionSubcommand::End => session_end(paths),
    }
}

pub fn commit(paths: &AppPaths, args: StoreCommitArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let git = CliGit::new();
    let repo = paths.riptask_repo.as_std_path().to_path_buf();
    let tracked = [
        paths.riptask_repo.join("issues"),
        paths.riptask_repo.join("templates"),
        paths.system_config_path(),
    ];
    let refs = tracked
        .iter()
        .map(|path| path.as_std_path())
        .collect::<Vec<_>>();
    git.add(repo.as_path(), &refs)?;
    let message = format!(
        "riptask: manual commit {}",
        crate::services::issue_service::now_utc()
    );
    if args.edit {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .arg("commit")
            .arg("--edit")
            .arg("-m")
            .arg(&message)
            .status()
            .context("failed to run git commit")
            .map_err(RiptaskError::Other)?;
        if !status.success() {
            return Err(RiptaskError::General("git commit failed".into()));
        }
        return Ok(());
    }
    git.commit(repo.as_path(), &message)
}

fn session_start(paths: &AppPaths, args: SessionStartArgs) -> Result<(), RiptaskError> {
    if session_store::load_session(paths.session_state_path().as_std_path())?.is_some() {
        return Err(RiptaskError::Conflict("session already active".into()));
    }
    let SessionStartArgs { scope, id } = args;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::require_id(paths, &config, &cwd, id, &scope)? else {
        return Ok(());
    };
    let git = CliGit::new();
    let repo = current_repo()?;
    let previous_branch = git.current_branch(repo.as_path())?;
    let session_branch = {
        let path = crate::storage::issue_store::find_issue(paths, &id)?;
        let mut issue =
            crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        let branch = issue.frontmatter.id_slug.clone().unwrap_or_else(|| {
            crate::services::issue_service::generate_slug(
                &issue.frontmatter.id,
                &issue.frontmatter.title,
            )
        });
        if !git.branch_exists(repo.as_path(), &branch)? {
            git.create_branch(repo.as_path(), &branch)?;
        } else {
            git.checkout(repo.as_path(), &branch)?;
        }
        issue.frontmatter.branch = Some(branch.clone());
        crate::storage::frontmatter::save_issue(path.as_std_path(), &issue)
            .map_err(RiptaskError::Other)?;
        branch
    };
    session_store::save_session(
        paths.session_state_path().as_std_path(),
        &SessionState {
            previous_branch,
            session_branch: session_branch.clone(),
            issue_id: Some(id),
            started_at: crate::services::issue_service::now_utc(),
        },
    )?;
    println!("{session_branch}");
    Ok(())
}

fn session_end(paths: &AppPaths) -> Result<(), RiptaskError> {
    let Some(session) = session_store::load_session(paths.session_state_path().as_std_path())?
    else {
        return Err(RiptaskError::NotFound("no active session".into()));
    };
    let git = CliGit::new();
    let repo = current_repo()?;
    if git.has_working_tree_changes(paths.riptask_repo.as_std_path())? {
        commit(paths, StoreCommitArgs { edit: false })?;
    }
    git.checkout(repo.as_path(), &session.previous_branch)?;
    session_store::clear_session(paths.session_state_path().as_std_path())?;
    Ok(())
}

fn current_repo() -> Result<std::path::PathBuf, RiptaskError> {
    std::env::current_dir().map_err(RiptaskError::from)
}
