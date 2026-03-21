use crate::domain::issue::{IssueDocument, Priority};
use crate::error::RiptskError;
use crate::services::issue_ids;
use console::style;
use std::process::{Command, Stdio};

const TITLE_MAX_WIDTH: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueDisplayMode {
    PerProject,
    AllProjects,
}

pub trait Picker {
    fn pick_issue(
        &self,
        issues: &[IssueDocument],
        prompt: &str,
        issues_dir: Option<&str>,
        mode: &IssueDisplayMode,
    ) -> Result<Option<String>, RiptskError>;
    fn pick_many(
        &self,
        issues: &[IssueDocument],
        prompt: &str,
        issues_dir: Option<&str>,
        mode: &IssueDisplayMode,
    ) -> Result<Vec<String>, RiptskError>;
    fn pick_enum(&self, choices: &[String], prompt: &str) -> Result<Option<String>, RiptskError>;
}

#[derive(Debug, Clone, Default)]
pub struct FzfPicker {
    pub fzf_opts: Option<String>,
}

impl Picker for FzfPicker {
    fn pick_issue(
        &self,
        issues: &[IssueDocument],
        prompt: &str,
        issues_dir: Option<&str>,
        mode: &IssueDisplayMode,
    ) -> Result<Option<String>, RiptskError> {
        let input = issues
            .iter()
            .map(|issue| format_issue_line(issue, mode, true))
            .collect::<Vec<_>>()
            .join("\n");
        let selection = run_fzf_issue(&input, prompt, self.fzf_opts.as_deref(), issues_dir, &[])?;
        Ok(selection.map(|line| extract_hidden_id(&line)))
    }

    fn pick_many(
        &self,
        issues: &[IssueDocument],
        prompt: &str,
        issues_dir: Option<&str>,
        mode: &IssueDisplayMode,
    ) -> Result<Vec<String>, RiptskError> {
        let input = issues
            .iter()
            .map(|issue| format_issue_line(issue, mode, true))
            .collect::<Vec<_>>()
            .join("\n");
        let Some(selection) = run_fzf_issue(
            &input,
            prompt,
            self.fzf_opts.as_deref(),
            issues_dir,
            &["--multi"],
        )?
        else {
            return Ok(Vec::new());
        };
        Ok(selection
            .lines()
            .map(extract_hidden_id)
            .filter(|id| !id.is_empty())
            .collect())
    }

    fn pick_enum(&self, choices: &[String], prompt: &str) -> Result<Option<String>, RiptskError> {
        let input = choices.join("\n");
        run_fzf_with_args(&input, prompt, self.fzf_opts.as_deref(), &[])
    }
}

/// Format an issue line for display. When `with_hidden_id` is true, appends a
/// tab-separated full ID (hidden in fzf via `--with-nth=1`).
pub fn format_issue_line(
    issue: &IssueDocument,
    mode: &IssueDisplayMode,
    with_hidden_id: bool,
) -> String {
    let numeric_id = issue_ids::parse_id(&issue.frontmatter.id)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| issue.frontmatter.id.clone());

    let title = truncate_title(&issue.frontmatter.title, TITLE_MAX_WIDTH);

    let priority = issue
        .frontmatter
        .priority
        .as_ref()
        .map(Priority::as_str)
        .unwrap_or("-");
    let board = sanitize_control(&issue.frontmatter.board);
    let tag = format!("[{}/{}]", priority, board);

    let id_plain = format!("#{:<7}", numeric_id);
    let id_str = format!("{}", style(&id_plain).cyan());
    let title_str = format!("{}", style(&title).bold());
    let tag_str = format!("{}", style(&tag).dim());

    let line = match mode {
        IssueDisplayMode::PerProject => {
            format!("{} {}  {}", id_str, title_str, tag_str)
        }
        IssueDisplayMode::AllProjects => {
            let project_label = format!("[{}]", sanitize_control(&issue.frontmatter.project));
            let proj_str = format!("{}", style(&project_label).magenta());
            format!("{} {} {}  {}", proj_str, id_str, title_str, tag_str)
        }
    };

    if with_hidden_id {
        format!("{}\t{}", line, issue.frontmatter.id)
    } else {
        line
    }
}

/// Plain tab-separated format for non-terminal (piped) output.
pub fn format_issue_plain(issue: &IssueDocument) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}",
        issue.frontmatter.id,
        issue
            .frontmatter
            .priority
            .as_ref()
            .map(Priority::as_str)
            .unwrap_or(""),
        issue.frontmatter.state.as_str(),
        sanitize_control(&issue.frontmatter.project),
        sanitize_control(&issue.frontmatter.title)
    )
}

fn sanitize_control(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn truncate_title(title: &str, max: usize) -> String {
    let clean: String = title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() <= max {
        clean
    } else {
        let truncated: String = clean.chars().take(max - 1).collect();
        format!("{truncated}\u{2026}")
    }
}

/// Extract the hidden full ID from a tab-separated fzf selection line.
fn extract_hidden_id(line: &str) -> String {
    line.rsplit('\t')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Run fzf with issue-specific UX: full-screen, preview, keybindings.
fn run_fzf_issue(
    input: &str,
    prompt: &str,
    extra_opts: Option<&str>,
    issues_dir: Option<&str>,
    extra_args: &[&str],
) -> Result<Option<String>, RiptskError> {
    let mut args: Vec<String> = vec![
        "--height=100%".into(),
        "--layout=reverse".into(),
        "--ansi".into(),
        "--delimiter=\t".into(),
        "--with-nth=1".into(),
        "--bind=alt-j:preview-down,alt-k:preview-up".into(),
        "--header=Alt-J/K: scroll preview".into(),
    ];

    if let Some(dir) = issues_dir {
        args.push(format!(
            "--preview=cat '{}/'{{2}}'.md' 2>/dev/null || echo 'No preview available'",
            dir.replace('\'', "'\\''")
        ));
        args.push("--preview-window=right:60%:wrap".into());
    }

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut all_args = arg_refs.clone();
    all_args.extend_from_slice(extra_args);

    run_fzf_with_args(input, prompt, extra_opts, &all_args)
}

fn run_fzf_with_args(
    input: &str,
    prompt: &str,
    extra_opts: Option<&str>,
    args: &[&str],
) -> Result<Option<String>, RiptskError> {
    let mut command = Command::new("fzf");
    if let Some(extra_opts) = extra_opts {
        for option in extra_opts.split_whitespace() {
            command.arg(option);
        }
    }
    command.arg("--prompt").arg(prompt);
    command.args(args);
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = command.spawn().map_err(|_| {
        RiptskError::General("<ID> required (install fzf for interactive selection)".into())
    })?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin.write_all(input.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Ok(None);
    }
    let selection = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok((!selection.is_empty()).then_some(selection))
}
