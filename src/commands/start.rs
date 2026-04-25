use crate::adapters::git::{CliGit, GitBackend};
use crate::cli::{NewArgs, PrCreateArgs, ScopeArgs, StartArgs};
use crate::commands::{branch, issues, pr};
use crate::config::{Config, load_effective_config};
use crate::error::RiptaskError;
use crate::models::BackendKind;
use crate::paths::AppPaths;
use crate::services::backend_mapping::build_version_control;
use crate::services::id_resolution;
use crate::services::issue_ids;
use crate::services::project_detection;
use crate::storage::issue_store;
use camino::Utf8Path;

pub async fn run(paths: &AppPaths, mut args: StartArgs) -> Result<(), RiptaskError> {
    paths.require_initialized(&crate::paths::current_cwd())?;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let cwd = id_resolution::cwd_utf8();

    if should_auto_create_from_changes(&config, &cwd, &args).await {
        let new_args = NewArgs {
            title: None,
            description: None,
            ai: false,
            project: args.scope.projects.first().cloned(),
            board: args.board.clone(),
            status: args.status.clone(),
            priority: args.priority.clone(),
            template: args.template.clone(),
            edit: args.edit,
        };
        let (issue, _path) = issues::create_issue_from_args(paths, new_args).await?;
        issues::print_issue_created(&issue);
        args.title_pos = Some(numeric_from_id(&issue.frontmatter.id));
        args.title = None;
        args.pick = false;
    }

    // Determine mode: existing issue or new issue
    let is_numeric = args
        .title_pos
        .as_ref()
        .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()));

    let (issue, _issue_path) =
        if is_numeric || args.pick || (args.title_pos.is_none() && args.title.is_none()) {
            // Existing issue mode: resolve by ID, pick, or auto-detect from branch
            let id_input = if is_numeric {
                args.title_pos.clone()
            } else {
                None
            };
            let Some(id) = id_resolution::resolve_or_pick_id(
                paths,
                &config,
                &cwd,
                id_input,
                args.pick,
                &args.scope,
            )?
            else {
                return Ok(()); // User cancelled picker
            };
            let path = issue_store::find_issue(paths, &id)?;
            let issue = issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
            (issue, path)
        } else {
            // New issue mode: create from title
            let project = args.scope.projects.first().cloned();
            let new_args = NewArgs {
                title: args.title_pos.or(args.title),
                description: None,
                ai: false,
                project,
                board: args.board,
                status: args.status,
                priority: args.priority,
                template: args.template,
                edit: args.edit,
            };
            let (issue, path) = issues::create_issue_from_args(paths, new_args).await?;
            issues::print_issue_created(&issue);
            (issue, path)
        };

    let id = issue.frontmatter.id.clone();

    // Check if VC is available for branch/PR
    let repo_project = config
        .projects
        .iter()
        .find(|repo_project| repo_project.name == issue.frontmatter.project)
        .ok_or_else(|| RiptaskError::Unregistered(issue.frontmatter.project.clone()))?;

    let has_vc = repo_project.vc_backend.kind != BackendKind::Local;

    if !has_vc {
        crate::ui::info("No version control backend configured — skipping branch and PR creation.");
        return Ok(());
    }

    // Create branch
    let _branch = branch::create_branch_for_issue(paths, &id).await?;

    // Create PR
    pr::create(
        paths,
        PrCreateArgs {
            scope: ScopeArgs::default(),
            id: Some(id),
            no_ai: false,
        },
    )
    .await?;

    Ok(())
}

async fn should_auto_create_from_changes(
    config: &Config,
    cwd: &Utf8Path,
    args: &StartArgs,
) -> bool {
    if !(args.title_pos.is_none() && args.title.is_none() && !args.pick) {
        return false;
    }
    let Ok(Some(repo_project)) = project_detection::detect_from_cwd(cwd, config) else {
        return false;
    };
    if repo_project.vc_backend.kind == BackendKind::Local {
        return false;
    }
    let Ok(repo) = id_resolution::current_repo() else {
        return false;
    };
    let Ok(provider) = build_version_control(&repo_project.vc_backend, &repo_project.name) else {
        return false;
    };
    let git = CliGit::new();
    let Ok(current) = git.current_branch(repo.as_path()) else {
        return false;
    };
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let Ok(default) = crate::ui::spin_on_async("Checking default branch", async {
        provider.default_branch(repo_name).await
    })
    .await
    else {
        return false;
    };
    if current != default {
        return false;
    }
    git.has_uncommitted_changes(repo.as_path()).unwrap_or(false)
}

fn numeric_from_id(id: &str) -> String {
    issue_ids::parse_id(id)
        .map(|(_, number)| number.to_string())
        .unwrap_or_else(|| id.to_owned())
}
