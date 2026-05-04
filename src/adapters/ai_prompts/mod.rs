//! AI prompt assets. Each method on `AiBackend` corresponds to one task-specific
//! prompt file in this directory. The shared "house rules" preamble is composed
//! in front of every per-task prompt at call time.

const HOUSE_RULES: &str = include_str!("output_discipline.md");

const GENERATE_ISSUE_CONTENT: &str = include_str!("generate_issue_content.md");
const GENERATE_BODY: &str = include_str!("generate_body.md");
const SUGGEST_PROJECT_KEY: &str = include_str!("suggest_project_key.md");
const GENERATE_PR_DESCRIPTION: &str = include_str!("generate_pr_description.md");
const TRIAGE: &str = include_str!("triage.md");
const SUMMARIZE: &str = include_str!("summarize.md");
const ASK: &str = include_str!("ask.md");
const UPDATE_PR_DESCRIPTION: &str = include_str!("update_pr_description.md");
const GENERATE_COMMIT_MESSAGE: &str = include_str!("generate_commit_message.md");

pub(crate) const ISSUE_CONTENT_SCHEMA: &str = include_str!("schemas/issue_content.schema.json");
pub(crate) const TRIAGE_SCHEMA: &str = include_str!("schemas/triage.schema.json");

fn compose(task: &str) -> String {
    format!("{HOUSE_RULES}\n\n{task}")
}

fn compose_with_schema(task: &str, schema: &str) -> String {
    format!("{HOUSE_RULES}\n\n{task}\n\n<output_schema>\n{schema}\n</output_schema>\n")
}

pub fn generate_issue_content_system() -> String {
    compose_with_schema(GENERATE_ISSUE_CONTENT, ISSUE_CONTENT_SCHEMA)
}

pub fn generate_body_system() -> String {
    compose(GENERATE_BODY)
}

pub fn suggest_project_key_system() -> String {
    compose(SUGGEST_PROJECT_KEY)
}

pub fn generate_pr_description_system() -> String {
    compose(GENERATE_PR_DESCRIPTION)
}

pub fn triage_system() -> String {
    compose_with_schema(TRIAGE, TRIAGE_SCHEMA)
}

pub fn summarize_system() -> String {
    compose(SUMMARIZE)
}

pub fn ask_system() -> String {
    compose(ASK)
}

pub fn update_pr_description_system() -> String {
    compose(UPDATE_PR_DESCRIPTION)
}

pub fn generate_commit_message_system() -> String {
    compose(GENERATE_COMMIT_MESSAGE)
}

#[cfg(test)]
pub(crate) fn house_rules() -> &'static str {
    HOUSE_RULES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn house_rules_is_non_empty() {
        assert!(!HOUSE_RULES.is_empty());
    }

    #[test]
    fn every_task_prompt_is_non_empty() {
        for prompt in [
            GENERATE_ISSUE_CONTENT,
            GENERATE_BODY,
            SUGGEST_PROJECT_KEY,
            GENERATE_PR_DESCRIPTION,
            TRIAGE,
            SUMMARIZE,
            ASK,
            UPDATE_PR_DESCRIPTION,
            GENERATE_COMMIT_MESSAGE,
        ] {
            assert!(!prompt.is_empty());
        }
    }

    #[test]
    fn every_composed_system_includes_house_rules() {
        for system in [
            generate_issue_content_system(),
            generate_body_system(),
            suggest_project_key_system(),
            generate_pr_description_system(),
            triage_system(),
            summarize_system(),
            ask_system(),
            update_pr_description_system(),
            generate_commit_message_system(),
        ] {
            assert!(system.starts_with(HOUSE_RULES));
        }
    }

    #[test]
    fn house_rules_bans_known_anti_patterns() {
        let rules = house_rules();
        assert!(rules.contains("Looking at"));
        assert!(rules.contains("Would you like"));
        assert!(rules.contains("Let me know"));
    }

    #[test]
    fn issue_content_schema_is_valid_json() {
        let parsed: serde_json::Value = serde_json::from_str(ISSUE_CONTENT_SCHEMA)
            .expect("issue content schema must be valid JSON");
        assert_eq!(parsed["type"], "object");
        assert_eq!(parsed["additionalProperties"], false);
        let required = parsed["required"]
            .as_array()
            .expect("required must be an array");
        let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required_names.contains(&"title"));
        assert!(required_names.contains(&"body"));
    }

    #[test]
    fn generate_issue_content_system_includes_schema() {
        let system = generate_issue_content_system();
        assert!(system.contains("<output_schema>"));
        assert!(system.contains("</output_schema>"));
        assert!(system.contains("\"additionalProperties\": false"));
        assert!(system.contains("\"title\""));
        assert!(system.contains("\"body\""));
    }

    #[test]
    fn triage_schema_is_valid_json() {
        let parsed: serde_json::Value =
            serde_json::from_str(TRIAGE_SCHEMA).expect("triage schema must be valid JSON");
        assert_eq!(parsed["type"], "object");
        assert_eq!(parsed["additionalProperties"], false);
        let required = parsed["required"]
            .as_array()
            .expect("required must be an array");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"status"));
        assert!(names.contains(&"priority"));
        assert!(names.contains(&"labels"));
    }

    #[test]
    fn triage_system_includes_schema() {
        let system = triage_system();
        assert!(system.contains("<output_schema>"));
        assert!(system.contains("\"status\""));
        assert!(system.contains("\"priority\""));
        assert!(system.contains("\"labels\""));
    }
}
