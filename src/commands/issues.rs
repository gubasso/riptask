use crate::adapters::backend::BackendIssueUpsert;
use crate::adapters::git::CliGit;
use crate::cli::{IdArgs, LsArgs, MoveArgs, NewArgs};
use crate::config::load_config;
use crate::domain::issue::{GithubIssueMeta, GitlabIssueMeta, IssueDocument, IssueFrontmatter};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{build_provider_for_backend, build_state_labels};
use crate::services::id_resolution;
use crate::services::issue_ids;
use crate::services::issue_service::{IssueDraft, IssueService, generate_slug};
use crate::storage::{cache, frontmatter, issue_store};
use std::io::IsTerminal;

pub fn show(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let IdArgs { scope, id } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::require_id(paths, &config, &cwd, id, &scope)? else {
        return Ok(());
    };
    let path = issue_store::find_issue(paths, &id)?;
    print!("{}", std::fs::read_to_string(path)?);
    Ok(())
}

pub fn path(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let IdArgs { scope, id } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::require_id(paths, &config, &cwd, id, &scope)? else {
        return Ok(());
    };
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
    let scope =
        crate::scope::resolve_scope(&args.scope.projects, args.scope.all_projects, &cwd, &config)?;
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
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let mut args = args;
    if args.project.is_none() {
        let cwd = camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        );
        if let Some(backend) = crate::services::project_detection::detect_from_cwd(&cwd, &config)? {
            args.project = Some(backend.name);
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
    let issue = if draft
        .backend
        .as_ref()
        .is_some_and(|backend| matches!(backend, Backend::Github | Backend::Gitlab))
    {
        let backend = config
            .backends
            .iter()
            .find(|backend| backend.name == draft.project)
            .ok_or_else(|| RiptskError::Unregistered(draft.project.clone()))?;
        create_backend_issue(paths, &service, &draft, backend).await?
    } else {
        create_local_issue(&service, &draft)?
    };
    let issue_path = paths
        .issues_dir()
        .join(format!("{}.md", issue.frontmatter.id));
    maybe_auto_commit(
        &config,
        &CliGit,
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: new {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[issue_path.as_std_path()],
    )?;
    println!("{}", issue.frontmatter.id);
    Ok(())
}

pub fn edit(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let IdArgs { scope, id } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::require_id(paths, &config, &cwd, id, &scope)? else {
        return Ok(());
    };
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
    let MoveArgs { scope, id, state } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::require_id(paths, &config, &cwd, id, &scope)? else {
        return Ok(());
    };
    let state = state.ok_or_else(|| RiptskError::General("<state> required".into()))?;
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
    let IdArgs { scope, id } = args;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::require_id(paths, &config, &cwd, id, &scope)? else {
        return Ok(());
    };
    let path = issue_store::find_issue(paths, &id)?;
    let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    if let Some((provider, repo, issue_id)) = backend_delete_target(&issue) {
        cache::mark_deleted(
            paths,
            &crate::domain::backend_state::backend_state_key(provider, &repo, issue_id),
        )
        .map_err(RiptskError::Other)?;
    }
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

async fn create_backend_issue(
    paths: &AppPaths,
    service: &IssueService<'_>,
    draft: &IssueDraft,
    backend: &BackendConfig,
) -> Result<IssueDocument, RiptskError> {
    let provider = build_provider_for_backend(backend)?;
    let repo = backend
        .repo
        .as_deref()
        .ok_or_else(|| RiptskError::Config(format!("backend {} is missing repo", backend.name)))?;
    let upsert = BackendIssueUpsert {
        title: draft.title.clone(),
        body: draft.body.clone(),
        state: None,
        state_reason: None,
        labels: build_state_labels(&draft.state, &draft.labels),
        assignees: draft.assignee.clone().into_iter().collect(),
        milestone_id: None,
        due_date: None,
        weight: None,
        confidential: None,
        discussion_locked: None,
    };
    let record = provider.create_issue(repo, &upsert).await?;
    let scope = issue_ids::derive_scope_from_backend(backend);
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
            assignees: draft.assignee.clone().into_iter().collect(),
            milestone: record.milestone.clone(),
            state_reason: record.state_reason.clone(),
            cycle: None,
            order: Some(draft.order),
            gitlab: if backend.backend == Backend::Gitlab {
                Some(GitlabIssueMeta {
                    repo: repo.to_owned(),
                    issue_id: Some(record.issue_id),
                    milestone_id: record.milestone_id,
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(draft.state.clone()),
                })
            } else {
                None
            },
            github: if backend.backend == Backend::Github {
                Some(GithubIssueMeta {
                    repo: repo.to_owned(),
                    issue_id: Some(record.issue_id),
                    node_id: record.node_id.clone(),
                    milestone_id: record.milestone_id,
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(draft.state.clone()),
                })
            } else {
                None
            },
            local_updated_at: record.updated_at.clone(),
            due: record.due_date.clone(),
            weight: record.weight,
            confidential: record.confidential,
            discussion_locked: record.discussion_locked,
            issue_type: record.issue_type.clone(),
            locked: record.locked,
            lock_reason: record.lock_reason.clone(),
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
    cache::seed_backend_state_entry(paths, provider_name(backend), repo, &record)
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

fn provider_name(backend: &BackendConfig) -> &'static str {
    match backend.backend {
        Backend::Github => "github",
        Backend::Gitlab => "gitlab",
        Backend::Local => "local",
    }
}

fn backend_delete_target(issue: &IssueDocument) -> Option<(&'static str, String, u64)> {
    if let Some(meta) = issue.frontmatter.github.as_ref() {
        return meta
            .issue_id
            .map(|issue_id| ("github", meta.repo.clone(), issue_id));
    }
    if let Some(meta) = issue.frontmatter.gitlab.as_ref() {
        return meta
            .issue_id
            .map(|issue_id| ("gitlab", meta.repo.clone(), issue_id));
    }
    None
}
