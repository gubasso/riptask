use crate::cli::{NewArgs, PrCreateArgs, ScopeArgs, StartArgs};
use crate::commands::{branch, issues, pr};
use crate::config::load_config;
use crate::error::RiptskError;
use crate::models::Backend;
use crate::paths::AppPaths;
use crate::services::backend_mapping::resolve_vc_for_backend;
use crate::services::id_resolution;
use crate::storage::issue_store;

pub async fn run(paths: &AppPaths, args: StartArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = id_resolution::cwd_utf8();

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
                title_pos: args.title_pos,
                title: args.title,
                project,
                board: args.board,
                status: args.status,
                priority: args.priority,
                template: args.template,
                ai: config.ai.enabled && !args.no_ai,
                edit: args.edit,
            };
            let (issue, path) = issues::create_issue_from_args(paths, new_args).await?;
            issues::print_issue_created(&issue);
            (issue, path)
        };

    let id = issue.frontmatter.id.clone();

    // Check if VC is available for branch/PR
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

    if backend.backend == Backend::Jira {
        let vc_resolution = resolve_vc_for_backend(backend, &config)?;
        if vc_resolution.is_none() {
            crate::ui::info(
                "No version control backend configured — skipping branch and PR creation.",
            );
            return Ok(());
        }
    }

    // Create branch
    let _branch = branch::create_branch_for_issue(paths, &id).await?;

    // Create PR
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
