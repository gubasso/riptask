use crate::adapters::ai::AiBackend;
use crate::adapters::git::{CliGit, GitBackend};
use crate::cli::{PrArgs, PrCreateArgs, PrEditArgs, PrShowArgs, PrSubcommand, ScopeArgs};
use crate::commands::ai::{AI_BACKEND_MISSING, optional_backend};
use crate::commands::branch::{
    backend_issue_number, current_repo, cwd_utf8, find_issue_for_branch,
};
use crate::config::{Config, load_config};
use crate::domain::issue::{IssueDocument, IssueState};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{build_provider_for_backend, resolve_git_auth};
use crate::services::id_resolution;
use crate::services::issue_service::now_utc;
use crate::storage::{frontmatter, issue_store};
use std::fs;
use std::io::Write;
use std::path::Path;
const EMPTY_BRANCH_COMMIT_MESSAGE: &str = "chore: initialize branch for PR\n\n\
                                          Empty commit to allow PR creation on a branch with no changes yet.";

pub async fn run(paths: &AppPaths, args: PrArgs) -> Result<(), RiptskError> {
    match args.subcommand {
        Some(PrSubcommand::Create(args)) => create(paths, args).await,
        Some(PrSubcommand::Edit(args)) => edit(paths, args).await,
        Some(PrSubcommand::Show(args)) => show(paths, args).await,
        None => {
            create(
                paths,
                PrCreateArgs {
                    scope: args.scope,
                    id: args.id,
                    no_ai: false,
                },
            )
            .await
        }
    }
}

async fn create(paths: &AppPaths, args: PrCreateArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let repo = current_repo()?;
    let current_branch = CliGit::new().current_branch(repo.as_path())?;
    let (path, mut issue) = resolve_issue(paths, &config, &args.scope, args.id.as_deref())?;
    let branch = issue
        .frontmatter
        .branch
        .clone()
        .ok_or_else(|| RiptskError::General("run `tsk branch` first".into()))?;
    if current_branch != branch {
        return Err(RiptskError::General(
            "current branch does not match issue branch".into(),
        ));
    }

    let backend = resolve_hosted_backend(&config, &issue)?;
    let git = CliGit::with_auth(resolve_git_auth(backend));
    let provider = build_provider_for_backend(backend)?;
    let repo_name = backend.repo.as_deref().unwrap_or_default();
    let default_branch = provider.default_branch(repo_name).await?;

    // Check local metadata first, then remote, for idempotent create
    let local_pr_number = issue.frontmatter.pr_number.or_else(|| {
        issue
            .frontmatter
            .pr_url
            .as_deref()
            .and_then(parse_pr_number_from_url)
    });
    let existing_pr = if let Some(pr_number) = local_pr_number
        && let Ok(existing) = provider.get_pr(repo_name, pr_number).await
        && pr_head_matches(&existing.head, &branch)
        && existing.base == default_branch
    {
        Some(existing)
    } else {
        provider
            .find_pr_by_branch(repo_name, &branch, &default_branch)
            .await?
    };
    if let Some(existing) = existing_pr {
        sync_and_commit_pr(&config, &git, paths, &path, &mut issue, &existing)?;
        println!("{}", existing.url);
        return Ok(());
    }

    git.fetch(repo.as_path())?;
    git.push_with_upstream(repo.as_path(), &branch)?;

    let mut skip_ai = false;
    if git.commits_ahead_of_base(repo.as_path(), &branch, &default_branch)? == 0 {
        if git.has_staged_changes(repo.as_path())? {
            return Err(RiptskError::General(
                "branch has no commits but has staged changes; commit or unstage them first".into(),
            ));
        }
        crate::ui::info("branch has no commits; creating empty commit for PR...");
        git.create_empty_commit(repo.as_path(), EMPTY_BRANCH_COMMIT_MESSAGE)?;
        git.push_with_upstream(repo.as_path(), &branch)?;
        skip_ai = true;
    }

    let issue_number = backend_issue_number(backend, &issue)?;
    let title = default_title(issue_number, &issue.frontmatter.title);
    let mut body = default_body(issue_number);
    if !args.no_ai && !skip_ai {
        if let Some(ai) = optional_backend(&config) {
            let context =
                build_create_ai_context(&issue, &git, repo.as_path(), &default_branch, &branch)?;
            crate::ui::info("generating AI PR description...");
            match ai.generate_pr_description(&context) {
                Ok(description) if !description.trim().is_empty() => {
                    body = format!("{body}\n\n{}", description.trim());
                }
                Ok(_) => {}
                Err(error) => crate::ui::warn(&format!("AI PR description unavailable: {error}")),
            }
        } else if config.ai.enabled {
            crate::ui::warn(&format!("AI unavailable: {AI_BACKEND_MISSING}"));
        }
    }

    let record = provider
        .create_pr(repo_name, &branch, &default_branch, &title, &body)
        .await?;

    issue.frontmatter.pr_url = Some(record.url.clone());
    issue.frontmatter.pr_number = Some(record.number);
    if matches!(
        issue.frontmatter.status,
        IssueState::Backlog | IssueState::Todo
    ) {
        issue.frontmatter.status = IssueState::InProgress;
        issue.frontmatter.local_updated_at = now_utc();
    }
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
    sync_and_commit_pr(&config, &git, paths, &path, &mut issue, &record)?;
    println!("{}", record.url);
    Ok(())
}

