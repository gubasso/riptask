use crate::cli::PushArgs;
use crate::config::{Config, RemoteConfig, RemoteType, load_config};
use crate::domain::remote_state::remote_state_key;
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::remote_mapping::{
    build_provider_for_remote, current_timestamp, issue_to_upsert, remote_state_entry,
    update_issue_from_remote,
};
use crate::storage::{cache, frontmatter, issue_store};

pub fn run(paths: &AppPaths, args: PushArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let remotes = resolve_remotes(&config, &args)?;
    for remote in remotes {
        push_remote(paths, &config, remote, &args)?;
    }
    Ok(())
}

fn push_remote(
    paths: &AppPaths,
    config: &Config,
    remote: &RemoteConfig,
    args: &PushArgs,
) -> Result<(), RiptskError> {
    if !matches!(remote.remote_type, RemoteType::Github | RemoteType::Gitlab) {
        return Err(RiptskError::Config(format!(
            "push target must be github or gitlab: {}",
            remote.name
        )));
    }

    let provider = build_provider_for_remote(remote)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| RiptskError::Other(error.into()))?;
    let mut remote_state = cache::load_remote_state(paths).map_err(RiptskError::Other)?;
    let mut changed_paths = Vec::new();

    for path in collect_push_paths(paths, remote, args)? {
        let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
        let issue_id = match remote.remote_type {
            RemoteType::Github => issue
                .frontmatter
                .github
                .as_ref()
                .and_then(|meta| meta.issue_id),
            RemoteType::Gitlab => issue
                .frontmatter
                .gitlab
                .as_ref()
                .and_then(|meta| meta.issue_id),
            RemoteType::Local => None,
        };
        let Some(issue_id) = issue_id else {
            eprintln!(
                "skipping {}: missing remote issue metadata",
                issue.frontmatter.id
            );
            continue;
        };

        let upsert = issue_to_upsert(&issue);
        let mut record = runtime.block_on(provider.update_issue(
            remote.repo.as_deref().unwrap_or_default(),
            issue_id,
            &upsert,
        ))?;
        runtime.block_on(provider.sync_labels(
            remote.repo.as_deref().unwrap_or_default(),
            issue_id,
            &upsert.labels,
        ))?;
        if issue.frontmatter.state == crate::domain::issue::IssueState::Done {
            runtime.block_on(
                provider.close_issue(remote.repo.as_deref().unwrap_or_default(), issue_id),
            )?;
            record.state = "closed".into();
        } else {
            runtime.block_on(
                provider.reopen_issue(remote.repo.as_deref().unwrap_or_default(), issue_id),
            )?;
            record.state = "open".into();
        }
        record.updated_at = current_timestamp();

        let mut updated = issue;
        update_issue_from_remote(&mut updated, &record, remote);
        frontmatter::save_issue(path.as_std_path(), &updated).map_err(RiptskError::Other)?;
        changed_paths.push(path.clone());
        remote_state.insert(
            remote_state_key(
                match remote.remote_type {
                    RemoteType::Github => "github",
                    RemoteType::Gitlab => "gitlab",
                    RemoteType::Local => "local",
                },
                remote.repo.as_deref().unwrap_or_default(),
                record.issue_id,
            ),
            remote_state_entry(&record),
        );
    }

    cache::save_remote_state(paths, &remote_state).map_err(RiptskError::Other)?;
    let mut file_refs = changed_paths
        .iter()
        .map(|path| path.as_std_path())
        .collect::<Vec<_>>();
    let config_path = paths.config_path();
    file_refs.push(config_path.as_std_path());
    maybe_auto_commit(
        config,
        &crate::adapters::git::CliGit,
        paths.riptsk_repo.as_std_path(),
        &format!("riptsk: push local issues to {}", remote.name),
        &file_refs,
    )?;
    Ok(())
}

fn resolve_remotes<'a>(
    config: &'a Config,
    args: &PushArgs,
) -> Result<Vec<&'a RemoteConfig>, RiptskError> {
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
            .collect();
    }
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    if let Ok(Some(remote)) = crate::services::project_detection::detect_from_cwd(&cwd, config) {
        return config
            .remotes
            .iter()
            .find(|candidate| candidate.name == remote.name)
            .map(|candidate| vec![candidate])
            .ok_or_else(|| RiptskError::Unregistered(remote.name));
    }
    Ok(hosted_remotes(config))
}

fn collect_push_paths(
    paths: &AppPaths,
    remote: &RemoteConfig,
    args: &PushArgs,
) -> Result<Vec<camino::Utf8PathBuf>, RiptskError> {
    if !args.ids.is_empty() {
        let mut paths_to_push = Vec::new();
        for path in args
            .ids
            .iter()
            .map(|id| issue_store::find_issue(paths, id))
            .collect::<Result<Vec<_>, _>>()?
        {
            let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
            if issue.frontmatter.project == remote.name {
                paths_to_push.push(path);
            }
        }
        return Ok(paths_to_push);
    }

    let mut paths_to_push = Vec::new();
    for path in issue_store::list_issues(paths)? {
        let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
        if issue.frontmatter.project != remote.name {
            continue;
        }
        paths_to_push.push(path);
    }
    Ok(paths_to_push)
}

fn hosted_remotes(config: &Config) -> Vec<&RemoteConfig> {
    config
        .remotes
        .iter()
        .filter(|remote| matches!(remote.remote_type, RemoteType::Github | RemoteType::Gitlab))
        .collect()
}
