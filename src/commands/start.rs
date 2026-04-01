use crate::cli::{NewArgs, PrCreateArgs, ScopeArgs, StartArgs};
use crate::commands::{branch, issues, pr};
use crate::error::RiptskError;
use crate::paths::AppPaths;

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

    // Step 2: Create branch
    let _branch = branch::create_branch_for_issue(paths, &id).await?;

    // Step 3: Create PR
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
