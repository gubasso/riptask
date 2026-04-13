use crate::adapters::backend::BackendIssueUpsert;
use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::picker::{
    IssueDisplayMode, format_issue_plain, sanitize_control, truncate_title,
};
use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::{IdArgs, LsArgs, NewArgs, StatusArgs};
use crate::config::{load_config, parse_state};
use crate::domain::issue::{
    GithubIssueMeta, GitlabIssueMeta, IssueDocument, IssueFrontmatter, IssueState, JiraIssueMeta,
    Priority,
};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::build_state_labels;
use crate::services::id_resolution;
use crate::services::issue_ids;
use crate::services::issue_service::{IssueDraft, IssueService, generate_slug};
use crate::storage::{cache, frontmatter, issue_store};
use comfy_table::{
    Attribute, Cell, CellAlignment, Color, ContentArrangement, Table, presets::NOTHING,
};
use console::style;
use std::io::IsTerminal;

pub fn show(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let IdArgs { scope, id, pick } = args;
    let config = load_config(paths.config_path().as_std_path())?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::resolve_or_pick_id(paths, &config, &cwd, id, pick, &scope)?
    else {
        return Ok(());
    };
    let path = issue_store::find_issue(paths, &id)?;
    let content = std::fs::read_to_string(&path)?;
    if frontmatter::has_conflict_markers(&content) {
        crate::ui::warn(&format!(
            "issue {id} has unresolved sync conflicts\n\n  Resolve in editor:\n    tsk edit {id}\n\n  Then mark resolved:\n    tsk sync resolve {id}"
        ));
    }
    print!("{content}");
    Ok(())
}

pub fn path(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let IdArgs { scope, id, pick } = args;
    let config = load_config(paths.config_path().as_std_path())?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::resolve_or_pick_id(paths, &config, &cwd, id, pick, &scope)?
    else {
        return Ok(());
    };
    println!("{}", issue_store::find_issue(paths, &id)?);
    Ok(())
}

pub fn list(paths: &AppPaths, args: LsArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let scope =
        crate::scope::resolve_scope(&args.scope.projects, args.scope.all_projects, &cwd, &config)?;
    let mode = if args.scope.all_projects {
        IssueDisplayMode::AllProjects
    } else {
        IssueDisplayMode::PerProject
    };
    let is_tty = std::io::stdout().is_terminal();
    if args.conflicts {
        let conflicts = list_conflicted_ids(paths)?;
        if is_tty {
            if !conflicts.is_empty() {
                println!("{}", render_conflict_table(&conflicts, &mode));
            }
        } else {
            for id in conflicts {
                println!("!\t{id}\t\tCONFLICT\t\tUnresolved conflict");
            }
        }
        return Ok(());
    }
    let service = IssueService::new(paths, &config);
    let result = service.list_matching(&args, &scope)?;
    let issues = result.documents;

    if is_tty {
        if !issues.is_empty() {
            println!("{}", render_issue_table(&issues, &mode));
        }
        if result.conflict_count > 0 {
            println!(
                "\n{} issues have unresolved conflicts (tsk ls --conflicts)",
                result.conflict_count
            );
        }
    } else {
        for issue in &issues {
            println!("{}", format_issue_plain(issue));
        }
    }
    Ok(())
}

