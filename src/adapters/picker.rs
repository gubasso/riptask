use crate::domain::issue::{IssueDocument, Priority};
use crate::error::RiptskError;
use std::process::{Command, Stdio};

pub trait Picker {
    fn pick_issue(
        &self,
        issues: &[IssueDocument],
        prompt: &str,
    ) -> Result<Option<String>, RiptskError>;
    fn pick_many(&self, issues: &[IssueDocument], prompt: &str)
    -> Result<Vec<String>, RiptskError>;
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
    ) -> Result<Option<String>, RiptskError> {
        let input = issues
            .iter()
            .map(format_issue_picker_line)
            .collect::<Vec<_>>()
            .join("\n");
        run_fzf(&input, prompt, self.fzf_opts.as_deref())
    }

    fn pick_many(
        &self,
        issues: &[IssueDocument],
        prompt: &str,
    ) -> Result<Vec<String>, RiptskError> {
        let input = issues
            .iter()
            .map(format_issue_picker_line)
            .collect::<Vec<_>>()
            .join("\n");
        let Some(selection) = run_fzf_multi(&input, prompt, self.fzf_opts.as_deref())? else {
            return Ok(Vec::new());
        };
        Ok(selection
            .lines()
            .map(|line| line.split('\t').next().unwrap_or_default().to_owned())
            .filter(|line| !line.is_empty())
            .collect())
    }

    fn pick_enum(&self, choices: &[String], prompt: &str) -> Result<Option<String>, RiptskError> {
        let input = choices.join("\n");
        run_fzf(&input, prompt, self.fzf_opts.as_deref())
    }
}

pub fn format_issue_picker_line(issue: &IssueDocument) -> String {
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
        issue.frontmatter.project,
        issue.frontmatter.title
    )
}

fn run_fzf(
    input: &str,
    prompt: &str,
    extra_opts: Option<&str>,
) -> Result<Option<String>, RiptskError> {
    run_fzf_with_args(input, prompt, extra_opts, &[])
}

fn run_fzf_multi(
    input: &str,
    prompt: &str,
    extra_opts: Option<&str>,
) -> Result<Option<String>, RiptskError> {
    run_fzf_with_args(input, prompt, extra_opts, &["--multi"])
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
