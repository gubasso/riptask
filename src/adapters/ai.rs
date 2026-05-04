use crate::adapters::ai_prompts;
use crate::error::RiptaskError;
use regex::Regex;
use shell_escape::escape;
use std::borrow::Cow;
use std::io::Write;
use std::process::Command;
use std::sync::OnceLock;

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
    fn generate_issue_content(&self, context: &str) -> Result<GeneratedIssueContent, RiptaskError>;
    fn generate_body(&self, context: &str) -> Result<String, RiptaskError>;
    fn suggest_project_key(
        &self,
        repo_name: &str,
        backend_type: &str,
        existing_keys: &[String],
    ) -> Result<String, RiptaskError>;
    fn generate_pr_description(&self, context: &str) -> Result<String, RiptaskError>;
    fn triage(&self, issue_context: &str) -> Result<TriageSuggestion, RiptaskError>;
    fn summarize(&self, issues: &str) -> Result<String, RiptaskError>;
    fn ask(&self, question: &str, context: &str) -> Result<String, RiptaskError>;
    fn update_pr_description(&self, context: &str) -> Result<String, RiptaskError>;
    fn generate_commit_message(&self, diff: &str) -> Result<String, RiptaskError>;
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

const TITLE_MAX_LEN: usize = 120;

impl AiBackend for TemplateAiBackend {
    fn generate_issue_content(&self, context: &str) -> Result<GeneratedIssueContent, RiptaskError> {
        let system = ai_prompts::generate_issue_content_system();
        let output = run_ai(&self.command_template, &system, context)?;
        parse_issue_content_output(&output)
    }

    fn generate_body(&self, context: &str) -> Result<String, RiptaskError> {
        let system = ai_prompts::generate_body_system();
        let raw = run_ai(&self.command_template, &system, context)?;
        sanitize_issue_body(&raw)
    }

    fn suggest_project_key(
        &self,
        repo_name: &str,
        backend_type: &str,
        existing_keys: &[String],
    ) -> Result<String, RiptaskError> {
        let mut system = ai_prompts::suggest_project_key_system();
        system.push_str("\n\nAlready-taken keys to avoid: ");
        system.push_str(&existing_keys.join(", "));
        run_ai(
            &self.command_template,
            &system,
            &format!("repo: {repo_name}\nbackend: {backend_type}\n"),
        )
    }

    fn generate_pr_description(&self, context: &str) -> Result<String, RiptaskError> {
        let system = ai_prompts::generate_pr_description_system();
        run_ai(&self.command_template, &system, context)
    }

