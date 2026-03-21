use crate::error::RiptskError;
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
pub struct CommandAiBackend {
    pub binary: String,
    pub model: String,
}

impl AiBackend for CommandAiBackend {
    fn generate_body(&self, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.binary,
            &self.model,
            "Generate a concise issue body with a description and checklist.",
            context,
        )
    }

    fn generate_pr_description(&self, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.binary,
            &self.model,
            "Generate a concise PR description summarizing the changes. Include a summary section and key changes. Do not include the title.",
            context,
        )
    }

    fn triage(&self, issue_context: &str) -> Result<TriageSuggestion, RiptskError> {
        let output = run_ai(
            &self.binary,
            &self.model,
            "Return JSON with keys state, priority, labels.",
            issue_context,
        )?;
        let parsed: serde_json::Value = serde_json::from_str(&output).map_err(|error| {
            RiptskError::General(format!("invalid AI triage response: {error}"))
        })?;
        Ok(TriageSuggestion {
            state: parsed
                .get("state")
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
            &self.binary,
            &self.model,
            "Summarize issue status concisely.",
            issues,
        )
    }

    fn ask(&self, question: &str, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.binary,
            &self.model,
            "Answer the user's question from the provided issue corpus.",
            &format!("Question: {question}\n\nIssues:\n{context}"),
        )
    }

    fn update_pr_description(&self, context: &str) -> Result<String, RiptskError> {
        run_ai(
            &self.binary,
            &self.model,
            "Update this PR description based on the current changes. Keep it concise and focused on implementation details.",
            context,
        )
    }
}

fn run_ai(binary: &str, model: &str, system: &str, input: &str) -> Result<String, RiptskError> {
    let output = Command::new(binary)
        .arg("--system")
        .arg(system)
        .arg("--model")
        .arg(model)
        .arg(input)
        .output()
        .map_err(|error| RiptskError::General(format!("failed to invoke {binary}: {error}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(RiptskError::General(format!(
            "{binary} exited unsuccessfully"
        )))
    }
}