fn render_issue_table(issues: &[IssueDocument], mode: &IssueDisplayMode) -> String {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);

    let mut headers: Vec<Cell> = Vec::new();
    if *mode == IssueDisplayMode::AllProjects {
        headers.push(
            Cell::new("Project")
                .add_attribute(Attribute::Bold)
                .add_attribute(Attribute::Dim),
        );
    }
    headers.extend([
        Cell::new("ID")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
        Cell::new("Title")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
        Cell::new("Status")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
        Cell::new("Priority")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
        Cell::new("Board")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
    ]);
    table.set_header(headers);

    for issue in issues {
        let fm = &issue.frontmatter;
        let numeric_id = issue_ids::parse_id(&fm.id)
            .map(|(_, n)| format!("#{}", n))
            .unwrap_or_else(|| fm.id.clone());

        let title = truncate_title(&fm.title, 50);

        let priority_str = fm.priority.as_ref().map(Priority::as_str).unwrap_or("-");

        let mut row: Vec<Cell> = Vec::new();
        if *mode == IssueDisplayMode::AllProjects {
            row.push(Cell::new(sanitize_control(&fm.project)).fg(Color::Magenta));
        }
        row.extend([
            Cell::new(&numeric_id)
                .fg(Color::Cyan)
                .set_alignment(CellAlignment::Right),
            Cell::new(&title),
            Cell::new(fm.status.as_str()).fg(state_color(&fm.status)),
            Cell::new(priority_str).fg(priority_color(fm.priority.as_ref())),
            Cell::new(sanitize_control(&fm.board)).fg(Color::Grey),
        ]);
        table.add_row(row);
    }

    table.to_string()
}

fn render_conflict_table(conflicts: &[String], mode: &IssueDisplayMode) -> String {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);

    let mut headers: Vec<Cell> = Vec::new();
    if *mode == IssueDisplayMode::AllProjects {
        headers.push(
            Cell::new("Project")
                .add_attribute(Attribute::Bold)
                .add_attribute(Attribute::Dim),
        );
    }
    headers.extend([
        Cell::new("ID")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
        Cell::new("Title")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
        Cell::new("Status")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
        Cell::new("Priority")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
        Cell::new("Board")
            .add_attribute(Attribute::Bold)
            .add_attribute(Attribute::Dim),
    ]);
    table.set_header(headers);

    for id in conflicts {
        let mut row: Vec<Cell> = Vec::new();
        if *mode == IssueDisplayMode::AllProjects {
            row.push(Cell::new("-").fg(Color::Magenta));
        }
        row.extend([
            Cell::new(format!("! #{id}")).fg(Color::Red),
            Cell::new("Unresolved conflict"),
            Cell::new("CONFLICT").fg(Color::Red),
            Cell::new("-").fg(Color::Grey),
            Cell::new("-").fg(Color::Grey),
        ]);
        table.add_row(row);
    }

    table.to_string()
}

fn state_color(state: &IssueState) -> Color {
    match state {
        IssueState::Backlog => Color::Grey,
        IssueState::Todo => Color::Reset,
        IssueState::InProgress => Color::Yellow,
        IssueState::Review => Color::Cyan,
        IssueState::Done => Color::Green,
    }
}

fn priority_color(priority: Option<&Priority>) -> Color {
    match priority {
        Some(Priority::Urgent) => Color::Red,
        Some(Priority::High) => Color::Yellow,
        Some(Priority::Medium) => Color::Reset,
        Some(Priority::Low) => Color::Grey,
        None => Color::Grey,
    }
}

