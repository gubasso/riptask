use crate::adapters::git::{CliGit, GitBackend};
use crate::cli::CloneArgs;
use crate::commands::branch::{backend_issue_number, cwd_utf8};
use crate::config::load_config;
use crate::domain::work_clone::WorkCloneMarker;
use crate::error::RiptskError;
use crate::models::Backend;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{
    build_version_control, resolve_git_auth, resolve_vc_for_backend,
};
use crate::services::id_resolution;
use crate::services::issue_service::generate_branch_slug;
use crate::storage::{frontmatter, issue_store, work_clone};

pub async fn run(paths: &AppPaths, args: CloneArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = cwd_utf8();
    let Some(id) =
        id_resolution::resolve_or_pick_id(paths, &config, &cwd, args.id, args.pick, &args.scope)?
    else {
        return Ok(());
    };

    let path = issue_store::find_issue(paths, &id)?;
    let mut issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
    let backend = config
        .backends
        .iter()
        .find(|b| b.name == issue.frontmatter.project)
        .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?;
    if backend.backend == Backend::Local {
        return Err(RiptskError::Config(
            "cannot create remote branch for local-only project".into(),
        ));
    }

    let issue_number = backend_issue_number(backend, &issue)?;
    let slug = generate_branch_slug(issue_number, &issue.frontmatter.title);

    let (vc_provider, vc_backend, vc_issue_id) = if backend.backend == Backend::Jira {
        let (provider, vc_backend) =
            resolve_vc_for_backend(backend, &config)?.ok_or_else(|| {
                RiptskError::Config(format!(
                    "No version control backend configured for project '{}'. \
                     Add 'vc: <github-or-gitlab-backend>' to the backend config.",
                    backend.name
                ))
            })?;
        (provider, vc_backend.clone(), None)
    } else {
        let provider = build_version_control(backend)?;
        (provider, backend.clone(), Some(issue_number))
    };

    let auth = resolve_git_auth(&vc_backend);
    let git = CliGit::with_auth(auth);
    let repo_name = vc_backend.repo.as_deref().unwrap_or_default();
    let base = vc_provider.default_branch(repo_name).await?;

    match vc_provider
        .create_branch(repo_name, &slug, &base, vc_issue_id)
        .await
    {
        Ok(()) => {}
        Err(ref error) if error.to_string().to_lowercase().contains("already exists") => {
            crate::ui::info("remote branch already exists, continuing with work-clone creation");
        }
        Err(error) => return Err(error),
    }

    issue.frontmatter.id_slug = Some(slug.clone());
    issue.frontmatter.branch = Some(slug.clone());
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
    maybe_auto_commit(
        &config,
        &git,
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: branch {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )?;

    let repo_root = git.repo_root(std::env::current_dir()?.as_path())?;
    let remote_url = git.remote_url(repo_root.as_path(), "origin")?;
    let parent = repo_root
        .parent()
        .ok_or_else(|| RiptskError::General("repository root has no parent directory".into()))?;
    let repo_dir_name = repo_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            RiptskError::General("repository root has no valid directory name".into())
        })?;
    let target_path = parent.join(format!("{repo_dir_name}.{slug}"));
    if target_path.exists() {
        return Err(RiptskError::General(format!(
            "work-clone target already exists: {}",
            target_path.display()
        )));
    }

    git.clone_with_reference(repo_root.as_path(), &remote_url, target_path.as_path())?;
    git.fetch_and_checkout_tracking(target_path.as_path(), &slug)?;
    work_clone::save_marker(
        target_path.as_path(),
        &WorkCloneMarker {
            main_repo_path: repo_root.to_string_lossy().to_string(),
            branch: slug.clone(),
            issue_id: id,
            remote_url,
        },
    )?;

    crate::ui::success(&format!("created work-clone at {}", target_path.display()));
    crate::ui::info(&format!("cd {}", target_path.display()));
    Ok(())
}