async fn show(paths: &AppPaths, args: PrShowArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let (_path, issue) = resolve_issue(paths, &config, &args.scope, args.id.as_deref())?;
    let backend = resolve_hosted_backend(&config, &issue)?;
    let provider = build_provider_for_backend(backend)?;
    let repo_name = backend.repo.as_deref().unwrap_or_default();
    let pr_number = resolve_pr_number(provider.as_ref(), repo_name, &issue).await?;
    let record = provider.get_pr(repo_name, pr_number).await?;

    if args.json {
        let payload = serde_json::to_string_pretty(&record).map_err(|error| {
            RiptskError::Other(anyhow::Error::from(error).context("failed to serialize PR JSON"))
        })?;
        println!("{payload}");
    } else {
        println!("#{} [{}] {}", record.number, record.state, record.title);
        println!("{}", record.url);
        println!("{} -> {}", record.head, record.base);
        if !record.body.trim().is_empty() {
            println!();
            println!("{}", record.body);
        }
    }
    Ok(())
}

async fn edit(paths: &AppPaths, args: PrEditArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    let (path, mut issue) = resolve_issue(paths, &config, &args.scope, args.id.as_deref())?;
    let backend = resolve_hosted_backend(&config, &issue)?;
    let provider = build_provider_for_backend(backend)?;
    let repo_name = backend.repo.as_deref().unwrap_or_default();
    let pr_number = resolve_pr_number(provider.as_ref(), repo_name, &issue).await?;
    let current = provider.get_pr(repo_name, pr_number).await?;

    let record = if args.title.is_some() || args.description.is_some() {
        let title = args.title.as_deref().unwrap_or(&current.title);
        let body = args.description.as_deref().unwrap_or(&current.body);
        provider
            .update_pr(repo_name, pr_number, title, body)
            .await?
    } else if args.no_ai {
        let draft = edit_buffer(&current.title, &current.body)?;
        provider
            .update_pr(repo_name, pr_number, &draft.0, &draft.1)
            .await?
    } else {
        let ai_context = if let Some(branch) = issue.frontmatter.branch.as_deref() {
            match (provider.default_branch(repo_name).await, current_repo()) {
                (Ok(default_branch), Ok(repo)) => {
                    let git = CliGit::new();
                    build_update_ai_context(&current, &git, repo.as_path(), &default_branch, branch)
                        .ok()
                }
                _ => None,
            }
        } else {
            None
        };
        let draft_body = if let Some(context) = ai_context {
            match optional_backend(&config) {
                Some(ai) => {
                    crate::ui::info("generating AI PR description update...");
                    match ai.update_pr_description(&context) {
                        Ok(description) => description,
                        Err(error) => {
                            crate::ui::warn(&format!("AI PR update unavailable: {error}"));
                            String::new()
                        }
                    }
                }
                None => {
                    if config.ai.enabled {
                        crate::ui::warn(&format!("AI unavailable: {AI_BACKEND_MISSING}"));
                    }
                    String::new()
                }
            }
        } else {
            String::new()
        };
        let draft = if draft_body.is_empty() {
            edit_buffer(&current.title, &current.body)?
        } else if args.yes {
            (current.title.clone(), draft_body.trim().to_owned())
        } else {
            edit_buffer(&current.title, draft_body.trim())?
        };
        provider
            .update_pr(repo_name, pr_number, &draft.0, &draft.1)
            .await?
    };

    sync_issue_pr_metadata(&path, &mut issue, &record)?;
    crate::ui::success(&format!("updated PR #{}", record.number));
    Ok(())
}

