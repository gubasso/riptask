use crate::cli::{
    CommitArgs, ResolveArgs, SessionArgs, SessionStartArgs, SessionSubcommand, SyncArgs,
    SyncPullPushArgs, SyncSubcommand,
};
use crate::config::{Config, load_config};
use crate::domain::session::SessionState;
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::build_provider_for_backend;
use crate::services::id_resolution;
use crate::services::sync_engine::SyncEngine;
use crate::storage::{frontmatter, issue_store, session as session_store};
use crate::{adapters::git::CliGit, adapters::git::GitBackend};
use anyhow::Context;
use console::style;
use std::collections::HashSet;
use std::io::IsTerminal;

pub async fn run(paths: &AppPaths, args: SyncArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
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
) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let engine = SyncEngine::new(paths, &config);
    for backend in resolve_sync_backends(args, &config)? {
        let provider = build_provider_for_backend(backend)?;
        let pull_ids = resolve_pull_filter_ids(&subargs.ids, backend);
        let summary = engine
            .pull(
                provider.as_ref(),
                backend,
                args.force,
                (!pull_ids.is_empty()).then_some(&pull_ids),
            )
            .await?;
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
) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
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
    for backend in resolve_sync_backends(args, &config)? {
        let provider = build_provider_for_backend(backend)?;
        let issue_paths = collect_push_paths(paths, backend, &resolved_ids)?;
        let summary = if resolved_ids.is_empty() {
            engine.push(provider.as_ref(), backend).await?
        } else {
            engine
                .push_issues(provider.as_ref(), backend, &issue_paths)
                .await?
        };
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
        let config_path = paths.config_path();
        file_refs.push(config_path.as_std_path());
        maybe_auto_commit(
            &config,
            &crate::adapters::git::CliGit::new(),
            paths.riptsk_repo.as_std_path(),
            &format!("riptsk: push local issues to {}", backend.name),
            &file_refs,
        )?;
    }
    Ok(())
}

fn status(paths: &AppPaths, args: &SyncArgs) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let backends = resolve_sync_backends(args, &config)?;
    let backend_names = backends
        .iter()
        .map(|backend| backend.name.as_str())
        .collect::<HashSet<_>>();
    let summary = SyncEngine::new(paths, &config).status()?;
    let tty = std::io::stdout().is_terminal();
    for id in summary.conflicts {
        if issue_matches_backend_scope(paths, &id, &backend_names)? {
            if tty {
                println!("{} {id}", style("CONFLICT").red().bold());
            } else {
                println!("CONFLICT {id}");
            }
        }
    }
    for id in summary.creates {
        if issue_matches_backend_scope(paths, &id, &backend_names)? {
            if tty {
                println!("{}   {id}", style("CREATE").green());
            } else {
                println!("CREATE {id}");
            }
        }
    }
    for id in summary.pushes {
        if issue_matches_backend_scope(paths, &id, &backend_names)? {
            if tty {
                println!("{}     {id}", style("PUSH").cyan());
            } else {
                println!("PUSH {id}");
            }
        }
    }
    for key in summary.deletes {
        if deleted_key_matches_backend_scope(&key, &backends) {
            if tty {
                println!("{}   {key}", style("DELETE").yellow());
            } else {
                println!("DELETE {key}");
            }
        }
    }
    Ok(())
}

