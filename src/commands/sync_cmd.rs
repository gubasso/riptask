use crate::cli::{
    CommitArgs, ResolveArgs, SessionArgs, SessionSubcommand, SyncArgs, SyncSubcommand,
};
use crate::config::{Config, RemoteConfig, RemoteType, load_config};
use crate::domain::session::SessionState;
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::services::remote_mapping::build_provider_for_remote;
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
    for remote in resolve_sync_remotes(args, &config)? {
        let provider = build_provider_for_remote(remote)?;
        let _ = engine.pull(provider.as_ref(), remote, args.force).await?;
    }
    Ok(())
}

async fn push(paths: &AppPaths, args: &SyncArgs) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let engine = SyncEngine::new(paths, &config);
    for remote in resolve_sync_remotes(args, &config)? {
        let provider = build_provider_for_remote(remote)?;
        let _ = engine.push(provider.as_ref(), remote).await?;
    }
    Ok(())
}

fn status(paths: &AppPaths, args: &SyncArgs) -> Result<(), RiptskError> {
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let remote_names = resolve_sync_remotes(args, &config)?
        .into_iter()
        .map(|remote| remote.name.as_str())
        .collect::<HashSet<_>>();
    let summary = SyncEngine::new(paths, &config).status()?;
    for id in summary.conflicts {
        if issue_matches_remote_scope(paths, &id, &remote_names)? {
            println!("CONFLICT {id}");
        }
    }
    for id in summary.pushes {
        if issue_matches_remote_scope(paths, &id, &remote_names)? {
            println!("PUSH {id}");
        }
    }
    Ok(())
}

fn resolve_sync_remotes<'a>(
    args: &SyncArgs,
    config: &'a Config,
) -> Result<Vec<&'a RemoteConfig>, RiptskError> {
    if let Some(name) = args.remote.as_deref() {
        return Ok(hosted_remotes(config)
            .into_iter()
            .filter(|remote| remote.name == name)
            .collect());
    }
    if args.scope.all_projects {
        return Ok(hosted_remotes(config));
    }
    if !args.scope.projects.is_empty() {
        return args
            .scope
            .projects
            .iter()
            .map(|project| {
                config
                    .remotes
                    .iter()
                    .find(|remote| remote.name == *project)
                    .ok_or_else(|| RiptskError::Unregistered(project.clone()))
            })
            .map(|result| {
                result.and_then(|remote| {
                    if matches!(remote.remote_type, RemoteType::Github | RemoteType::Gitlab) {
                        Ok(remote)
                    } else {
                        Err(RiptskError::Config(format!(
                            "sync target must be github or gitlab: {}",
                            remote.name
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
    if let Ok(Some(remote)) = crate::services::project_detection::detect_from_cwd(&cwd, config)
        && let Some(candidate) = config.remotes.iter().find(|candidate| {
            candidate.name == remote.name
                && matches!(
                    candidate.remote_type,
                    RemoteType::Github | RemoteType::Gitlab
                )
        })
    {
        return Ok(vec![candidate]);
    }

    Ok(hosted_remotes(config))
}

fn issue_matches_remote_scope(
    paths: &AppPaths,
    id: &str,
    remote_names: &HashSet<&str>,
) -> Result<bool, RiptskError> {
    let path = crate::storage::issue_store::find_issue(paths, id)?;
    let issue =
        crate::storage::frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    Ok(remote_names.contains(issue.frontmatter.project.as_str()))
}

fn hosted_remotes(config: &Config) -> Vec<&RemoteConfig> {
    config
        .remotes
        .iter()
        .filter(|remote| matches!(remote.remote_type, RemoteType::Github | RemoteType::Gitlab))
        .collect()
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
    let path = crate::storage::issue_store::find_issue(paths, &args.id)?;
    let mut issue =
        crate::storage::frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    let remote_file = issue
        .frontmatter
        .conflict
        .as_ref()
        .map(|conflict| paths.issues_dir().join(&conflict.remote_file))
        .ok_or_else(|| RiptskError::Conflict(format!("no conflict for {}", args.id)))?;

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
        SessionSubcommand::Start { id } => session_start(paths, id),
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

fn session_start(paths: &AppPaths, id: Option<String>) -> Result<(), RiptskError> {
    if session_store::load_session(paths.session_state_path().as_std_path())?.is_some() {
        return Err(RiptskError::Conflict("session already active".into()));
    }
    let git = CliGit;
    let repo = current_repo()?;
    let previous_branch = git.current_branch(repo.as_path())?;
    let session_branch = if let Some(ref id) = id {
        let path = crate::storage::issue_store::find_issue(paths, id)?;
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
    } else {
        let branch = format!(
            "riptsk-session-{}",
            crate::services::issue_service::now_utc().replace(':', "-")
        );
        git.create_branch(repo.as_path(), &branch)?;
        branch
    };
    session_store::save_session(
        paths.session_state_path().as_std_path(),
        &SessionState {
            previous_branch,
            session_branch: session_branch.clone(),
            issue_id: id,
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
