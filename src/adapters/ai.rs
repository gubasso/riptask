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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedIssueContent {
    pub title: String,
    pub body: String,
}

pub trait AiBackend {
    fn generate_issue_content(&self, context: &str) -> Result<GeneratedIssueContent, RiptskError>;
    fn generate_body(&self, context: &str) -> Result<String, RiptskError>;
    fn suggest_project_key(
        &self,
        repo_name: &str,
        backend_type: &str,
        existing_keys: &[String],
    ) -> Result<String, RiptskError>;
    fn generate_pr_description(&self, context: &str) -> Result<String, RiptskError>;
    fn triage(&self, issue_context: &str) -> Result<TriageSuggestion, RiptskError>;
    fn summarize(&self, issues: &str) -> Result<String, RiptskError>;
    fn ask(&self, question: &str, context: &str) -> Result<String, RiptskError>;
    fn update_pr_description(&self, context: &str) -> Result<String, RiptskError>;
    fn generate_commit_message(&self, diff: &str) -> Result<String, RiptskError>;
}

#[derive(Debug, Clone)]
pub struct TemplateAiBackend {
    pub command_template: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    Auth,
    Config,
    Network,
    Execution,
}

impl AiBackend for TemplateAiBackend {
    fn generate_issue_content(&self, context: &str) -> Result<GeneratedIssueContent, RiptskError> {
        let output = run_ai(
            &self.command_template,
            "Generate a concise issue title and body. Return strict JSON with keys \"title\" and \"body\" only.",
            context,
        )?;
        parse_issue_content_output(&output)
    }

    fn generate_body(&self, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.command_template,
            "Generate a concise issue body with a description and checklist.",
            context,
        )
    }

    fn suggest_project_key(
        &self,
        repo_name: &str,
        backend_type: &str,
        existing_keys: &[String],
    ) -> Result<String, RiptskError> {
        run_ai(
            &self.command_template,
            &format!(
                "You suggest a short project key for an issue tracker. Return a single uppercase ASCII token of 2 to 10 characters, matching [A-Z0-9]+, with no prefix, suffix, explanation, whitespace, or punctuation. Avoid any of these already-taken keys: {}.",
                existing_keys.join(", ")
            ),
            &format!("repo: {repo_name}\nbackend: {backend_type}\n"),
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

    fn generate_commit_message(&self, diff: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.command_template,
            concat!(
                "Generate a conventional commit message for the given diff.\n",
                "\n",
                "FORMAT (output ONLY the raw commit message — no fences, no markdown, no commentary):\n",
                "\n",
                "Line 1: subject in the form type(scope): imperative description\n",
                "Line 2: blank\n",
                "Line 3+: body (required if change is non-trivial)\n",
                "\n",
                "SUBJECT RULES:\n",
                "- type: feat|fix|refactor|docs|test|chore|ci|style|perf|build\n",
                "- scope: lowercase module or area, hierarchical with / (e.g. cli/commit, adapters/git)\n",
                "- description: imperative mood, lowercase start, no trailing period\n",
                "- HARD LIMIT: 72 characters total for the subject line\n",
                "\n",
                "BODY RULES:\n",
                "- 1-2 sentence summary of why, then 2-6 bullet points of what changed\n",
                "- HARD LIMIT: every body line must be at most 72 characters\n",
                "- Do NOT mention AI, generated, automated, or similar\n",
                "\n",
                "CRITICAL: Output the raw commit message text only. Do NOT wrap in ``` or any other formatting.",
            ),
            diff,
        )
    }
}

#[derive(serde::Deserialize)]
struct GeneratedIssueContentResponse {
    title: String,
    body: String,
}

fn parse_issue_content_output(output: &str) -> Result<GeneratedIssueContent, RiptskError> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Err(RiptskError::General(
            "AI returned empty output for issue generation".into(),
        ));
    }

    match serde_json::from_str::<GeneratedIssueContentResponse>(trimmed) {
        Ok(parsed) => {
            let title = parsed.title.trim().to_owned();
            let body = parsed.body.trim().to_owned();
            if title.is_empty() {
                return Err(RiptskError::General(
                    "AI issue generation returned an empty title".into(),
                ));
            }
            if body.is_empty() {
                return Err(RiptskError::General(
                    "AI issue generation returned an empty body".into(),
                ));
            }
            Ok(GeneratedIssueContent { title, body })
        }
        Err(json_err) => {
            let stripped = strip_markdown_fences(trimmed);
            if let Ok(retry) = serde_json::from_str::<GeneratedIssueContentResponse>(&stripped) {
                let title = retry.title.trim().to_owned();
                let body = retry.body.trim().to_owned();
                if title.is_empty() || body.is_empty() {
                    return Err(RiptskError::General(
                        "AI issue generation returned empty title or body".into(),
                    ));
                }
                return Ok(GeneratedIssueContent { title, body });
            }
            if trimmed.starts_with('{') || stripped.starts_with('{') {
                return Err(RiptskError::General(format!(
                    "AI returned malformed JSON for issue generation: {json_err}"
                )));
            }
            fallback_issue_content(trimmed)
        }
    }
}

