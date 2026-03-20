use crate::adapters::git::{CliGit, GitBackend};
use crate::cli::IdArgs;
use crate::config::{RemoteType, load_config};
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::issue_service::generate_slug;
use crate::services::remote_mapping::build_provider_for_remote;
use crate::storage::{frontmatter, issue_store};

pub fn branch(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let id = args
        .id
        .ok_or_else(|| RiptskError::General("<ID> required".into()))?;
    let path = issue_store::find_issue(paths, &id)?;
    let mut issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    let slug = issue
        .frontmatter
        .id_slug
        .clone()
        .unwrap_or_else(|| generate_slug(&issue.frontmatter.id, &issue.frontmatter.title));
    let repo = current_repo()?;
    let git = CliGit;
    if git.branch_exists(repo.as_path(), &slug)? {
        return Err(RiptskError::General(format!(
            "branch already exists: {slug}"
        )));
    }
    git.create_branch(repo.as_path(), &slug)?;
    git.push_with_upstream(repo.as_path(), &slug)?;
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
    println!("{slug}");
    Ok(())
}

pub async fn pr(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let repo = current_repo()?;
    let git = CliGit;
    let current_branch = git.current_branch(repo.as_path())?;
    if matches!(
        current_branch.as_str(),
        "main" | "master" | "develop" | "dev" | "trunk"
    ) {
        return Err(RiptskError::General(format!(
            "protected branch: {current_branch}"
        )));
    }

    let path = if let Some(id) = args.id {
        issue_store::find_issue(paths, &id)?
    } else {
        find_issue_for_branch(paths, &current_branch)?
    };
    let mut issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    if issue.frontmatter.pr_url.is_some() {
        return Err(RiptskError::General(format!(
            "PR already exists for {}",
            issue.frontmatter.id
        )));
    }
    if let Some(branch) = issue.frontmatter.branch.as_ref() {
        if branch != &current_branch {
            return Err(RiptskError::General(
                "current branch does not match issue branch".into(),
            ));
        }
    } else {
        issue.frontmatter.branch = Some(current_branch.clone());
    }

    let remote = config
        .remotes
        .iter()
        .find(|remote| remote.name == issue.frontmatter.project)
        .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?;
    if !matches!(remote.remote_type, RemoteType::Github | RemoteType::Gitlab) {
        return Err(RiptskError::Config(format!(
            "project {} does not have a hosted remote",
            remote.name
        )));
    }
    let provider = build_provider_for_remote(remote)?;
    let repo_name = remote.repo.as_deref().unwrap_or_default();
    let base = provider.default_branch(repo_name).await?;
    let issue_number = match remote.remote_type {
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
    }
    .ok_or_else(|| {
        RiptskError::Config(format!(
            "issue {} is missing remote metadata",
            issue.frontmatter.id
        ))
    })?;
    let title = format!("Resolve \"{}\"", issue.frontmatter.title);
    let body = format!("Closes #{issue_number}");
    let pr_url = provider
        .create_pr(repo_name, &current_branch, &base, &title, &body)
        .await?;
    issue.frontmatter.pr_url = Some(pr_url.clone());
    if issue.frontmatter.state == crate::domain::issue::IssueState::Todo {
        issue.frontmatter.state = crate::domain::issue::IssueState::InProgress;
        issue.frontmatter.local_updated_at = crate::services::issue_service::now_utc();
    }
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
    maybe_auto_commit(
        &config,
        &git,
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: pr {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )?;
    println!("{pr_url}");
    Ok(())
}

fn current_repo() -> Result<std::path::PathBuf, RiptskError> {
    std::env::current_dir().map_err(RiptskError::from)
}

fn find_issue_for_branch(
    paths: &AppPaths,
    branch: &str,
) -> Result<camino::Utf8PathBuf, RiptskError> {
    for path in issue_store::list_issues(paths)? {
        let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
        if issue.frontmatter.branch.as_deref() == Some(branch)
            || issue.frontmatter.id_slug.as_deref() == Some(branch)
        {
            return Ok(path);
        }
    }
    Err(RiptskError::NotFound(format!(
        "no issue found for branch {branch}"
    )))
}
