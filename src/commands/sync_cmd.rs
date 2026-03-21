use crate::cli::{
    CommitArgs, ResolveArgs, SessionArgs, SessionStartArgs, SessionSubcommand, SyncArgs,
    SyncSubcommand,
};
use crate::config::{Config, load_config};
use crate::domain::session::SessionState;
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::backend_mapping::build_provider_for_backend;
use crate::services::id_resolution;
use crate::services::sync_engine::SyncEngine;
use crate::storage::session as session_store;
use crate::{adapters::git::CliGit, adapters::git::GitBackend};
use anyhow::Context;
use std::collections::HashSet;

pub async fn run(paths: &AppPaths, args: SyncArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    match args.subcommand.clone() {
        Some(SyncSubcommand::Pull) => pull(paths, &args).await,
        Some(SyncSubcommand::Push) => push(paths, &args).await,
        Some(SyncSubcommand::Status) => status(paths, &args),
        None => {
            // Bare `tsk sync` = pull then push (matches Bash behavior)
            pull(paths, &args).await?;
            push(paths, &args).await
        }
    }
}

async fn pull(paths: &AppPaths, args: &SyncArgs) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let engine = SyncEngine::new(paths, &config);
    for backend in resolve_sync_backends(args, &config)? {
        let provider = build_provider_for_backend(backend)?;
        let summary = engine.pull(provider.as_ref(), backend, args.force).await?;
        println!(
            "pull: {} created, {} updated, {} deleted, {} conflicts",
            summary.created.len(),
            summary.updated.len(),
            summary.deleted.len(),
            summary.conflicts.len()
        );
    }
    Ok(())
}

async fn push(paths: &AppPaths, args: &SyncArgs) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let engine = SyncEngine::new(paths, &config);
    for backend in resolve_sync_backends(args, &config)? {
        let provider = build_provider_for_backend(backend)?;
        let summary = engine.push(provider.as_ref(), backend).await?;
        println!(
            "push: {} created, {} updated, {} deleted, {} skipped",
            summary.created.len(),
            summary.updated.len(),
            summary.deleted.len(),
            summary.skipped.len()
        );
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
    for id in summary.conflicts {
        if issue_matches_backend_scope(paths, &id, &backend_names)? {
            println!("CONFLICT {id}");
        }
    }
    for id in summary.creates {
        if issue_matches_backend_scope(paths, &id, &backend_names)? {
            println!("CREATE {id}");
        }
    }
    for id in summary.pushes {
        if issue_matches_backend_scope(paths, &id, &backend_names)? {
            println!("PUSH {id}");
        }
    }
    for key in summary.deletes {
        if deleted_key_matches_backend_scope(&key, &backends) {
            println!("DELETE {key}");
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

    Ok(hosted_backends(config))
}

fn issue_matches_backend_scope(
    paths: &AppPaths,
    id: &str,
    backend_names: &HashSet<&str>,
) -> Result<bool, RiptskError> {
    let path = crate::storage::issue_store::find_issue(paths, id)?;
    let issue =
        crate::storage::frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    Ok(backend_names.contains(issue.frontmatter.project.as_str()))
}

fn hosted_backends(config: &Config) -> Vec<&BackendConfig> {
    config
        .backends
        .iter()
        .filter(|backend| matches!(backend.backend, Backend::Github | Backend::Gitlab))
        .collect()
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
    if !args.take_remote && !args.take_local {
        return Err(RiptskError::General(
            "specify --take-remote or --take-local".into(),
        ));
    }
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
    let mut issue =
        crate::storage::frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    let remote_file = issue
        .frontmatter
        .conflict
        .as_ref()
        .map(|conflict| paths.issues_dir().join(&conflict.remote_file))
        .ok_or_else(|| RiptskError::Conflict(format!("no conflict for {id}")))?;

    if args.take_remote {
        let mut remote = crate::storage::frontmatter::load_issue(remote_file.as_std_path())
            .map_err(RiptskError::Other)?;
        remote.frontmatter.conflict = None;
        remote.frontmatter.conflict_role = None;
        remote.frontmatter.conflict_parent = None;
        remote.frontmatter.local_updated_at = crate::services::issue_service::now_utc();
        crate::storage::frontmatter::save_issue(path.as_std_path(), &remote)
            .map_err(RiptskError::Other)?;
    } else {
        issue.frontmatter.conflict = None;
        issue.frontmatter.conflict_role = None;
        issue.frontmatter.conflict_parent = None;
        issue.frontmatter.local_updated_at = crate::services::issue_service::now_utc();
        crate::storage::frontmatter::save_issue(path.as_std_path(), &issue)
            .map_err(RiptskError::Other)?;
    }

    if remote_file.exists() {
        std::fs::remove_file(remote_file)?;
    }
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
    let git = CliGit;
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
    let git = CliGit;
    let repo = current_repo()?;
    let previous_branch = git.current_branch(repo.as_path())?;
    let session_branch = {
        let path = crate::storage::issue_store::find_issue(paths, &id)?;
        let mut issue = crate::storage::frontmatter::load_issue(path.as_std_path())
            .map_err(RiptskError::Other)?;
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
    let git = CliGit;
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