pub(crate) async fn create_issue_from_args(
    paths: &AppPaths,
    args: NewArgs,
) -> Result<(IssueDocument, camino::Utf8PathBuf), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path())?;
    let mut args = args;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    if args.project.is_none()
        && let Some(backend) = crate::services::project_detection::detect_from_cwd(&cwd, &config)?
    {
        args.project = Some(backend.name);
    }
    let edit = args.edit;
    let prompts = DialoguerPrompts;
    let git = CliGit::new();
    let mut generated_body = None;
    let description = args.description.clone();
    let mut should_generate_body = false;
    let prompt_for_ai_context = || {
        prompts
            .input("Describe the task for AI", None)
            .map_err(|error| {
                RiptskError::General(format!(
                    "AI issue generation failed and no --title provided\n{error}"
                ))
            })
    };

    if args.title.is_none() {
        let ai_context = match resolve_ai_context(&git, &config, args.project.as_deref(), &cwd) {
            Ok(Some(diff)) => diff,
            Ok(None) => prompt_for_ai_context()?,
            Err(error) => {
                crate::ui::warn(&format!(
                    "AI context detection failed, falling back to prompt: {error}"
                ));
                prompt_for_ai_context()?
            }
        };
        match crate::ui::spin_on("Generating issue content", || {
            crate::commands::ai::generate_issue_content(paths, &ai_context)
        }) {
            Ok(generated) => {
                args.title = Some(generated.title);
                if description.is_none() {
                    generated_body = Some(generated.body);
                }
            }
            Err(error) => {
                return Err(RiptskError::General(format!(
                    "AI issue generation failed and no --title provided\n{error}"
                )));
            }
        }
    } else if description.is_none() {
        should_generate_body = true;
    }
    let service = IssueService::new(paths, &config);
    let mut draft = service.prepare_issue_draft(args)?;
    if let Some(description) = description.as_ref() {
        draft.body = description.clone();
    } else if should_generate_body {
        match crate::ui::spin_on("Generating issue description", || {
            crate::commands::ai::generate_body(paths, &draft.title, &draft.project)
        }) {
            Ok(body) => generated_body = Some(body),
            Err(error) => {
                crate::ui::warn(&format!(
                    "AI body generation failed, using template body\n{error}"
                ));
            }
        }
    }
    if description.is_none()
        && let Some(body) = generated_body
    {
        draft.body = body;
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
        &CliGit::new(),
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: new {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[issue_path.as_std_path()],
    )?;
    if edit {
        service.edit_issue(Some(issue.frontmatter.id.clone()))?;
        maybe_auto_commit(
            &config,
            &CliGit::new(),
            paths.riptsk_repo.as_std_path(),
            &format!(
                "riptsk: edit {} - {}",
                issue.frontmatter.id, issue.frontmatter.title
            ),
            &[issue_path.as_std_path()],
        )?;
    }
    Ok((issue, issue_path))
}

pub async fn new(paths: &AppPaths, args: NewArgs) -> Result<(), RiptskError> {
    let (issue, _path) = create_issue_from_args(paths, args).await?;
    print_issue_created(&issue);
    Ok(())
}

fn resolve_ai_context(
    git: &dyn GitBackend,
    config: &crate::config::Config,
    project: Option<&str>,
    cwd: &camino::Utf8Path,
) -> Result<Option<String>, RiptskError> {
    let repo_path = resolve_project_repo_path(config, project, cwd);
    match git.has_uncommitted_changes(repo_path.as_std_path()) {
        Ok(true) => {
            let diff = git.working_tree_diff(repo_path.as_std_path())?;
            if diff.is_empty() {
                Ok(None)
            } else {
                Ok(Some(diff))
            }
        }
        Ok(false) => Ok(None),
        Err(_) => Ok(None),
    }
}

fn resolve_project_repo_path(
    config: &crate::config::Config,
    project: Option<&str>,
    cwd: &camino::Utf8Path,
) -> camino::Utf8PathBuf {
    if let Some(project) = project
        && let Some(path) = config
            .backends
            .iter()
            .find(|backend| backend.name == project)
            .and_then(|backend| backend.path.as_ref())
    {
        return camino::Utf8PathBuf::from(path);
    }
    cwd.to_path_buf()
}

pub(crate) fn print_issue_created(issue: &IssueDocument) {
    if !crate::ui::is_tty() {
        println!("{}", issue.frontmatter.id);
        return;
    }

    let fm = &issue.frontmatter;
    let issue_label = issue_ids::parse_id(&fm.id)
        .map(|(_, n)| format!("#{n}"))
        .unwrap_or_else(|| fm.id.clone());
    println!(
        "{} Created issue {}",
        style("✓").green(),
        style(issue_label).cyan()
    );
    println!("  Title:    {}", fm.title);
    println!("  Project:  {}", fm.project);
    println!("  Board:    {}", fm.board);
    println!(
        "  Status:   {}",
        style_with_color(display_label(fm.status.as_str()), state_color(&fm.status))
    );
    println!(
        "  Priority: {}",
        style_with_color(
            fm.priority
                .as_ref()
                .map(|priority| display_label(priority.as_str()))
                .unwrap_or_else(|| "-".into()),
            priority_color(fm.priority.as_ref())
        )
    );
}

