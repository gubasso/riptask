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
        let raw = run_ai(
            &self.command_template,
            &system,
            &format!("repo: {repo_name}\nbackend: {backend_type}\n"),
        )?;
        validate_project_key_output(&raw, existing_keys)
    }

    fn generate_pr_description(&self, context: &str) -> Result<String, RiptaskError> {
        let system = ai_prompts::generate_pr_description_system();
        run_ai(&self.command_template, &system, context)
    }

    fn triage(&self, issue_context: &str) -> Result<TriageSuggestion, RiptaskError> {
        let system = ai_prompts::triage_system();
        let output = run_ai(&self.command_template, &system, issue_context)?;
        parse_triage_output(&output)
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TriageResponse {
    status: Option<String>,
    priority: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
}

const TRIAGE_STATUS_ALLOWED: &[&str] = &["todo", "doing", "blocked", "done"];
const TRIAGE_PRIORITY_ALLOWED: &[&str] = &["low", "medium", "high", "critical"];
const TRIAGE_LABEL_MAX_LEN: usize = 64;
const TRIAGE_LABELS_MAX_ITEMS: usize = 10;

fn parse_triage_output(output: &str) -> Result<TriageSuggestion, RiptaskError> {
    if output.trim().is_empty() {
        return Err(RiptaskError::General(
            "AI returned empty output for triage".into(),
        ));
    }
    let json_slice = extract_first_json_object(output).ok_or_else(|| {
        RiptaskError::General(
            "AI returned no JSON object for triage; expected a single {…} object".into(),
        )
    })?;
    let parsed = serde_json::from_str::<TriageResponse>(json_slice).map_err(|err| {
        RiptaskError::General(format!("AI returned malformed JSON for triage: {err}"))
    })?;

    let status = match parsed.status {
        Some(s) => {
            let trimmed = s.trim().to_owned();
            if trimmed.is_empty() {
                None
            } else if TRIAGE_STATUS_ALLOWED.contains(&trimmed.as_str()) {
                Some(trimmed)
            } else {
                return Err(RiptaskError::General(format!(
                    "AI triage returned an invalid status: {trimmed:?} (allowed: {})",
                    TRIAGE_STATUS_ALLOWED.join(", ")
                )));
            }
        }
        None => None,
    };

    let priority = match parsed.priority {
        Some(p) => {
            let trimmed = p.trim().to_owned();
            if trimmed.is_empty() {
                None
            } else if TRIAGE_PRIORITY_ALLOWED.contains(&trimmed.as_str()) {
                Some(trimmed)
            } else {
                return Err(RiptaskError::General(format!(
                    "AI triage returned an invalid priority: {trimmed:?} (allowed: {})",
                    TRIAGE_PRIORITY_ALLOWED.join(", ")
                )));
            }
        }
        None => None,
    };

    if parsed.labels.len() > TRIAGE_LABELS_MAX_ITEMS {
        return Err(RiptaskError::General(format!(
            "AI triage returned too many labels ({} > {TRIAGE_LABELS_MAX_ITEMS})",
            parsed.labels.len()
        )));
    }
    let mut labels = Vec::with_capacity(parsed.labels.len());
    for label in parsed.labels {
        let trimmed = label.trim().to_owned();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > TRIAGE_LABEL_MAX_LEN {
            return Err(RiptaskError::General(format!(
                "AI triage returned a label exceeding {TRIAGE_LABEL_MAX_LEN} chars: {trimmed:?}"
            )));
        }
        labels.push(trimmed);
    }

    Ok(TriageSuggestion {
        state: status,
        priority,
        labels,
    })
}

fn parse_issue_content_output(output: &str) -> Result<GeneratedIssueContent, RiptaskError> {
    if output.trim().is_empty() {
        return Err(RiptaskError::General(
            "AI returned empty output for issue generation".into(),
        ));
    }
    let json_slice = extract_first_json_object(output).ok_or_else(|| {
        RiptaskError::General(
            "AI returned no JSON object for issue generation; expected a single {…} object".into(),
        )
    })?;
    let parsed =
        serde_json::from_str::<GeneratedIssueContentResponse>(json_slice).map_err(|err| {
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

/// Extract the first complete JSON object (`{...}`) from arbitrary text.
///
/// Walks the input as bytes, tracks brace depth, and is aware of JSON string
/// state (so braces inside `"..."` strings, including escaped quotes, do not
/// affect the depth count). Returns the slice from the first `{` to the
/// matching `}`, inclusive. Anything before or after that slice — preamble,
/// code fences, trailing prose — is ignored.
///
/// This is structurally bounded: the returned slice still has to parse as
/// valid JSON, and downstream code still validates every field. It is not a
/// salvage-from-prose path.
fn extract_first_json_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escape = false;
    for (offset, &b) in bytes[start..].iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            match b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + offset + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_optional_outer_fence(text: &str) -> String {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed.to_owned();
    };
    let Some((header, body)) = rest.split_once('\n') else {
        return trimmed.to_owned();
    };
    let header = header.trim();
    if !header.is_empty() && !header.chars().all(|c| c.is_alphanumeric()) {
        return trimmed.to_owned();
    }
    let body_trimmed = body.trim_end();
    let Some(inner) = body_trimmed.strip_suffix("```") else {
        return trimmed.to_owned();
    };
    inner.trim().to_owned()
}

fn validate_project_key_output(
    raw: &str,
    existing_keys: &[String],
) -> Result<String, RiptaskError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RiptaskError::General(
            "AI returned empty output for project key".into(),
        ));
    }
    // Reject any output that would imply prose, formatting, or explanation.
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(RiptaskError::General(
            "AI project key output spans multiple lines".into(),
        ));
    }
    static KEY_RE: OnceLock<Regex> = OnceLock::new();
    let re =
        KEY_RE.get_or_init(|| Regex::new(r"^[A-Z0-9]{2,10}$").expect("valid project key regex"));
    if !re.is_match(trimmed) {
        return Err(RiptaskError::General(format!(
            "AI project key {trimmed:?} does not match required pattern ^[A-Z0-9]{{2,10}}$"
        )));
    }
    if existing_keys.iter().any(|existing| existing == trimmed) {
        return Err(RiptaskError::General(format!(
            "AI project key {trimmed:?} collides with an already-taken key"
        )));
    }
    Ok(trimmed.to_owned())
}