    fn triage(&self, issue_context: &str) -> Result<TriageSuggestion, RiptaskError> {
        let system = ai_prompts::triage_system();
        let output = run_ai(&self.command_template, &system, issue_context)?;
        let parsed: serde_json::Value = serde_json::from_str(&output).map_err(|error| {
            RiptaskError::General(format!("invalid AI triage response: {error}"))
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

    fn summarize(&self, issues: &str) -> Result<String, RiptaskError> {
        let system = ai_prompts::summarize_system();
        run_ai(&self.command_template, &system, issues)
    }

    fn ask(&self, question: &str, context: &str) -> Result<String, RiptaskError> {
        let system = ai_prompts::ask_system();
        run_ai(
            &self.command_template,
            &system,
            &format!("Question: {question}\n\nIssues:\n{context}"),
        )
    }

    fn update_pr_description(&self, context: &str) -> Result<String, RiptaskError> {
        let system = ai_prompts::update_pr_description_system();
        run_ai(&self.command_template, &system, context)
    }

    fn generate_commit_message(&self, diff: &str) -> Result<String, RiptaskError> {
        let system = ai_prompts::generate_commit_message_system();
        let raw = run_ai(&self.command_template, &system, diff)?;
        validate_commit_message_output(&raw)
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GeneratedIssueContentResponse {
    title: String,
    body: String,
}

fn parse_issue_content_output(output: &str) -> Result<GeneratedIssueContent, RiptaskError> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Err(RiptaskError::General(
            "AI returned empty output for issue generation".into(),
        ));
    }
    if trimmed.starts_with("```") {
        return Err(RiptaskError::General(
            "AI returned fenced output for issue generation; expected a raw JSON object".into(),
        ));
    }
    let parsed = serde_json::from_str::<GeneratedIssueContentResponse>(trimmed).map_err(|err| {
        RiptaskError::General(format!(
            "AI returned malformed JSON for issue generation: {err}"
        ))
    })?;

    let title = parsed.title.trim().to_owned();
    if title.is_empty() {
        return Err(RiptaskError::General(
            "AI issue generation returned an empty title".into(),
        ));
    }
    if title.contains('\n') || title.contains('\r') {
        return Err(RiptaskError::General(
            "AI issue generation returned a multiline title".into(),
        ));
    }
    if title.chars().count() > TITLE_MAX_LEN {
        return Err(RiptaskError::General(format!(
            "AI issue generation title exceeds {TITLE_MAX_LEN} chars"
        )));
    }
    if is_preface_line(&title) {
        return Err(RiptaskError::General(
            "AI issue generation returned a conversational title".into(),
        ));
    }
    if ends_with_question_mark(&title) {
        return Err(RiptaskError::General(
            "AI issue generation returned a question title".into(),
        ));
    }

    let body = sanitize_issue_body(&parsed.body)?;
    Ok(GeneratedIssueContent { title, body })
}

fn sanitize_issue_body(raw: &str) -> Result<String, RiptaskError> {
    let trimmed = raw.trim();
    let lines: Vec<&str> = trimmed.lines().collect();

    let mut start = 0usize;
    while start < lines.len() && is_preface_line(lines[start]) {
        start += 1;
    }
    if start > 0 {
        while start < lines.len() && lines[start].trim().is_empty() {
            start += 1;
        }
        if start < lines.len() && is_horizontal_rule(lines[start]) {
            start += 1;
            while start < lines.len() && lines[start].trim().is_empty() {
                start += 1;
            }
        }
    }

    let mut end = lines.len();
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    if let Some((paragraph_start, paragraph)) = trailing_paragraph(&lines[start..end])
        && is_followup_paragraph(&paragraph)
    {
        end = start + paragraph_start;
        while end > start && lines[end - 1].trim().is_empty() {
            end -= 1;
        }
        if end > start && is_horizontal_rule(lines[end - 1]) {
            end -= 1;
            while end > start && lines[end - 1].trim().is_empty() {
                end -= 1;
            }
        }
    }

    let sanitized = lines[start..end].join("\n").trim().to_owned();
    if sanitized.is_empty() {
        return Err(RiptaskError::General(
            "AI body sanitization removed all content".into(),
        ));
    }
    Ok(sanitized)
}

fn is_preface_line(line: &str) -> bool {
    static PREFACE_RE: OnceLock<Regex> = OnceLock::new();
    PREFACE_RE
        .get_or_init(|| {
            Regex::new(
                r"(?i)^(I'?ll|I will|I'?m going to|Let me|Here'?s|Here is|Based on (the )?(context|the above|your)|Looking at|Reviewing|Sure[,!]?|Certainly[,!]?|Of course[,!]?)\b.*",
            )
            .expect("valid preface regex")
        })
        .is_match(&normalize_apostrophes(line.trim()))
}

/// Normalize Unicode "smart" apostrophes (U+2019) to ASCII apostrophes so the
/// preface regex catches phrases like "I’ll" alongside "I'll".
fn normalize_apostrophes(text: &str) -> String {
    text.replace('\u{2019}', "'")
}

fn ends_with_question_mark(text: &str) -> bool {
    matches!(text.chars().next_back(), Some('?' | '\u{FF1F}'))
}

fn is_followup_paragraph(paragraph: &str) -> bool {
    static FOLLOWUP_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    FOLLOWUP_PATTERNS
        .get_or_init(|| {
            vec![
                Regex::new(r"(?is)^Would you like (me )?to .*\?\s*$")
                    .expect("valid followup regex"),
                Regex::new(r"(?is)^Let me know (if|whether) .*\.?\s*$")
                    .expect("valid followup regex"),
                Regex::new(r"(?is)^Want me to .*\?\s*$").expect("valid followup regex"),
                Regex::new(r"(?is)^Should I .*\?\s*$").expect("valid followup regex"),
                Regex::new(r"(?is)^Do you want .*\?\s*$").expect("valid followup regex"),
            ]
        })
        .iter()
        .any(|pattern| pattern.is_match(paragraph.trim()))
}

fn trailing_paragraph(lines: &[&str]) -> Option<(usize, String)> {
    let mut end = lines.len();
    while end > 0 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }

    let mut start = end - 1;
    while start > 0 && !lines[start - 1].trim().is_empty() {
        start -= 1;
    }
    Some((start, lines[start..end].join("\n")))
}

fn is_horizontal_rule(line: &str) -> bool {
    line.trim() == "---"
}

fn validate_commit_message_output(raw: &str) -> Result<String, RiptaskError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RiptaskError::General(
            "AI returned empty commit message".into(),
        ));
    }
    if trimmed.starts_with("```") || trimmed.contains("\n```") {
        return Err(RiptaskError::General(
            "AI returned fenced commit message; expected raw text".into(),
        ));
    }

    let mut lines = trimmed.lines();
    let subject = lines.next().unwrap_or("").trim_end();
    let subject_chars = subject.chars().count();
    if subject_chars > 72 {
        return Err(RiptaskError::General(format!(
            "AI commit subject exceeds 72 chars ({subject_chars} chars)"
        )));
    }
    if subject.ends_with('.') {
        return Err(RiptaskError::General(
            "AI commit subject ends with a period".into(),
        ));
    }

    static SUBJECT_RE: OnceLock<Regex> = OnceLock::new();
    let re = SUBJECT_RE.get_or_init(|| {
        Regex::new(
            r"^(feat|fix|refactor|docs|test|chore|ci|style|perf|build)(\([a-z0-9_./-]+\))?: \S.*",
        )
        .expect("valid commit subject regex")
    });
    if !re.is_match(subject) {
        return Err(RiptaskError::General(format!(
            "AI commit subject does not match conventional commits: {subject:?}"
        )));
    }
    if is_preface_line(subject) {
        return Err(RiptaskError::General(
            "AI commit subject is a conversational opener".into(),
        ));
    }

    if let Some(blank) = lines.next()
        && !blank.trim().is_empty()
    {
        return Err(RiptaskError::General(
            "AI commit message missing blank line after subject".into(),
        ));
    }
    for line in trimmed.lines().skip(2) {
        if line.chars().count() > 72 {
            return Err(RiptaskError::General(format!(
                "AI commit body line exceeds 72 chars: {line:?}"
            )));
        }
    }

    Ok(trimmed.to_owned())
}