fn resolve_issue(
    paths: &AppPaths,
    config: &Config,
    _scope: &ScopeArgs,
    id: Option<&str>,
) -> Result<(camino::Utf8PathBuf, IssueDocument), RiptskError> {
    let cwd = cwd_utf8();
    // When no ID given, resolve by current branch (scope unused in branch-based resolution)
    let path = if let Some(id) = id {
        let resolved = id_resolution::resolve_id(paths, config, &cwd, id)?;
        issue_store::find_issue(paths, &resolved)?
    } else {
        let repo = current_repo()?;
        let branch = CliGit::new().current_branch(repo.as_path())?;
        find_issue_for_branch(paths, &branch)?
    };
    let issue = frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
    Ok((path, issue))
}

pub(crate) fn resolve_hosted_backend<'a>(
    config: &'a Config,
    issue: &IssueDocument,
) -> Result<&'a BackendConfig, RiptskError> {
    let backend = config
        .backends
        .iter()
        .find(|backend| backend.name == issue.frontmatter.project)
        .ok_or_else(|| RiptskError::Unregistered(issue.frontmatter.project.clone()))?;
    if !matches!(backend.backend, Backend::Github | Backend::Gitlab) {
        return Err(RiptskError::Config(format!(
            "project {} does not have a hosted backend",
            backend.name
        )));
    }
    Ok(backend)
}

pub(crate) async fn resolve_pr_number(
    provider: &dyn crate::adapters::backend::BackendProvider,
    repo_name: &str,
    issue: &IssueDocument,
) -> Result<u64, RiptskError> {
    if let Some(number) = issue.frontmatter.pr_number {
        return Ok(number);
    }
    if let Some(url) = issue.frontmatter.pr_url.as_deref()
        && let Some(number) = parse_pr_number_from_url(url)
    {
        return Ok(number);
    }
    let branch = issue
        .frontmatter
        .branch
        .as_deref()
        .ok_or_else(|| RiptskError::General("issue has no associated branch".into()))?;
    let base = provider.default_branch(repo_name).await?;
    if let Some(record) = provider.find_pr_by_branch(repo_name, branch, &base).await? {
        return Ok(record.number);
    }
    Err(RiptskError::NotFound(format!(
        "no PR found for issue {}",
        issue.frontmatter.id
    )))
}

fn sync_issue_pr_metadata(
    path: &camino::Utf8Path,
    issue: &mut IssueDocument,
    record: &crate::adapters::backend::BackendPrRecord,
) -> Result<(), RiptskError> {
    let mut changed = false;
    if issue.frontmatter.pr_url.as_deref() != Some(record.url.as_str()) {
        issue.frontmatter.pr_url = Some(record.url.clone());
        changed = true;
    }
    if issue.frontmatter.pr_number != Some(record.number) {
        issue.frontmatter.pr_number = Some(record.number);
        changed = true;
    }
    if changed {
        frontmatter::save_issue(path.as_std_path(), issue).map_err(RiptskError::Other)?;
    }
    Ok(())
}

