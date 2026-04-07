use crate::adapters::ai::{AiBackend, GeneratedIssueContent, TemplateAiBackend};
use crate::cli::{AskArgs, SummarizeArgs};
use crate::config::load_config;
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::storage::{frontmatter, issue_store};

pub(crate) const AI_BACKEND_MISSING: &str = "ai.command is not configured";

pub fn summarize(paths: &AppPaths, args: SummarizeArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    if !config.ai.enabled || !config.ai.features.summarize {
        println!("AI features disabled");
        return Ok(());
    }
    let backend = match backend(&config) {
        Ok(backend) => backend,
        Err(error) => {
            crate::ui::error(&format!("AI unavailable: {error}"));
            return Ok(());
        }
    };
    let mut context = String::new();
    for path in issue_store::list_issues(paths)? {
        let issue = match frontmatter::try_load_issue(path.as_std_path()) {
            frontmatter::IssueLoadResult::Ok(issue) => issue,
            frontmatter::IssueLoadResult::Conflict { .. } => continue,
            frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
        };
        if let Some(project) = args.project.as_deref()
            && issue.frontmatter.project != project
        {
            continue;
        }
        if let Some(board) = args.board.as_deref()
            && issue.frontmatter.board != board
        {
            continue;
        }
        if let Some(cycle) = args.cycle.as_deref()
            && issue.frontmatter.cycle.as_deref() != Some(cycle)
        {
            continue;
        }
        context.push_str(&format!(
            "{} {}\n",
            issue.frontmatter.id, issue.frontmatter.title
        ));
    }
    println!("{}", backend.summarize(&context)?);
    Ok(())
}

pub fn ask(paths: &AppPaths, args: AskArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    if !config.ai.enabled || !config.ai.features.ask {
        println!("AI features disabled");
        return Ok(());
    }
    let backend = match backend(&config) {
        Ok(backend) => backend,
        Err(error) => {
            crate::ui::error(&format!("AI unavailable: {error}"));
            return Ok(());
        }
    };
    let mut context = String::new();
    for path in issue_store::list_issues(paths)? {
        let issue = match frontmatter::try_load_issue(path.as_std_path()) {
            frontmatter::IssueLoadResult::Ok(issue) => issue,
            frontmatter::IssueLoadResult::Conflict { .. } => continue,
            frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
        };
        context.push_str(&format!(
            "{} {}\n{}\n\n",
            issue.frontmatter.id, issue.frontmatter.title, issue.body
        ));
    }
    println!("{}", backend.ask(&args.question, &context)?);
    Ok(())
}

pub fn generate_body(paths: &AppPaths, title: &str, project: &str) -> Result<String, RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    if !config.ai.features.new_body_gen {
        return Err(RiptskError::General("AI features disabled".into()));
    }
    let backend = backend(&config)?;
    backend.generate_body(&format!("Title: {title}\n\nContext:\n{project}"))
}

pub fn generate_issue_content(
    paths: &AppPaths,
    context: &str,
) -> Result<GeneratedIssueContent, RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    if !config.ai.features.new_body_gen {
        return Err(RiptskError::General("AI features disabled".into()));
    }
    let backend = backend(&config)?;
    backend.generate_issue_content(context)
}

fn backend(config: &crate::config::Config) -> Result<TemplateAiBackend, RiptskError> {
    optional_backend(config).ok_or_else(|| RiptskError::Config(AI_BACKEND_MISSING.into()))
}

pub(crate) fn optional_backend(config: &crate::config::Config) -> Option<TemplateAiBackend> {
    config.ai.command.as_ref().map(|cmd| TemplateAiBackend {
        command_template: cmd.clone(),
    })
}