fn validate_commit_message_output(raw: &str) -> Result<String, RiptaskError> {
    let unwrapped = strip_optional_outer_fence(raw);
    let trimmed = unwrapped.trim();
    if trimmed.is_empty() {
        return Err(RiptaskError::General(
            "AI returned empty commit message".into(),
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
        assert!(err.to_string().contains("no JSON object"));
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
        assert!(err.to_string().contains("no JSON object"));
    }

    #[test]
    fn generate_issue_content_unwraps_fenced_json() {
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\n```json\n{\"title\":\"Fix login\",\"body\":\"Details\"}\n```\nEOF"
                    .into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("fenced json with valid content should be unwrapped and parsed");
        assert_eq!(result.title, "Fix login");
        assert_eq!(result.body, "Details");
    }

    #[test]
    fn generate_issue_content_unwraps_bare_fenced_json() {
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\n```\n{\"title\":\"Fix login\",\"body\":\"Details\"}\n```\nEOF".into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("bare fenced json should be unwrapped and parsed");
        assert_eq!(result.title, "Fix login");
        assert_eq!(result.body, "Details");
    }

    #[test]
    fn generate_issue_content_rejects_fenced_prose() {
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\n```\nLooking at the diff, you've made improvements...\n```\nEOF"
                    .into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("fenced prose with no JSON object should be rejected");
        assert!(err.to_string().contains("no JSON object"));
    }

    #[test]
    fn generate_issue_content_extracts_json_with_prose_preamble() {
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\nHere is the JSON for the issue:\n\n{\"title\":\"Fix login\",\"body\":\"Details\"}\nEOF"
                    .into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("prose preamble before JSON should be ignored");
        assert_eq!(result.title, "Fix login");
        assert_eq!(result.body, "Details");
    }

    #[test]
    fn generate_issue_content_extracts_json_with_prose_postamble() {
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\n{\"title\":\"Fix login\",\"body\":\"Details\"}\n\nLet me know if you need any changes.\nEOF"
                    .into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("prose postamble after JSON should be ignored");
        assert_eq!(result.title, "Fix login");
        assert_eq!(result.body, "Details");
    }

    #[test]
    fn generate_issue_content_extracts_json_with_prose_and_fence() {
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\nSure! Here's the issue:\n\n```json\n{\"title\":\"Fix login\",\"body\":\"Details\"}\n```\n\nLet me know if you'd like changes.\nEOF"
                    .into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("preamble + fenced JSON + postamble should all be ignored");
        assert_eq!(result.title, "Fix login");
        assert_eq!(result.body, "Details");
    }

    #[test]
    fn generate_issue_content_handles_braces_inside_json_strings() {
        let backend = TemplateAiBackend {
            command_template:
                "cat <<'EOF'\n{\"title\":\"Use let x = {a:1} in Rust\",\"body\":\"## Notes\\n\\nNested {braces} inside strings must not break extraction.\"}\nEOF"
                    .into(),
        };

        let result = backend
            .generate_issue_content("context")
            .expect("braces inside JSON string values must not confuse the extractor");
        assert_eq!(result.title, "Use let x = {a:1} in Rust");
        assert!(result.body.contains("{braces}"));
    }

    #[test]
    fn extract_first_json_object_is_string_aware() {
        // Bare unit test for the helper: a `}` inside a string must not close the object.
        let input = r#"prefix {"k":"v with } brace","n":1} suffix"#;
        let extracted = extract_first_json_object(input).expect("balanced object");
        assert_eq!(extracted, r#"{"k":"v with } brace","n":1}"#);
    }

    #[test]
    fn extract_first_json_object_returns_none_without_brace() {
        assert!(extract_first_json_object("Looking at the diff...").is_none());
    }

    #[test]
    fn extract_first_json_object_returns_none_when_unbalanced() {
        assert!(extract_first_json_object("{\"k\": \"v\"").is_none());
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
        assert!(err.to_string().contains("no JSON object"));
    }

    #[test]
    fn generate_issue_content_real_example_b_rejected() {
        let backend = TemplateAiBackend {
            command_template: "cat <<'EOF'\nLooking at these changes, I can see you're adding support for:\n1. **MCP auth caching** (`.claude.json` and `mcp-needs-auth-cache.json` to sync files)\n2. **Plugins directory** (adding `plugins/` to linked directories)\n\nWhat would you like me to do with these changes?\n- Commit them (with a commit message)?\n- Review them for completeness?\nEOF".into(),
        };

        let err = backend
            .generate_issue_content("context")
            .expect_err("real example b should be rejected");
        assert!(err.to_string().contains("no JSON object"));
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
    fn commit_message_validator_unwraps_fenced() {
        let result = validate_commit_message_output("```text\nfeat(cli): add new flag\n```")
            .expect("fenced commit message with valid content should be unwrapped");
        assert_eq!(result, "feat(cli): add new flag");
    }

    #[test]
    fn commit_message_validator_unwraps_bare_fenced() {
        let result = validate_commit_message_output("```\nfeat(cli): add new flag\n```")
            .expect("bare fenced commit message should be unwrapped");
        assert_eq!(result, "feat(cli): add new flag");
    }

    #[test]
    fn commit_message_validator_rejects_fenced_conversational() {
        let err = validate_commit_message_output(
            "```\nLooking at the diff, you've made improvements\n```",
        )
        .expect_err("fenced conversational commit should still be rejected after unwrapping");
        assert!(
            err.to_string().contains("conventional commits")
                || err.to_string().contains("conversational opener")
        );
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

    // --- Triage parser ---

    #[test]
    fn triage_parser_accepts_minimal_object() {
        let parsed = parse_triage_output(r#"{"status":"todo","priority":"high","labels":["bug"]}"#)
            .expect("minimal triage object");
        assert_eq!(parsed.state.as_deref(), Some("todo"));
        assert_eq!(parsed.priority.as_deref(), Some("high"));
        assert_eq!(parsed.labels, vec!["bug".to_string()]);
    }

    #[test]
    fn triage_parser_accepts_nulls_and_empty_labels() {
        let parsed = parse_triage_output(r#"{"status":null,"priority":null,"labels":[]}"#)
            .expect("nulls and empty labels");
        assert!(parsed.state.is_none());
        assert!(parsed.priority.is_none());
        assert!(parsed.labels.is_empty());
    }

    #[test]
    fn triage_parser_extracts_json_with_prose_preamble() {
        let parsed = parse_triage_output(
            "Here is the triage:\n{\"status\":\"todo\",\"priority\":\"high\",\"labels\":[\"bug\"]}",
        )
        .expect("prose preamble should be ignored");
        assert_eq!(parsed.state.as_deref(), Some("todo"));
    }

    #[test]
    fn triage_parser_rejects_non_json_prose() {
        let err = parse_triage_output("Looking at the issue, I think it should be todo.")
            .expect_err("prose with no JSON object should be rejected");
        assert!(err.to_string().contains("no JSON object"));
    }

    #[test]
    fn triage_parser_rejects_invalid_status() {
        let err =
            parse_triage_output(r#"{"status":"in-progress","priority":"high","labels":["bug"]}"#)
                .expect_err("invalid status should be rejected");
        assert!(err.to_string().contains("invalid status"));
    }

    #[test]
    fn triage_parser_rejects_invalid_priority() {
        let err = parse_triage_output(r#"{"status":"todo","priority":"urgent","labels":["bug"]}"#)
            .expect_err("invalid priority should be rejected");
        assert!(err.to_string().contains("invalid priority"));
    }

    #[test]
    fn triage_parser_rejects_extra_keys() {
        let err = parse_triage_output(
            r#"{"status":"todo","priority":"high","labels":["bug"],"notes":"x"}"#,
        )
        .expect_err("extra keys should be rejected");
        assert!(err.to_string().contains("malformed JSON"));
    }

    #[test]
    fn triage_parser_rejects_too_many_labels() {
        let labels: Vec<String> = (0..11).map(|i| format!("\"l{i}\"")).collect();
        let payload = format!(
            r#"{{"status":"todo","priority":"high","labels":[{}]}}"#,
            labels.join(",")
        );
        let err = parse_triage_output(&payload).expect_err("too many labels should be rejected");
        assert!(err.to_string().contains("too many labels"));
    }

    #[test]
    fn triage_parser_rejects_overlong_label() {
        let long = "x".repeat(TRIAGE_LABEL_MAX_LEN + 1);
        let payload = format!(r#"{{"status":"todo","priority":"high","labels":["{long}"]}}"#);
        let err = parse_triage_output(&payload).expect_err("overlong label should be rejected");
        assert!(err.to_string().contains("exceeding"));
    }

    // --- Project-key validator ---

    #[test]
    fn project_key_validator_accepts_valid_key() {
        let key =
            validate_project_key_output("RIPTASK", &[]).expect("valid project key should pass");
        assert_eq!(key, "RIPTASK");
    }

    #[test]
    fn project_key_validator_trims_surrounding_whitespace() {
        let key = validate_project_key_output("  RIPTASK  \n", &[])
            .expect("surrounding whitespace should be trimmed");
        assert_eq!(key, "RIPTASK");
    }

    #[test]
    fn project_key_validator_rejects_lowercase() {
        let err = validate_project_key_output("riptask", &[])
            .expect_err("lowercase keys should be rejected");
        assert!(err.to_string().contains("does not match required pattern"));
    }

    #[test]
    fn project_key_validator_rejects_hyphen() {
        let err = validate_project_key_output("RIP-TASK", &[])
            .expect_err("hyphenated keys should be rejected");
        assert!(err.to_string().contains("does not match required pattern"));
    }

    #[test]
    fn project_key_validator_rejects_prose_preamble() {
        let err = validate_project_key_output("Here is a suggested key: RIPTASK", &[])
            .expect_err("prose preamble should be rejected");
        assert!(err.to_string().contains("does not match required pattern"));
    }

    #[test]
    fn project_key_validator_rejects_backticks() {
        let err = validate_project_key_output("`RIPTASK`", &[])
            .expect_err("backtick wrapping should be rejected");
        assert!(err.to_string().contains("does not match required pattern"));
    }

    #[test]
    fn project_key_validator_rejects_too_short() {
        let err =
            validate_project_key_output("R", &[]).expect_err("single-char keys should be rejected");
        assert!(err.to_string().contains("does not match required pattern"));
    }

    #[test]
    fn project_key_validator_rejects_too_long() {
        let err = validate_project_key_output("ABCDEFGHIJK", &[])
            .expect_err("11-char keys should be rejected");
        assert!(err.to_string().contains("does not match required pattern"));
    }

    #[test]
    fn project_key_validator_rejects_collision() {
        let err = validate_project_key_output("RIPTASK", &["RIPTASK".to_string()])
            .expect_err("collision with existing key should be rejected");
        assert!(err.to_string().contains("collides"));
    }

    #[test]
    fn project_key_validator_rejects_empty() {
        let err =
            validate_project_key_output("", &[]).expect_err("empty output should be rejected");
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn project_key_validator_rejects_multiline() {
        let err = validate_project_key_output("RIPTASK\nDEVCTL", &[])
            .expect_err("multiline output should be rejected");
        assert!(err.to_string().contains("multiple lines"));
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