fn sync_and_commit_pr(
    config: &Config,
    git: &dyn GitBackend,
    paths: &AppPaths,
    path: &camino::Utf8Path,
    issue: &mut IssueDocument,
    record: &crate::adapters::backend::BackendPrRecord,
) -> Result<(), RiptskError> {
    sync_issue_pr_metadata(path, issue, record)?;
    maybe_auto_commit(
        config,
        git,
        paths.riptsk_repo.as_std_path(),
        &format!(
            "riptsk: pr {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )
}

fn default_title(issue_number: u64, issue_title: &str) -> String {
    format!("Solves issue \"#{issue_number} {issue_title}\"")
}

fn default_body(issue_number: u64) -> String {
    format!("Closes #{issue_number}")
}

fn build_create_ai_context(
    issue: &IssueDocument,
    git: &dyn GitBackend,
    repo: &Path,
    base: &str,
    branch: &str,
) -> Result<String, RiptskError> {
    Ok(format!(
        "Issue: {} {}\n\nIssue body:\n{}\n\nCommits:\n{}\n\nDiff:\n{}",
        issue.frontmatter.id,
        issue.frontmatter.title,
        issue.body,
        git.log_between(repo, base, branch)?,
        git.diff_between(repo, base, branch)?,
    ))
}

fn build_update_ai_context(
    current: &crate::adapters::backend::BackendPrRecord,
    git: &dyn GitBackend,
    repo: &Path,
    base: &str,
    branch: &str,
) -> Result<String, RiptskError> {
    Ok(format!(
        "Current PR title: {}\n\nCurrent PR body:\n{}\n\nCommits:\n{}\n\nDiff:\n{}",
        current.title,
        current.body,
        git.log_between(repo, base, branch)?,
        git.diff_between(repo, base, branch)?,
    ))
}

fn edit_buffer(title: &str, body: &str) -> Result<(String, String), RiptskError> {
    let mut file = tempfile::NamedTempFile::new().map_err(RiptskError::Io)?;
    write!(file, "{}", render_editor_buffer(title, body)).map_err(RiptskError::Io)?;
    open_in_editor(file.path())?;
    let content = fs::read_to_string(file.path())?;
    parse_editor_buffer(&content)
}

fn render_editor_buffer(title: &str, body: &str) -> String {
    if body.is_empty() {
        format!("{title}\n")
    } else {
        format!("{title}\n\n{body}")
    }
}

fn parse_editor_buffer(content: &str) -> Result<(String, String), RiptskError> {
    let normalized = content.replace("\r\n", "\n");
    let mut parts = normalized.splitn(2, '\n');
    let title = parts.next().unwrap_or_default().trim().to_owned();
    if title.is_empty() {
        return Err(RiptskError::General("PR title cannot be empty".into()));
    }
    let rest = parts.next().unwrap_or_default();
    let body = if rest.is_empty() {
        String::new()
    } else if let Some(body) = rest.strip_prefix('\n') {
        body.to_owned()
    } else {
        return Err(RiptskError::General(
            "expected a blank line between title and body".into(),
        ));
    };
    Ok((title, body))
}

fn open_in_editor(path: &Path) -> Result<(), RiptskError> {
    let status = crate::services::editor::open_in_editor(path)?;
    if status.success() {
        Ok(())
    } else {
        Err(RiptskError::General("editor exited unsuccessfully".into()))
    }
}

/// Check if a PR head ref matches a branch name. GitHub formats head as
/// `owner:branch`, GitLab uses just `branch`.
fn pr_head_matches(head: &str, branch: &str) -> bool {
    // GitHub: "owner:branch_name" — compare after the colon
    if let Some((_owner, head_branch)) = head.split_once(':') {
        head_branch == branch
    } else {
        head == branch
    }
}

fn parse_pr_number_from_url(url: &str) -> Option<u64> {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|segment| segment.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{
        default_body, default_title, parse_editor_buffer, parse_pr_number_from_url, pr_head_matches,
    };

    #[test]
    fn parses_github_pr_number_from_url() {
        assert_eq!(
            parse_pr_number_from_url("https://github.com/owner/repo/pull/123"),
            Some(123)
        );
    }

    #[test]
    fn parses_gitlab_mr_number_from_url() {
        assert_eq!(
            parse_pr_number_from_url("https://gitlab.com/group/project/-/merge_requests/456"),
            Some(456)
        );
    }

    #[test]
    fn builds_default_title_and_body() {
        assert_eq!(
            default_title(42, "Implement feature"),
            "Solves issue \"#42 Implement feature\""
        );
        assert_eq!(default_body(42), "Closes #42");
    }

    #[test]
    fn parses_editor_buffer_with_body() {
        let parsed =
            parse_editor_buffer("PR Title\n\nLine one\nLine two").expect("parse editor buffer");
        assert_eq!(parsed.0, "PR Title");
        assert_eq!(parsed.1, "Line one\nLine two");
    }

    #[test]
    fn parses_editor_buffer_without_body() {
        let parsed = parse_editor_buffer("PR Title\n").expect("parse editor buffer");
        assert_eq!(parsed.0, "PR Title");
        assert!(parsed.1.is_empty());
    }

    #[test]
    fn pr_head_matches_github_format() {
        assert!(pr_head_matches("owner:42-fix-bug", "42-fix-bug"));
        assert!(!pr_head_matches("owner:42-fix-bug", "fix-bug"));
    }

    #[test]
    fn pr_head_matches_gitlab_format() {
        assert!(pr_head_matches("42-fix-bug", "42-fix-bug"));
        assert!(!pr_head_matches("42-fix-bug", "fix-bug"));
    }
}