fn run_ai(template: &str, system: &str, input: &str) -> Result<String, RiptaskError> {
    let mut input_tempfile = tempfile::NamedTempFile::new()
        .map_err(|e| {
            RiptaskError::General(format!(
                "failed to create AI input temp file: {e}\n  hint: check filesystem permissions and temporary directory availability"
            ))
        })?;
    input_tempfile
        .write_all(input.as_bytes())
        .map_err(|e| {
            RiptaskError::General(format!(
                "failed to write AI input temp file: {e}\n  hint: check filesystem permissions and temporary directory availability"
            ))
        })?;
    let input_file_path = input_tempfile.path().to_string_lossy().to_string();

    let system_escaped = escape(Cow::Borrowed(system));
    let input_escaped = escape(Cow::Borrowed(input));

    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.add_template("cmd", template).map_err(|e| {
        RiptaskError::Config(format!(
            "invalid ai.command template: {e}\n  hint: check ai.command syntax in config.yaml"
        ))
    })?;
    let tmpl = env.get_template("cmd").map_err(|e| {
        RiptaskError::Config(format!(
            "invalid ai.command template: {e}\n  hint: check ai.command syntax in config.yaml"
        ))
    })?;
    let rendered = tmpl
        .render(minijinja::context! {
            system => system_escaped.as_ref(),
            input => input_escaped.as_ref(),
            input_file => &input_file_path,
        })
        .map_err(|e| {
            RiptaskError::Config(format!(
                "failed to render ai.command template: {e}\n  hint: check ai.command syntax in config.yaml"
            ))
        })?;

    let output = Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .output()
        .map_err(|e| {
            RiptaskError::General(format!(
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
            ErrorKind::Auth => Err(RiptaskError::Auth(detail)),
            ErrorKind::Config => Err(RiptaskError::Config(detail)),
            ErrorKind::Network => Err(RiptaskError::Unreachable(detail)),
            ErrorKind::Execution => Err(RiptaskError::General(detail)),
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
    fn generate_issue_content_rejects_non_json_prose() {
        let backend = TemplateAiBackend {
            command_template:
                "printf '%s' 'Investigate mobile timeout\n\n## Description\n\nDetails'".into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("non-json prose should be rejected");
        assert!(err.to_string().contains("malformed JSON"));
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
    fn sanitize_strips_preface_rule_and_trailing_question() {
        let input = "I'll generate a concise issue body for this GitHub issue. Based on the context, here's a suggested format:\n\n---\n\n## Description\n\nFix the login bug.\n\n- [ ] Reproduce\n- [ ] Patch\n\n---\n\nWould you like me to adjust the description, add more detail, or modify any of the checklist items?";
        let sanitized = sanitize_issue_body(input).expect("sanitized body");
        assert!(sanitized.starts_with("## Description"));
        assert!(sanitized.ends_with("- [ ] Patch"));
        assert!(!sanitized.contains("I'll generate"));
        assert!(!sanitized.contains("Would you like me"));
    }

    #[test]
    fn sanitize_strips_preface_only_when_no_rule() {
        let input = "Here's the issue body:\n\n## Description\n\nFix bug.";
        let sanitized = sanitize_issue_body(input).expect("sanitized body");
        assert_eq!(sanitized, "## Description\n\nFix bug.");
    }

    #[test]
    fn sanitize_passthrough_clean_body() {
        let input = "## Description\n\nFix bug.";
        let sanitized = sanitize_issue_body(input).expect("sanitized body");
        assert_eq!(sanitized, input);
    }

    #[test]
    fn sanitize_preserves_internal_horizontal_rule() {
        let input = "## Section A\n\nBody.\n\n---\n\n## Section B\n\nMore body.";
        let sanitized = sanitize_issue_body(input).expect("sanitized body");
        assert_eq!(sanitized, input);
    }

    #[test]
    fn sanitize_preserves_body_starting_with_rule() {
        let input = "---\n\n## Description\n\nFix bug.";
        let sanitized = sanitize_issue_body(input).expect("sanitized body");
        assert_eq!(sanitized, input);
    }

    #[test]
    fn sanitize_preserves_yaml_example_with_dashes() {
        let input = "## Description\n\n```yaml\n---\nfake: frontmatter\n---\n```\n";
        let sanitized = sanitize_issue_body(input).expect("sanitized body");
        assert_eq!(sanitized, input.trim());
    }

    #[test]
    fn sanitize_strips_only_matching_trailing_question() {
        let input = "## Description\n\nFix bug.\n\nWould you like me to adjust the description?";
        let sanitized = sanitize_issue_body(input).expect("sanitized body");
        assert_eq!(sanitized, "## Description\n\nFix bug.");
    }

    #[test]
    fn sanitize_preserves_legitimate_question_in_body() {
        let input = "## Description\n\nWhy does the login form reject valid emails?";
        let sanitized = sanitize_issue_body(input).expect("sanitized body");
        assert_eq!(sanitized, input);
    }

    #[test]
    fn sanitize_returns_error_when_emptied() {
        let input = "I'll generate a body.\n\n---\n\nWould you like me to adjust?";
        let err = sanitize_issue_body(input).expect_err("empty sanitized body");
        assert!(err.to_string().contains("removed all content"));
    }

    #[test]
    fn generate_body_returns_sanitized_output() {
        let backend = TemplateAiBackend {
            command_template: "cat <<'EOF'\nI'll generate a concise issue body. Based on the context, here's a suggested format:\n\n---\n\n## Description\n\nFix the login bug.\n\n- [ ] Reproduce\n- [ ] Patch\n\n---\n\nWould you like me to adjust the description?\nEOF".into(),
        };

        let result = backend.generate_body("context").expect("sanitized body");
        assert_eq!(
            result,
            "## Description\n\nFix the login bug.\n\n- [ ] Reproduce\n- [ ] Patch"
        );
    }

    #[test]
    fn generate_issue_content_sanitizes_parsed_body() {
        let backend = TemplateAiBackend {
            command_template: "cat <<'EOF'\n{\"title\":\"Fix login\",\"body\":\"I'll generate a concise issue body. Based on the context, here's a suggested format:\\n\\n---\\n\\n## Description\\n\\nFix the login bug.\\n\\n- [ ] Reproduce\\n- [ ] Patch\\n\\n---\\n\\nWould you like me to adjust the description?\"}\nEOF".into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("issue content");

        assert_eq!(result.title, "Fix login");
        assert_eq!(
            result.body,
            "## Description\n\nFix the login bug.\n\n- [ ] Reproduce\n- [ ] Patch"
        );
    }

    #[test]
    fn generate_issue_content_rejects_prosed_fallback_body() {
        let backend = TemplateAiBackend {
            command_template: "cat <<'EOF'\nI'll generate a concise issue body. Based on the context, here's a suggested format:\n\n---\n\n# Investigate mobile timeout\n\n## Description\n\nDetails\n\n---\n\nWould you like me to adjust the description?\nEOF".into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("prose fallback should be rejected");
        assert!(err.to_string().contains("malformed JSON"));
    }

    #[test]
    fn generate_issue_content_rejects_fenced_json() {
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\n```json\n{\"title\":\"Fix login\",\"body\":\"Details\"}\n```\nEOF"
                    .into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("fenced json should be rejected");
        assert!(err.to_string().contains("fenced output"));
    }

    #[test]
    fn generate_issue_content_rejects_extra_keys() {
        let backend = TemplateAiBackend {
            command_template:
                "printf '%s' '{\"title\":\"Fix login\",\"body\":\"Details\",\"notes\":\"z\"}'"
                    .into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("extra keys should be rejected");
        assert!(err.to_string().contains("malformed JSON"));
    }

    #[test]
    fn generate_issue_content_rejects_multiline_title() {
        let backend = TemplateAiBackend {
            command_template: "printf '%s' '{\"title\":\"line1\\nline2\",\"body\":\"Details\"}'"
                .into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("multiline title should be rejected");
        assert!(err.to_string().contains("multiline title"));
    }

    #[test]
    fn generate_issue_content_rejects_question_title() {
        let backend = TemplateAiBackend {
            command_template:
                "printf '%s' '{\"title\":\"What should we do?\",\"body\":\"Details\"}'".into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("question title should be rejected");
        assert!(err.to_string().contains("question title"));
    }

    #[test]
    fn generate_issue_content_rejects_conversational_title() {
        let backend = TemplateAiBackend {
            command_template: "cat <<'EOF'\n{\"title\":\"Looking at the diff, you've made two important improvements...\",\"body\":\"Details\"}\nEOF".into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("conversational title should be rejected");
        assert!(err.to_string().contains("conversational"));
    }

    #[test]
    fn generate_issue_content_rejects_overlong_title() {
        let title = "a".repeat(TITLE_MAX_LEN + 1);
        let backend = TemplateAiBackend {
            command_template: format!(
                "printf '%s' '{{\"title\":\"{title}\",\"body\":\"Details\"}}'"
            ),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("overlong title should be rejected");
        assert!(err.to_string().contains("exceeds"));
    }

    #[test]
    fn generate_issue_content_strips_trailing_prose_from_json_body() {
        let backend = TemplateAiBackend {
            command_template: "cat <<'EOF'\n{\"title\":\"Fix login\",\"body\":\"## Description\\n\\nDetails\\n\\nWould you like me to commit these changes?\"}\nEOF".into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("issue content");
        assert_eq!(result.body, "## Description\n\nDetails");
    }

    #[test]
    fn generate_issue_content_real_example_a_rejected() {
        let backend = TemplateAiBackend {
            command_template: "cat <<'EOF'\nLooking at the diff, you've made two important improvements to the agent Dockerfile:\n\n1. **Retry loop for package installation** — Wraps `zypper install` in a 3-attempt loop with metadata refresh between retries to handle temporary CDN 404s\n2. **Pre-create bind mount parent directories** — Creates `.local/lib` and `.local/state/claude-cost` with correct ownership before the non-root user runs, preventing Docker's auto-creation as root:root from breaking writes\n\nWould you like me to commit these changes? I can use the `/commit` skill to create a conventional commit with the full context.\nEOF".into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("real example a should be rejected");
        assert!(err.to_string().contains("malformed JSON"));
    }

    #[test]
    fn generate_issue_content_real_example_b_rejected() {
        let backend = TemplateAiBackend {
            command_template: "cat <<'EOF'\nLooking at these changes, I can see you're adding support for:\n1. **MCP auth caching** (`.claude.json` and `mcp-needs-auth-cache.json` to sync files)\n2. **Plugins directory** (adding `plugins/` to linked directories)\n\nWhat would you like me to do with these changes?\n- Commit them (with a commit message)?\n- Review them for completeness?\nEOF".into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("real example b should be rejected");
        assert!(err.to_string().contains("malformed JSON"));
    }

    #[test]
    fn commit_message_validator_accepts_valid_message() {
        let message = "feat(cli): add new flag\n\nDetail line.";
        assert_eq!(
            validate_commit_message_output(message).expect("valid commit message"),
            message
        );
    }

    #[test]
    fn commit_message_validator_rejects_fenced() {
        let err = validate_commit_message_output("```text\nfeat(cli): add new flag\n```")
            .expect_err("fenced commit message should be rejected");
        assert!(err.to_string().contains("fenced commit message"));
    }

    #[test]
    fn commit_message_validator_rejects_conversational() {
        let err = validate_commit_message_output(
            "Looking at the diff, you've made two important improvements...",
        )
        .expect_err("conversational commit subject should be rejected");
        assert!(
            err.to_string().contains("exceeds 72 chars")
                || err.to_string().contains("ends with a period")
                || err.to_string().contains("conventional commits")
                || err.to_string().contains("conversational opener")
        );
    }

    #[test]
    fn commit_message_validator_rejects_invalid_subject() {
        let err = validate_commit_message_output("added a thing\n\nDetail line.")
            .expect_err("invalid subject should be rejected");
        assert!(err.to_string().contains("conventional commits"));
    }

    #[test]
    fn commit_message_validator_rejects_overlong_subject() {
        let subject = format!("feat: {}", "a".repeat(67));
        let err = validate_commit_message_output(&subject)
            .expect_err("overlong subject should be rejected");
        assert!(err.to_string().contains("exceeds 72 chars"));
    }

    #[test]
    fn commit_message_validator_rejects_overlong_body_line() {
        let message = format!("feat(cli): add new flag\n\n{}", "a".repeat(73));
        let err = validate_commit_message_output(&message)
            .expect_err("overlong body line should be rejected");
        assert!(err.to_string().contains("body line exceeds 72 chars"));
    }

    #[test]
    fn commit_message_validator_rejects_missing_blank_line() {
        let err = validate_commit_message_output("feat(cli): add new flag\nDetail line.")
            .expect_err("missing blank line should be rejected");
        assert!(err.to_string().contains("missing blank line"));
    }

    #[test]
    fn commit_message_validator_rejects_trailing_period() {
        let err = validate_commit_message_output("feat(cli): add new flag.\n\nDetail line.")
            .expect_err("trailing period should be rejected");
        assert!(err.to_string().contains("ends with a period"));
    }

    #[test]
    fn commit_message_validator_accepts_multibyte_subject_within_char_limit() {
        // 72 multibyte chars total: "feat(cli): " (11) + 61 'é's = 72 chars,
        // but 11 + 61*2 = 133 bytes. Must pass char-based limit.
        let subject = format!("feat(cli): {}", "é".repeat(61));
        assert_eq!(subject.chars().count(), 72);
        assert!(subject.len() > 72);
        let result = validate_commit_message_output(&subject)
            .expect("multibyte subject within char limit should be accepted");
        assert_eq!(result, subject);
    }

    #[test]
    fn commit_message_validator_accepts_multibyte_body_within_char_limit() {
        // body line: 72 multibyte chars = 144 bytes.
        let body_line = "é".repeat(72);
        let message = format!("feat(cli): add\n\n{body_line}");
        assert!(body_line.len() > 72);
        let result = validate_commit_message_output(&message)
            .expect("multibyte body line within char limit should be accepted");
        assert_eq!(result, message);
    }

    #[test]
    fn generate_issue_content_rejects_smart_apostrophe_conversational_title() {
        // U+2019 right single quotation mark instead of ASCII apostrophe.
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\n{\"title\":\"I\u{2019}ll fix the bug\",\"body\":\"Details\"}\nEOF"
                    .into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("smart-apostrophe conversational title should be rejected");
        assert!(err.to_string().contains("conversational"));
    }

    #[test]
    fn generate_issue_content_rejects_fullwidth_question_title() {
        // U+FF1F fullwidth question mark.
        let backend = TemplateAiBackend {
            command_template: "printf '%s' '{\"title\":\"What now\u{FF1F}\",\"body\":\"Details\"}'"
                .into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("fullwidth question title should be rejected");
        assert!(err.to_string().contains("question title"));
    }

    #[test]
    fn auth_like_ai_errors_map_to_auth() {
        let result = run_ai(
            "echo '401 unauthorized token missing' >&2; exit 1",
            "sys",
            "in",
        );
        match result {
            Err(RiptaskError::Auth(message)) => assert!(message.contains("hint:")),
            other => panic!("expected auth error, got {other:?}"),
        }
    }

    #[test]
    fn nonexistent_ai_command_maps_to_config() {
        let result = run_ai("nonexistent_command_xyz_12345 {{input}}", "sys", "in");
        assert!(matches!(result, Err(RiptaskError::Config(_))));
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
        assert!(matches!(result, Err(RiptaskError::Auth(_))));
    }

    #[test]
    fn token_limit_errors_are_not_auth() {
        let result = run_ai("echo 'max tokens exceeded' >&2; exit 1", "sys", "in");
        assert!(
            !matches!(result, Err(RiptaskError::Auth(_))),
            "token limit error should not be classified as auth"
        );
    }
}
