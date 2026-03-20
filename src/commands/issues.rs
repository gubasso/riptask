use crate::adapters::git::CliGit;
use crate::adapters::remote::RemoteIssueUpsert;
use crate::cli::{IdArgs, LsArgs, MoveArgs, NewArgs};
use crate::config::{load_config, save_config};
use crate::domain::issue::{GithubIssueMeta, GitlabIssueMeta, IssueDocument, IssueFrontmatter};
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::issue_ids;
use crate::services::issue_service::{IssueDraft, IssueService, generate_slug};
use crate::services::remote_mapping::{build_provider_for_remote, build_state_labels};
use crate::storage::{cache, frontmatter, issue_store};
use std::io::IsTerminal;

pub fn show(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let id = args
        .id
        .ok_or_else(|| RiptskError::General("<ID> required".into()))?;
    let path = issue_store::find_issue(paths, &id)?;
    print!("{}", std::fs::read_to_string(path)?);
    Ok(())
}

pub fn path(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let id = args
        .id
        .ok_or_else(|| RiptskError::General("<ID> required".into()))?;
    println!("{}", issue_store::find_issue(paths, &id)?);
    Ok(())
}

pub fn list(paths: &AppPaths, args: LsArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let scope = crate::scope::resolve_scope(&args.projects, args.all_projects, &cwd, &config)?;
    let service = IssueService::new(paths, &config);
    for issue in service.list_matching(&args, &scope)? {
        let marker = if issue.frontmatter.conflict.is_some() {
            " [CONFLICT]"
        } else {
            ""
        };
        println!(
            "{}\t{}\t{}\t{}\t{}{}",
            issue.frontmatter.id,
            issue
                .frontmatter
                .priority
                .as_ref()
                .map(|value| value.as_str())
                .unwrap_or(""),
            issue.frontmatter.state.as_str(),
            issue.frontmatter.project,
            issue.frontmatter.title,
            marker
        );
    }
    Ok(())
}

pub async fn new(paths: &AppPaths, args: NewArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let mut config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let mut args = args;
    let mut config_changed = false;
    if args.project.is_none() {
        let cwd = camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        );
        if let Some(remote) = crate::services::project_detection::detect_from_cwd(&cwd, &config)? {
            args.project = Some(remote.name);
        } else if let Some(remote) =
            crate::services::project_detection::register_project_auto(&cwd, &mut config)?
        {
            args.project = Some(remote.name);
            config_changed = true;
        }
    }
    if args.title.is_none() && std::io::stdin().is_terminal() {
        let title = dialoguer::Input::<String>::new()
            .with_prompt("Title")
            .interact_text()
            .map_err(|error| RiptskError::General(error.to_string()))?;
        args.title = Some(title);
    }
    let service = IssueService::new(paths, &config);
    let ai = args.ai;
    let mut draft = service.prepare_issue_draft(args)?;
    if ai {
        draft.body = match crate::commands::ai::generate_body(paths, &draft.title, &draft.project) {
            Ok(body) => body,
            Err(_) => {
                eprintln!("warning: AI body generation failed, using template body");
                draft.body.clone()
            }
        };
    }
    let issue = if matches!(
        draft.remote_type.as_ref(),
        Some(crate::config::RemoteType::Github | crate::config::RemoteType::Gitlab)
    ) {
        let remote = config
            .remotes
            .iter()
            .find(|remote| remote.name == draft.project)
            .ok_or_else(|| RiptskError::Unregistered(draft.project.clone()))?;
        create_remote_issue(paths, &service, &draft, remote).await?
    } else {
        create_local_issue(&service, &draft)?
    };
    if config_changed {
        save_config(paths.config_path().as_std_path(), &config).map_err(RiptskError::Other)?;
    }
    let issue_path = paths
        .issues_dir()
        .join(format!("{}.md", issue.frontmatter.id));
    let mut files = vec![issue_path.as_std_path()];
    let config_path = paths.config_path();
    if config_changed {
        files.push(config_path.as_std_path());
    }
    maybe_auto_commit(
        &config,
        &CliGit,
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: new {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &files,
    )?;
    println!("{}", issue.frontmatter.id);
    Ok(())
}

pub fn edit(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let id = args
        .id
        .ok_or_else(|| RiptskError::General("<ID> required".into()))?;
    let path = issue_store::find_issue(paths, &id)?;
    let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    IssueService::new(paths, &config).edit_issue(Some(id.clone()))?;
    maybe_auto_commit(
        &config,
        &CliGit,
        paths.riptsk_repo.as_std_path(),
        &format!("riptsk: edit {} - {}", id, issue.frontmatter.title),
        &[path.as_std_path()],
    )?;
    Ok(())
}

