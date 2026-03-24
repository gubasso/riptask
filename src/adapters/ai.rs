use crate::error::RiptskError;
use shell_escape::escape;
use std::borrow::Cow;
use std::io::Write;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageSuggestion {
    pub state: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
}

pub trait AiBackend {
    fn generate_body(&self, context: &str) -> Result<String, RiptskError>;
    fn generate_pr_description(&self, context: &str) -> Result<String, RiptskError>;
    fn triage(&self, issue_context: &str) -> Result<TriageSuggestion, RiptskError>;
    fn summarize(&self, issues: &str) -> Result<String, RiptskError>;
    fn ask(&self, question: &str, context: &str) -> Result<String, RiptskError>;
    fn update_pr_description(&self, context: &str) -> Result<String, RiptskError>;
}

#[derive(Debug, Clone)]
pub struct TemplateAiBackend {
    pub command_template: String,
}

impl AiBackend for TemplateAiBackend {
    fn generate_body(&self, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.command_template,
            "Generate a concise issue body with a description and checklist.",
            context,
        )
    }

    fn generate_pr_description(&self, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.command_template,
            "Generate a concise PR description summarizing the changes. Include a summary section and key changes. Do not include the title.",
            context,
        )
    }

    fn triage(&self, issue_context: &str) -> Result<TriageSuggestion, RiptskError> {
        let output = run_ai(
            &self.command_template,
            "Return JSON with keys status, priority, labels.",
            issue_context,
        )?;
        let parsed: serde_json::Value = serde_json::from_str(&output).map_err(|error| {
            RiptskError::General(format!("invalid AI triage response: {error}"))
        })?;
        Ok(TriageSuggestion {
            state: parsed
                .get("status")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            priority: parsed
                .get("priority")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned),
            labels: parsed
                .get("labels")
                .and_then(|value| value.as_array())
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|label| label.as_str().map(ToOwned::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    fn summarize(&self, issues: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.command_template,
            "Summarize issue status concisely.",
            issues,
        )
    }

    fn ask(&self, question: &str, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.command_template,
            "Answer the user's question from the provided issue corpus.",
            &format!("Question: {question}\n\nIssues:\n{context}"),
        )
    }

    fn update_pr_description(&self, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.command_template,
            "Update this PR description based on the current changes. Keep it concise and focused on implementation details.",
            context,
        )
    }
}

fn run_ai(template: &str, system: &str, input: &str) -> Result<String, RiptskError> {
    let mut input_tempfile = tempfile::NamedTempFile::new()
        .map_err(|e| RiptskError::General(format!("failed to create temp file: {e}")))?;
    input_tempfile
        .write_all(input.as_bytes())
        .map_err(|e| RiptskError::General(format!("failed to write temp file: {e}")))?;
    let input_file_path = input_tempfile.path().to_string_lossy().to_string();

    let system_escaped = escape(Cow::Borrowed(system));
    let input_escaped = escape(Cow::Borrowed(input));

    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.add_template("cmd", template)
        .map_err(|e| RiptskError::General(format!("invalid ai.command template: {e}")))?;
    let tmpl = env
        .get_template("cmd")
        .map_err(|e| RiptskError::General(format!("invalid ai.command template: {e}")))?;
    let rendered = tmpl
        .render(minijinja::context! {
            system => system_escaped.as_ref(),
            input => input_escaped.as_ref(),
            input_file => &input_file_path,
        })
        .map_err(|e| RiptskError::General(format!("failed to render ai.command: {e}")))?;

    let output = Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .output()
        .map_err(|e| RiptskError::General(format!("failed to execute ai command: {e}")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".into());
        Err(RiptskError::General(format!(
            "ai command exited with status {code}: {stderr}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_template_with_short_input() {
        let result = run_ai("echo {{input}}", "test system", "hello world");
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn renders_template_with_input_file() {
        let result = run_ai("cat {{input_file}}", "test system", "file content here");
        assert_eq!(result.unwrap(), "file content here");
    }

    #[test]
    fn nonzero_exit_includes_stderr() {
        let result = run_ai("echo 'fail' >&2; exit 1", "sys", "in");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("fail"));
        assert!(err.contains("status 1"));
    }

    #[test]
    fn undefined_placeholder_fails() {
        let result = run_ai("echo {{unknown}}", "sys", "in");
        assert!(result.is_err());
    }
}
