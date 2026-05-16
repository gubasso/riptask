use crate::adapters::backend::BranchCreateOutcome;
use crate::adapters::git::{CliGit, GitBackend};
use crate::cli::CloneArgs;
use crate::commands::branch::{backend_issue_number, cwd_utf8};
use crate::config::load_effective_config;
use crate::domain::work_clone::WorkCloneMarker;
use crate::error::RiptaskError;
use crate::models::BackendKind;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{build_version_control, resolve_git_auth};
use crate::services::id_resolution;
use crate::services::issue_service::generate_branch_slug;
use crate::storage::{frontmatter, issue_store, work_clone};

pub async fn run(paths: &AppPaths, args: CloneArgs) -> Result<(), RiptaskError> {
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
    let cwd = cwd_utf8();
    let Some(id) =
        id_resolution::resolve_or_pick_id(paths, &config, &cwd, args.id, args.pick, &args.scope)?
    else {
        return Ok(());
    };

    let path = issue_store::find_issue(paths, &id)?;
    let mut issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
    let repo_project = config
        .projects
        .iter()
        .find(|rp| rp.name == issue.frontmatter.project)
        .ok_or_else(|| RiptaskError::Unregistered(issue.frontmatter.project.clone()))?;
    if repo_project.vc_backend.kind == BackendKind::Local {
        return Err(RiptaskError::Config(
            "cannot create remote branch for local-only project".into(),
        ));
    }

    let issue_number = backend_issue_number(repo_project, &issue)?;
    let slug = generate_branch_slug(issue_number, &issue.frontmatter.title);

    let vc_provider = build_version_control(&repo_project.vc_backend, &repo_project.name)?;
    let vc_issue_id = if repo_project.tasks_backend.kind == BackendKind::Jira {
        None
    } else {
        Some(issue_number)
    };

    let auth = resolve_git_auth(&repo_project.vc_backend, &repo_project.name);
    let git = CliGit::with_auth(auth);
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let base = crate::ui::spin_on_async("Fetching default branch", async {
        vc_provider.default_branch(repo_name).await
    })
    .await?;

    let branch_spinner = crate::ui::spinner("Creating remote branch".to_owned());
    let create_branch_result = vc_provider
        .create_branch(repo_name, &slug, &base, vc_issue_id)
        .await;
    if let Some(ref pb) = branch_spinner {
        pb.finish_and_clear();
    }
    match create_branch_result {
        Ok(BranchCreateOutcome::Created) => {}
        Ok(BranchCreateOutcome::AlreadyExists) => {
            crate::ui::info("remote branch already exists, continuing with work-clone creation");
        }
        Ok(BranchCreateOutcome::EmptyRemote) => {
            return Err(RiptaskError::Config(
                "cannot clone work-tree for empty remote; bootstrap the repository first by running `tsk pr` in the primary clone"
                    .into(),
            ));
        }
        Err(error) => return Err(error),
    }
    tracing::info!(branch = %slug, backend = %repo_project.name, "ensured remote branch exists for clone");

    issue.frontmatter.id_slug = Some(slug.clone());
    issue.frontmatter.branch = Some(slug.clone());
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
    maybe_auto_commit(
        &config,
        &git,
        paths.riptask_repo.as_std_path(),
        &format!(
            "riptask: branch {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )?;

    let repo_root = git.repo_root(std::env::current_dir()?.as_path())?;
    let remote_url = git.remote_url(repo_root.as_path(), "origin")?;
    let parent = repo_root
        .parent()
        .ok_or_else(|| RiptaskError::General("repository root has no parent directory".into()))?;
    let repo_dir_name = repo_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            RiptaskError::General("repository root has no valid directory name".into())
        })?;
    let target_path = parent.join(format!("{repo_dir_name}.{slug}"));
    if target_path.exists() {
        return Err(RiptaskError::General(format!(
            "work-clone target already exists: {}",
            target_path.display()
        )));
    }

    crate::ui::spin_on("Cloning repository", || {
        git.clone_with_reference(repo_root.as_path(), &remote_url, target_path.as_path())
    })?;
    crate::ui::spin_on("Checking out branch", || {
        git.fetch_and_checkout_tracking(target_path.as_path(), &slug)
    })?;
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
    tracing::info!(branch = %slug, target = %target_path.display(), "created work clone");
    Ok(())
}