pub fn move_issue(paths: &AppPaths, args: MoveArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let id = args
        .id
        .ok_or_else(|| RiptskError::General("<ID> required".into()))?;
    let state = args
        .state
        .ok_or_else(|| RiptskError::General("<state> required".into()))?;
    let path = issue_store::find_issue(paths, &id)?;
    let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    IssueService::new(paths, &config).move_issue(&id, &state)?;
    maybe_auto_commit(
        &config,
        &CliGit,
        paths.riptsk_repo.as_std_path(),
        &format!("riptsk: move {} - {}", id, issue.frontmatter.title),
        &[path.as_std_path()],
    )?;
    Ok(())
}

pub fn close(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    move_issue(
        paths,
        MoveArgs {
            scope: args.scope,
            id: args.id,
            state: Some("done".into()),
        },
    )
}

pub fn reopen(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    move_issue(
        paths,
        MoveArgs {
            scope: args.scope,
            id: args.id,
            state: Some("todo".into()),
        },
    )
}

pub fn remove(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let id = args
        .id
        .ok_or_else(|| RiptskError::General("<ID> required".into()))?;
    let path = issue_store::find_issue(paths, &id)?;
    let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    IssueService::new(paths, &config).remove_issue(&id)?;
    maybe_auto_commit(
        &config,
        &CliGit,
        paths.riptsk_repo.as_std_path(),
        &format!("riptsk: rm {} - {}", id, issue.frontmatter.title),
        &[path.as_std_path()],
    )?;
    Ok(())
}

async fn create_remote_issue(
    paths: &AppPaths,
    service: &IssueService<'_>,
    draft: &IssueDraft,
    remote: &crate::config::RemoteConfig,
) -> Result<IssueDocument, RiptskError> {
    let provider = build_provider_for_remote(remote)?;
    let repo = remote
        .repo
        .as_deref()
        .ok_or_else(|| RiptskError::Config(format!("remote {} is missing repo", remote.name)))?;
    let upsert = RemoteIssueUpsert {
        title: draft.title.clone(),
        body: draft.body.clone(),
        labels: build_state_labels(&draft.state, &draft.labels),
        assignee: draft.assignee.clone(),
    };
    let record = provider.create_issue(repo, &upsert).await?;
    let scope = issue_ids::derive_scope_from_remote(remote);
    let id = issue_ids::format_id(&scope, record.issue_id);
    let document = IssueDocument {
        frontmatter: IssueFrontmatter {
            id: id.clone(),
            title: draft.title.clone(),
            state: draft.state.clone(),
            board: draft.board.clone(),
            project: draft.project.clone(),
            org: draft.org.clone(),
            priority: Some(draft.priority.clone()),
            labels: draft.labels.clone(),
            assignee: draft.assignee.clone(),
            milestone: None,
            cycle: None,
            order: Some(draft.order),
            gitlab: if remote.remote_type == crate::config::RemoteType::Gitlab {
                Some(GitlabIssueMeta {
                    repo: repo.to_owned(),
                    issue_id: Some(record.issue_id),
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(draft.state.clone()),
                })
            } else {
                None
            },
            github: if remote.remote_type == crate::config::RemoteType::Github {
                Some(GithubIssueMeta {
                    repo: repo.to_owned(),
                    issue_id: Some(record.issue_id),
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(draft.state.clone()),
                })
            } else {
                None
            },
            local_updated_at: record.updated_at.clone(),
            due: None,
            recurring: None,
            remote_deleted: false,
            conflict: None,
            conflict_role: None,
            conflict_parent: None,
            id_slug: Some(generate_slug(&id, &draft.title)),
            branch: None,
            pr_url: None,
        },
        body: draft.body.clone(),
        remote_section: None,
    };
    service.persist_issue(&document)?;
    cache::seed_remote_state_entry(paths, provider_name(remote), repo, &record)
        .map_err(RiptskError::Other)?;
    Ok(document)
}

fn create_local_issue(
    service: &IssueService<'_>,
    draft: &IssueDraft,
) -> Result<IssueDocument, RiptskError> {
    let document = service.build_local_issue_document(draft)?;
    service.persist_issue(&document)?;
    Ok(document)
}

fn provider_name(remote: &crate::config::RemoteConfig) -> &'static str {
    match remote.remote_type {
        crate::config::RemoteType::Github => "github",
        crate::config::RemoteType::Gitlab => "gitlab",
        crate::config::RemoteType::Local => "local",
    }
}
