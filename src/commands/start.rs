use crate::cli::{NewArgs, PrCreateArgs, ScopeArgs, StartArgs};
use crate::commands::{branch, issues, pr};
use crate::config::load_config;
use crate::error::RiptskError;
use crate::models::Backend;
use crate::paths::AppPaths;
use crate::services::backend_mapping::resolve_vc_for_backend;

pub async fn run(paths: &AppPaths, args: StartArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;

    // Convert StartArgs to NewArgs with AI enabled by default
    let new_args = NewArgs {
        title_pos: args.title_pos,
        title: args.title,
        project: args.project,
        board: args.board,
        status: args.status,
        priority: args.priority,
        template: args.template,
        ai: !args.no_ai,
        edit: args.edit,
    };

    // Step 1: Create issue
    let (issue, _issue_path) = issues::create_issue_from_args(paths, new_args).await?;
    let id = issue.frontmatter.id.clone();
    issues::print_issue_created(&issue);

    // Step 2: Check if VC is available for branch/PR
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let backend = config
        .backends
        .iter()
        .find(|b| b.name == issue.frontmatter.project)
        .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?;

    let has_vc = match backend.backend {
        Backend::Github | Backend::Gitlab => true,
        Backend::Jira => backend.vc.is_some(),
        Backend::Local => false,
    };

    if !has_vc {
        if backend.backend == Backend::Jira {
            crate::ui::info(
                "No version control backend configured — skipping branch and PR creation.",
            );
        }
        return Ok(());
    }

    // For Jira with vc, resolve the VC backend for branch/PR operations
    if backend.backend == Backend::Jira {
        let vc_resolution = resolve_vc_for_backend(backend, &config)?;
        if vc_resolution.is_none() {
            crate::ui::info(
                "No version control backend configured — skipping branch and PR creation.",
            );
            return Ok(());
        }
    }

    // Step 3: Create branch
    let _branch = branch::create_branch_for_issue(paths, &id).await?;

    // Step 4: Create PR
    pr::create(
        paths,
        PrCreateArgs {
            scope: ScopeArgs::default(),
            id: Some(id),
            no_ai: args.no_ai,
        },
    )
    .await?;

    Ok(())
}