fn strip_markdown_fences(text: &str) -> String {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let inner = rest
            .trim_start_matches(|c: char| c.is_alphanumeric())
            .trim_start();
        if let Some(stripped) = inner.strip_suffix("```") {
            return stripped.trim().to_owned();
        }
    }
    trimmed.to_owned()
}

fn fallback_issue_content(output: &str) -> Result<GeneratedIssueContent, RiptskError> {
    let body = output.trim().to_owned();
    let Some(first_line) = body.lines().find(|line| !line.trim().is_empty()) else {
        return Err(RiptskError::General(
            "AI issue generation fallback could not derive a title from empty output".into(),
        ));
    };
    let title = first_line.trim().trim_start_matches('#').trim().to_owned();
    if title.is_empty() {
        return Err(RiptskError::General(
            "AI issue generation fallback derived an empty title".into(),
        ));
    }
    Ok(GeneratedIssueContent { title, body })
}

fn run_ai(template: &str, system: &str, input: &str) -> Result<String, RiptskError> {
    let mut input_tempfile = tempfile::NamedTempFile::new()
        .map_err(|e| {
            RiptskError::General(format!(
                "failed to create AI input temp file: {e}\n  hint: check filesystem permissions and temporary directory availability"
            ))
        })?;
    input_tempfile
        .write_all(input.as_bytes())
        .map_err(|e| {
            RiptskError::General(format!(
                "failed to write AI input temp file: {e}\n  hint: check filesystem permissions and temporary directory availability"
            ))
        })?;
    let input_file_path = input_tempfile.path().to_string_lossy().to_string();

    let system_escaped = escape(Cow::Borrowed(system));
    let input_escaped = escape(Cow::Borrowed(input));

    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.add_template("cmd", template).map_err(|e| {
        RiptskError::Config(format!(
            "invalid ai.command template: {e}\n  hint: check ai.command syntax in riptsk.yaml"
        ))
    })?;
    let tmpl = env.get_template("cmd").map_err(|e| {
        RiptskError::Config(format!(
            "invalid ai.command template: {e}\n  hint: check ai.command syntax in riptsk.yaml"
        ))
    })?;
    let rendered = tmpl
        .render(minijinja::context! {
            system => system_escaped.as_ref(),
            input => input_escaped.as_ref(),
            input_file => &input_file_path,
        })
        .map_err(|e| {
            RiptskError::Config(format!(
                "failed to render ai.command template: {e}\n  hint: check ai.command syntax in riptsk.yaml"
            ))
        })?;

    let output = Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .output()
        .map_err(|e| {
            RiptskError::General(format!(
                "failed to execute ai.command via sh -c: {e}\n  hint: check that your shell environment and ai.command are valid"
            ))
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".into());
        let preview = render_command_preview(template);
        let combined = if stderr.is_empty() && !stdout.is_empty() {
            &stdout
        } else {
            &stderr
        };
        let (kind, hint) = classify_and_hint(combined, &code);
        let mut detail = format!(
            "ai.command failed (exit status {code})\n  command: {preview}\n  stderr:\n{}",
            format_stderr_lines(&stderr)
        );
        if !stdout.is_empty() {
            detail.push_str(&format!("\n  stdout:\n{}", format_stderr_lines(&stdout)));
        }
        detail.push_str(&format!("\n  hint: {hint}"));
        match kind {
            ErrorKind::Auth => Err(RiptskError::Auth(detail)),
            ErrorKind::Config => Err(RiptskError::Config(detail)),
            ErrorKind::Network => Err(RiptskError::Unreachable(detail)),
            ErrorKind::Execution => Err(RiptskError::General(detail)),
        }
    }
}

fn render_command_preview(template: &str) -> String {
    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    if env.add_template("cmd", template).is_err() {
        return "<unable to render command preview>".into();
    }

    let Ok(tmpl) = env.get_template("cmd") else {
        return "<unable to render command preview>".into();
    };

    let redacted = escape(Cow::Borrowed("<redacted>")).into_owned();
    let rendered = tmpl.render(minijinja::context! {
        system => redacted.as_str(),
        input => redacted.as_str(),
        input_file => "<redacted>",
    });

    match rendered {
        Ok(preview) => truncate_preview(preview.trim()),
        Err(_) => "<unable to render command preview>".into(),
    }
}

fn truncate_preview(preview: &str) -> String {
    if preview.chars().count() <= 200 {
        preview.to_owned()
    } else {
        let truncated: String = preview.chars().take(197).collect();
        format!("{truncated}...")
    }
}

fn format_stderr_lines(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().take(5).collect();
    if lines.is_empty() {
        "    <empty>".into()
    } else {
        lines
            .into_iter()
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn classify_and_hint(stderr: &str, exit_code: &str) -> (ErrorKind, &'static str) {
    let lowered = stderr.to_ascii_lowercase();

    if [
        "unauthorized",
        "authentication failed",
        "invalid token",
        "invalid api key",
        "invalid api token",
        "api key",
        "api_key",
        "invalid key",
        "token expired",
        "credentials",
        "401 ",
        "http 401",
        "403 forbidden",
        "http 403",
    ]
    .iter()
    .any(|pattern| lowered.contains(pattern))
    {
        return (
            ErrorKind::Auth,
            "check the API token or key used by ai.command",
        );
    }

    if lowered.contains("command not found")
        || lowered.contains("no such file or directory")
        || lowered.contains("permission denied")
        || (exit_code == "127" && lowered.contains("not found"))
    {
        return (
            ErrorKind::Config,
            "check the ai.command executable path and any referenced files or directories",
        );
    }

    if [
        "timeout",
        "timed out",
        "connection refused",
        "could not resolve",
        "dns",
        "rate limit",
        "429",
        "503",
    ]
    .iter()
    .any(|pattern| lowered.contains(pattern))
    {
        return (
            ErrorKind::Network,
            "check network connectivity, provider availability, and retry",
        );
    }

    (
        ErrorKind::Execution,
        "run the rendered command directly to inspect the full failure",
    )
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
        assert!(err.contains("ai.command failed (exit status 1)"));
        assert!(err.contains("command:"));
        assert!(err.contains("stderr:"));
    }

    #[test]
    fn undefined_placeholder_fails() {
        let result = run_ai("echo {{unknown}}", "sys", "in");
        assert!(result.is_err());
    }

    #[test]
    fn generate_issue_content_parses_valid_json() {
        let backend = TemplateAiBackend {
            command_template:
                "printf '%s' '{\"title\":\"Fix login\",\"body\":\"## Description\\n\\nDetails\"}'"
                    .into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("issue content");

        assert_eq!(
            result,
            GeneratedIssueContent {
                title: "Fix login".into(),
                body: "## Description\n\nDetails".into(),
            }
        );
    }

    #[test]
    fn generate_issue_content_falls_back_on_malformed_json() {
        let backend = TemplateAiBackend {
            command_template:
                "printf '%s' 'Investigate mobile timeout\n\n## Description\n\nDetails'".into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("fallback issue content");

        assert_eq!(result.title, "Investigate mobile timeout");
        assert!(result.body.contains("## Description"));
    }

    #[test]
    fn generate_issue_content_rejects_empty_output() {
        let backend = TemplateAiBackend {
            command_template: "printf ''".into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("empty output");
        assert!(err.to_string().contains("empty output"));
    }

    #[test]
    fn auth_like_ai_errors_map_to_auth() {
        let result = run_ai(
            "echo '401 unauthorized token missing' >&2; exit 1",
            "sys",
            "in",
        );
        match result {
            Err(RiptskError::Auth(message)) => assert!(message.contains("hint:")),
            other => panic!("expected auth error, got {other:?}"),
        }
    }

    #[test]
    fn nonexistent_ai_command_maps_to_config() {
        let result = run_ai("nonexistent_command_xyz_12345 {{input}}", "sys", "in");
        assert!(matches!(result, Err(RiptskError::Config(_))));
    }

    #[test]
    fn command_preview_redacts_system_and_input() {
        let result = run_ai(
            "false {{system}} {{input}}",
            "super secret system prompt",
            "super secret input body",
        );
        let err = result.unwrap_err().to_string();
        assert!(err.contains("<redacted>"));
        assert!(!err.contains("super secret system prompt"));
        assert!(!err.contains("super secret input body"));
    }

    #[test]
    fn ai_command_errors_include_hint() {
        let result = run_ai("false", "sys", "in");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("hint:"));
    }

    #[test]
    fn nonzero_exit_includes_stdout_when_stderr_empty() {
        let result = run_ai("echo 'error on stdout'; exit 1", "sys", "in");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("stdout:"));
        assert!(err.contains("error on stdout"));
    }

    #[test]
    fn auth_error_on_stdout_maps_to_auth() {
        let result = run_ai("echo '401 unauthorized' ; exit 1", "sys", "in");
        assert!(matches!(result, Err(RiptskError::Auth(_))));
    }

    #[test]
    fn token_limit_errors_are_not_auth() {
        let result = run_ai("echo 'max tokens exceeded' >&2; exit 1", "sys", "in");
        assert!(
            !matches!(result, Err(RiptskError::Auth(_))),
            "token limit error should not be classified as auth"
        );
    }
}