fn display_label(value: &str) -> String {
    value
        .split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    let mut rendered = first.to_uppercase().to_string();
                    rendered.push_str(chars.as_str());
                    rendered
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn style_with_color(text: String, color: Color) -> console::StyledObject<String> {
    match color {
        Color::Black => style(text).black(),
        Color::Red => style(text).red(),
        Color::Green => style(text).green(),
        Color::Yellow => style(text).yellow(),
        Color::Blue => style(text).blue(),
        Color::Magenta => style(text).magenta(),
        Color::Cyan => style(text).cyan(),
        Color::White => style(text).white(),
        Color::Grey => style(text).dim(),
        _ => style(text),
    }
}

pub fn edit(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let IdArgs { scope, id, pick } = args;
    let config = load_config(paths.config_path().as_std_path())?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::resolve_or_pick_id(paths, &config, &cwd, id, pick, &scope)?
    else {
        return Ok(());
    };
    let path = issue_store::find_issue(paths, &id)?;
    let content = std::fs::read_to_string(&path)?;
    let has_conflict = frontmatter::has_conflict_markers(&content);
    if has_conflict {
        crate::ui::warn(&format!(
            "issue {id} has unresolved sync conflicts\n\n  Resolve the markers, then run:\n    tsk sync resolve {id}"
        ));
    }
    IssueService::new(paths, &config).edit_issue(Some(id.clone()))?;
    if !has_conflict {
        let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
        maybe_auto_commit(
            &config,
            &CliGit::new(),
            paths.riptsk_repo.as_std_path(),
            &format!("riptsk: edit {} - {}", id, issue.frontmatter.title),
            &[path.as_std_path()],
        )?;
    }
    Ok(())
}

pub fn set_status(paths: &AppPaths, args: StatusArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let StatusArgs {
        scope,
        id,
        status,
        pick,
    } = args;
    let config = load_config(paths.config_path().as_std_path())?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let (id, status) = if status.is_none() && id.as_ref().is_some_and(|v| parse_state(v).is_ok()) {
        (None, id)
    } else {
        (id, status)
    };
    let Some(id) = id_resolution::resolve_or_pick_id(paths, &config, &cwd, id, pick, &scope)?
    else {
        return Ok(());
    };
    let status = status.ok_or_else(|| RiptskError::General("<status> required".into()))?;
    let path = issue_store::find_issue(paths, &id)?;
    let issue = load_issue_or_conflict_error(path.as_std_path(), &id)?;
    IssueService::new(paths, &config).move_issue(&id, &status)?;
    maybe_auto_commit(
        &config,
        &CliGit::new(),
        paths.riptsk_repo.as_std_path(),
        &format!("riptsk: status {} - {}", id, issue.frontmatter.title),
        &[path.as_std_path()],
    )?;
    Ok(())
}

pub fn close(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    set_status(
        paths,
        StatusArgs {
            scope: args.scope,
            pick: args.pick,
            id: args.id,
            status: Some("done".into()),
        },
    )
}

pub fn reopen(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    set_status(
        paths,
        StatusArgs {
            scope: args.scope,
            pick: args.pick,
            id: args.id,
            status: Some("todo".into()),
        },
    )
}

pub fn remove(paths: &AppPaths, args: IdArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let IdArgs { scope, id, pick } = args;
    let config = load_config(paths.config_path().as_std_path())?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let Some(id) = id_resolution::resolve_or_pick_id(paths, &config, &cwd, id, pick, &scope)?
    else {
        return Ok(());
    };
    let path = issue_store::find_issue(paths, &id)?;
    let issue = load_issue_or_conflict_error(path.as_std_path(), &id)?;
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
        &CliGit::new(),
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
    let provider = crate::services::backend_mapping::build_issue_tracker(backend)?;
    let repo = backend
        .repo
        .as_deref()
        .ok_or_else(|| RiptskError::Config(format!("backend {} is missing repo", backend.name)))?;
    let upsert = BackendIssueUpsert {
        title: draft.title.clone(),
        body: draft.body.clone(),
        state: None,
        state_reason: None,
        labels: build_state_labels(&draft.status, &draft.labels),
        assignees: draft.assignee.clone().into_iter().collect(),
        milestone_id: None,
        due_date: None,
        weight: None,
        confidential: None,
        discussion_locked: None,
        assignee_account_id: None,
        assignee_name: None,
    };
    let record = crate::ui::spin_on_async("Creating issue on remote", async {
        provider.create_issue(repo, &upsert).await
    })
    .await?;
    let scope = issue_ids::effective_key(backend);
    let id = issue_ids::format_id(&scope, record.issue_id);
    let document = IssueDocument {
        frontmatter: IssueFrontmatter {
            id: id.clone(),
            title: draft.title.clone(),
            status: draft.status.clone(),
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
                    last_pushed_state: Some(draft.status.clone()),
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
                    last_pushed_state: Some(draft.status.clone()),
                })
            } else {
                None
            },
            jira: if backend.backend == Backend::Jira {
                let issue_key = record.url.rsplit('/').next().map(|s| s.to_owned());
                Some(JiraIssueMeta {
                    project_key: crate::adapters::jira::JiraProvider::project_key(repo).to_owned(),
                    issue_key,
                    issue_id: Some(record.issue_id),
                    url: Some(record.url.clone()),
                    updated_at: record.updated_at.clone(),
                    last_pushed_state: Some(draft.status.clone()),
                    issue_type: record.issue_type.clone(),
                    assignee_account_id: record.assignee_account_id.clone(),
                    assignee_name: record.assignee_name.clone(),
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
            id_slug: Some(generate_slug(&id, &draft.title)),
            branch: None,
            pr_url: None,
            pr_number: None,
        },
        body: draft.body.clone(),
        remote_section: None,
    };
    service.persist_issue(&document)?;
    cache::seed_backend_state_entry(paths, provider_name(backend), repo, &record)
        .map_err(RiptskError::Other)?;
    tracing::info!(
        issue_id = %document.frontmatter.id,
        project = %document.frontmatter.project,
        backend = %backend.name,
        "created issue"
    );
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
        Backend::Jira => "jira",
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
    if let Some(meta) = issue.frontmatter.jira.as_ref() {
        return meta
            .issue_id
            .map(|issue_id| ("jira", meta.project_key.clone(), issue_id));
    }
    None
}

pub fn load_issue_or_conflict_error(
    path: &std::path::Path,
    id: &str,
) -> Result<IssueDocument, RiptskError> {
    match frontmatter::try_load_issue(path) {
        frontmatter::IssueLoadResult::Ok(document) => Ok(*document),
        frontmatter::IssueLoadResult::Conflict { .. } => Err(RiptskError::Conflict(format!(
            "issue {id} has unresolved sync conflicts\n\n  \
             Edit the file to resolve:\n    \
             tsk edit {id}\n\n  \
             Then mark resolved:\n    \
             tsk sync resolve {id}\n\n  \
             Or take one side:\n    \
             tsk sync resolve {id} --take-local\n    \
             tsk sync resolve {id} --take-remote"
        ))),
        frontmatter::IssueLoadResult::Err(error) => Err(RiptskError::Other(error)),
    }
}

fn list_conflicted_ids(paths: &AppPaths) -> Result<Vec<String>, RiptskError> {
    let mut conflicts = Vec::new();
    for path in issue_store::list_issues(paths)? {
        let content = std::fs::read_to_string(&path)?;
        if frontmatter::has_conflict_markers(&content) {
            conflicts.push(path.file_stem().unwrap_or_default().to_string());
        }
    }
    conflicts.sort();
    Ok(conflicts)
}
