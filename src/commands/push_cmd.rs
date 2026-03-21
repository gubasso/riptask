use crate::cli::PushArgs;
use crate::config::{Config, load_config};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::build_provider_for_backend;
use crate::services::id_resolution;
use crate::services::sync_engine::SyncEngine;
use crate::storage::{frontmatter, issue_store};

pub fn run(paths: &AppPaths, args: PushArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let mut args = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    args.ids = args
        .ids
        .iter()
        .map(|id| id_resolution::resolve_id(paths, &config, &cwd, id))
        .collect::<Result<Vec<_>, _>>()?;
    let backends = resolve_backends(&config, &args)?;
    for backend in backends {
        push_backend(paths, &config, backend, &args)?;
    }
    Ok(())
}

fn push_backend(
    paths: &AppPaths,
    config: &Config,
    backend: &BackendConfig,
    args: &PushArgs,
) -> Result<(), RiptskError> {
    if !matches!(backend.backend, Backend::Github | Backend::Gitlab) {
        return Err(RiptskError::Config(format!(
            "push target must be github or gitlab: {}",
            backend.name
        )));
    }

    let provider = build_provider_for_backend(backend)?;
    let issue_paths = collect_push_paths(paths, backend, args)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| RiptskError::Other(error.into()))?;
    let engine = SyncEngine::new(paths, config);
    if args.ids.is_empty() {
        runtime.block_on(engine.push(provider.as_ref(), backend))?;
    } else {
        runtime.block_on(engine.push_issues(provider.as_ref(), backend, &issue_paths))?;
    }

    let mut file_refs = issue_paths
        .iter()
        .map(|path| path.as_std_path())
        .collect::<Vec<_>>();
    let config_path = paths.config_path();
    file_refs.push(config_path.as_std_path());
    maybe_auto_commit(
        config,
        &crate::adapters::git::CliGit,
        paths.riptsk_repo.as_std_path(),
        &format!("riptsk: push local issues to {}", backend.name),
        &file_refs,
    )?;
    Ok(())
}

fn resolve_backends<'a>(
    config: &'a Config,
    args: &PushArgs,
) -> Result<Vec<&'a BackendConfig>, RiptskError> {
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
            .collect();
    }
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    if let Ok(Some(backend)) = crate::services::project_detection::detect_from_cwd(&cwd, config) {
        return config
            .backends
            .iter()
            .find(|candidate| candidate.name == backend.name)
            .map(|candidate| vec![candidate])
            .ok_or_else(|| RiptskError::Unregistered(backend.name));
    }
    Ok(hosted_backends(config))
}

fn collect_push_paths(
    paths: &AppPaths,
    backend: &BackendConfig,
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
            if issue.frontmatter.project == backend.name {
                paths_to_push.push(path);
            }
        }
        return Ok(paths_to_push);
    }

    let mut paths_to_push = Vec::new();
    for path in issue_store::list_issues(paths)? {
        let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
        if issue.frontmatter.project != backend.name {
            continue;
        }
        paths_to_push.push(path);
    }
    Ok(paths_to_push)
}

fn hosted_backends(config: &Config) -> Vec<&BackendConfig> {
    config
        .backends
        .iter()
        .filter(|backend| matches!(backend.backend, Backend::Github | Backend::Gitlab))
        .collect()
}