fn resolve_sync_backends<'a>(
    args: &SyncArgs,
    config: &'a Config,
) -> Result<Vec<&'a BackendConfig>, RiptskError> {
    if let Some(name) = args.backend.as_deref() {
        return Ok(hosted_backends(config)
            .into_iter()
            .filter(|backend| backend.name == name)
            .collect());
    }
    if args.scope.all_projects {
        return Ok(hosted_backends(config));
    }
    if !args.scope.projects.is_empty() {
        return args
            .scope
            .projects
            .iter()
            .map(|project| {
                config
                    .backends
                    .iter()
                    .find(|backend| backend.name == *project)
                    .ok_or_else(|| RiptskError::Unregistered(project.clone()))
            })
            .map(|result| {
                result.and_then(|backend| {
                    if matches!(backend.backend, Backend::Github | Backend::Gitlab) {
                        Ok(backend)
                    } else {
                        Err(RiptskError::Config(format!(
                            "sync target must be github or gitlab: {}",
                            backend.name
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
    if let Ok(Some(backend)) = crate::services::project_detection::detect_from_cwd(&cwd, config)
        && let Some(candidate) = config.backends.iter().find(|candidate| {
            candidate.name == backend.name
                && matches!(candidate.backend, Backend::Github | Backend::Gitlab)
        })
    {
        return Ok(vec![candidate]);
    }

    Err(RiptskError::Config(
        "could not detect project from current directory; use -p <project>, --backend <name>, or -a to target all projects".into(),
    ))
}

fn issue_matches_backend_scope(
    paths: &AppPaths,
    id: &str,
    backend_names: &HashSet<&str>,
) -> Result<bool, RiptskError> {
    let path = crate::storage::issue_store::find_issue(paths, id)?;
    match crate::storage::frontmatter::try_load_issue(path.as_std_path()) {
        crate::storage::frontmatter::IssueLoadResult::Ok(issue) => {
            Ok(backend_names.contains(issue.frontmatter.project.as_str()))
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
                    return Ok(backend_names.contains(issue.frontmatter.project.as_str()));
                }
            }
            Ok(false)
        }
        crate::storage::frontmatter::IssueLoadResult::Err(error) => Err(RiptskError::Other(error)),
    }
}

fn hosted_backends(config: &Config) -> Vec<&BackendConfig> {
    config
        .backends
        .iter()
        .filter(|backend| matches!(backend.backend, Backend::Github | Backend::Gitlab))
        .collect()
}

fn resolve_pull_filter_ids(ids: &[String], backend: &BackendConfig) -> HashSet<String> {
    ids.iter()
        .map(|id| {
            if id.contains("--") {
                return id.clone();
            }
            if id.chars().all(|character| character.is_ascii_digit()) {
                let number = id.parse::<u64>().unwrap_or_default();
                return crate::services::issue_ids::format_id(
                    &crate::services::issue_ids::derive_scope_from_backend(backend),
                    number,
                );
            }
            id.clone()
        })
        .collect()
}

fn collect_push_paths(
    paths: &AppPaths,
    backend: &BackendConfig,
    ids: &[String],
) -> Result<Vec<camino::Utf8PathBuf>, RiptskError> {
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
                frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
            };
            if issue.frontmatter.project == backend.name {
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
            frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
        };
        if issue.frontmatter.project != backend.name {
            continue;
        }
        paths_to_push.push(path);
    }
    Ok(paths_to_push)
}

fn deleted_key_matches_backend_scope(key: &str, backends: &[&BackendConfig]) -> bool {
    let Some((provider, repo, _issue_id)) = parse_backend_state_key(key) else {
        return false;
    };
    backends
        .iter()
        .any(|backend| provider_name(backend) == provider && backend.repo.as_deref() == Some(repo))
}

fn parse_backend_state_key(key: &str) -> Option<(&str, &str, u64)> {
    let (provider, rest) = key.split_once(':')?;
    let (repo, issue_id) = rest.rsplit_once(':')?;
    Some((provider, repo, issue_id.parse().ok()?))
}

fn provider_name(backend: &BackendConfig) -> &'static str {
    match backend.backend {
        Backend::Github => "github",
        Backend::Gitlab => "gitlab",
        Backend::Local => "local",
    }
}

pub fn resolve(paths: &AppPaths, args: ResolveArgs) -> Result<(), RiptskError> {
    if args.take_remote && args.take_local {
        return Err(RiptskError::General(
            "cannot specify both --take-remote and --take-local".into(),
        ));
    }
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
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
            return Err(RiptskError::Conflict(format!("no remote backup for {id}")));
        }
        std::fs::copy(&remote_backup, &path)?;
        crate::storage::issue_store::delete_conflict_backups(paths, &id)?;
    } else if args.take_local {
        if !local_backup.exists() {
            return Err(RiptskError::Conflict(format!("no local backup for {id}")));
        }
        std::fs::copy(&local_backup, &path)?;
        crate::storage::issue_store::delete_conflict_backups(paths, &id)?;
        bump_local_updated_at(&path)?;
    } else {
        let content = std::fs::read_to_string(&path)?;
        if crate::storage::frontmatter::has_conflict_markers(&content) {
            return Err(RiptskError::Conflict(format!(
                "issue {id} still has unresolved conflict markers"
            )));
        }
        // Validate the file parses before deleting backups
        crate::storage::frontmatter::load_issue(path.as_std_path()).map_err(|_| {
            RiptskError::Conflict(format!(
                "issue {id} has invalid frontmatter after conflict resolution; backups preserved"
            ))
        })?;
        crate::storage::issue_store::delete_conflict_backups(paths, &id)?;
        bump_local_updated_at(&path)?;
    }
    crate::services::view_builder::ViewBuilder::new(paths, &config)
        .regenerate_all(None)
        .map_err(RiptskError::Other)?;
    Ok(())
}

fn bump_local_updated_at(path: &camino::Utf8PathBuf) -> Result<(), RiptskError> {
    let mut issue =
        crate::storage::frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    issue.frontmatter.local_updated_at = crate::services::issue_service::now_utc();
    crate::storage::frontmatter::save_issue(path.as_std_path(), &issue)
        .map_err(RiptskError::Other)?;
    Ok(())
}

pub fn session(paths: &AppPaths, args: SessionArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    match args.subcommand {
        SessionSubcommand::Start(args) => session_start(paths, args),
        SessionSubcommand::End => session_end(paths),
    }
}

pub fn commit(paths: &AppPaths, args: CommitArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let git = CliGit::new();
    let repo = paths.riptsk_repo.as_std_path().to_path_buf();
    let tracked = [
        paths.riptsk_repo.join("issues"),
        paths.riptsk_repo.join("templates"),
        paths.config_path(),
    ];
    let refs = tracked
        .iter()
        .map(|path| path.as_std_path())
        .collect::<Vec<_>>();
    git.add(repo.as_path(), &refs)?;
    let message = format!(
        "riptsk: manual commit {}",
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
            .map_err(RiptskError::Other)?;
        if !status.success() {
            return Err(RiptskError::General("git commit failed".into()));
        }
        return Ok(());
    }
    git.commit(repo.as_path(), &message)
}

fn session_start(paths: &AppPaths, args: SessionStartArgs) -> Result<(), RiptskError> {
    if session_store::load_session(paths.session_state_path().as_std_path())?.is_some() {
        return Err(RiptskError::Conflict("session already active".into()));
    }
    let SessionStartArgs { scope, id } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
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
            .map_err(RiptskError::Other)?;
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

fn session_end(paths: &AppPaths) -> Result<(), RiptskError> {
    let Some(session) = session_store::load_session(paths.session_state_path().as_std_path())?
    else {
        return Err(RiptskError::NotFound("no active session".into()));
    };
    let git = CliGit::new();
    let repo = current_repo()?;
    if git.has_working_tree_changes(paths.riptsk_repo.as_std_path())? {
        commit(paths, CommitArgs { edit: false })?;
    }
    git.checkout(repo.as_path(), &session.previous_branch)?;
    session_store::clear_session(paths.session_state_path().as_std_path())?;
    Ok(())
}

fn current_repo() -> Result<std::path::PathBuf, RiptskError> {
    std::env::current_dir().map_err(RiptskError::from)
}
