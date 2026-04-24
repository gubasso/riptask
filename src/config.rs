use crate::domain::issue::{IssueState, Priority};
use crate::error::RiptaskError;
use crate::models::{
    AiConfig, AiFeatures, BackendKind, BoardConfig, DefaultsConfig, RecurringDef, RepoProject,
    SyncConfig, UiConfig,
};
use crate::services::{issue_ids, repo_project_label};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub auto_commit: bool,
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub projects: Vec<RepoProject>,
    #[serde(default)]
    pub boards: Vec<BoardConfig>,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub recurring: Vec<RecurringDef>,
}

pub fn default_config() -> Config {
    Config {
        version: 1,
        auto_commit: false,
        defaults: DefaultsConfig {
            board: "personal".into(),
            status: IssueState::Backlog,
            priority: Priority::Medium,
            assignee: None,
            template: Some("task".into()),
        },
        projects: Vec::new(),
        boards: vec![BoardConfig {
            name: "personal".into(),
            statuses: vec![
                IssueState::Backlog,
                IssueState::Todo,
                IssueState::InProgress,
                IssueState::Done,
            ],
        }],
        ui: UiConfig {
            opener: Some("nvim -R".into()),
            tree_depth: Some(2),
            fzf_opts: Some("--border".into()),
        },
        ai: AiConfig {
            enabled: false,
            command: None,
            features: AiFeatures {
                new_body_gen: true,
                triage: true,
                summarize: true,
                ask: true,
                commit: true,
            },
        },
        sync: SyncConfig {
            conflict_detection: true,
        },
        recurring: Vec::new(),
    }
}

pub fn load_config(path: &Path) -> Result<Config, RiptaskError> {
    let content = fs::read_to_string(path)?;
    let mut config: Config = serde_yaml_ng::from_str(&content).map_err(|error| {
        RiptaskError::Config(format!("failed to parse {}: {error}", path.display()))
    })?;
    validate_config(&mut config)?;
    Ok(config)
}

pub fn save_config(path: &Path, config: &Config) -> Result<(), RiptaskError> {
    // `save_config` takes `&Config` to preserve callers that want to write out
    // the exact struct they built. Callers that load via `load_config` already
    // have a normalized `Config` in memory, and `config_set` normalizes before
    // calling `save_config`.
    let mut validation_view = config.clone();
    validate_config(&mut validation_view)?;
    let content = serde_yaml_ng::to_string(config)
        .map_err(|error| RiptaskError::Config(format!("failed to serialize config: {error}")))?;
    let dir = path
        .parent()
        .ok_or_else(|| RiptaskError::Config("config path has no parent".into()))?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    use std::io::Write;
    file.write_all(content.as_bytes())?;
    file.persist(path)
        .map_err(|error| RiptaskError::Io(error.error))?;
    Ok(())
}

pub fn validate_config(config: &mut Config) -> Result<(), RiptaskError> {
    // Check explicit `key:` syntax before normalization so users get a clear
    // error that names the offending backend instead of a downstream collision.
    for project in &config.projects {
        if let Some(key) = project.key.as_deref() {
            issue_ids::validate_explicit_key_syntax(key).map_err(|message| {
                RiptaskError::Config(format!(
                    "RepoProject '{}' has invalid key: {message}",
                    project.name
                ))
            })?;
        }
    }
    // Populate every group member with its canonical key so that downstream
    // `effective_key` calls return a stable value regardless of which backend
    // instance they are given.
    issue_ids::normalize_backend_keys(&mut config.projects)?;
    issue_ids::validate_no_key_collisions(&config.projects)?;
    require_unique(
        config.projects.iter().map(|project| project.name.as_str()),
        "duplicate RepoProject name in riptask.yaml",
    )?;
    require_unique(
        config.boards.iter().map(|board| board.name.as_str()),
        "duplicate board name in riptask.yaml",
    )?;
    validate_ai_command(config)?;
    validate_projects(config)?;
    Ok(())
}

fn validate_projects(config: &Config) -> Result<(), RiptaskError> {
    for repo_project in &config.projects {
        if repo_project.vc_backend.kind == BackendKind::Jira {
            return Err(RiptaskError::Config(format!(
                "RepoProject '{}' has invalid VCBackend: jira is not allowed",
                repo_project.name
            )));
        }

        if let Some(label) = repo_project.repo_project_label.as_deref() {
            repo_project_label::validate(label)?;
            if repo_project.tasks_backend.kind != BackendKind::Jira {
                return Err(RiptaskError::Config(format!(
                    "RepoProject '{}' sets repo_project_label '{}' but its TasksBackend is {:?}; the label is Jira-only and would be silently ignored",
                    repo_project.name, label, repo_project.tasks_backend.kind
                )));
            }
        }

        if repo_project.tasks_backend.kind == BackendKind::Jira {
            let host = repo_project
                .tasks_backend
                .host
                .as_deref()
                .unwrap_or_default();
            if host.is_empty() {
                return Err(RiptaskError::Config(format!(
                    "Jira TasksBackend for RepoProject '{}' requires a 'host' field (e.g., https://myteam.atlassian.net)",
                    repo_project.name
                )));
            }
            if !host.starts_with("https://") {
                return Err(RiptaskError::Config(format!(
                    "Jira TasksBackend for RepoProject '{}' host must start with https:// (got: {})",
                    repo_project.name, host
                )));
            }
            let jira_project = repo_project
                .tasks_backend
                .jira_project
                .as_deref()
                .unwrap_or_default();
            if jira_project.is_empty() || !jira_project.contains('/') {
                return Err(RiptaskError::Config(format!(
                    "Jira TasksBackend for RepoProject '{}' requires 'jira_project' in org/PROJECT_KEY format (got: {:?})",
                    repo_project.name, repo_project.tasks_backend.jira_project
                )));
            }
        }
    }
    Ok(())
}

fn validate_ai_command(config: &Config) -> Result<(), RiptaskError> {
    if let Some(command) = &config.ai.command {
        // Try to compile the template to catch syntax errors early
        let mut env = minijinja::Environment::new();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        env.add_template("cmd", command).map_err(|error| {
            RiptaskError::Config(format!(
                "invalid ai.command template syntax: {command}: {error}"
            ))
        })?;

        // Verify template contains at least one of the required input placeholders
        if !command.contains("{{input}}") && !command.contains("{{input_file}}") {
            return Err(RiptaskError::Config(
                "ai.command must contain {{{{input}}}} or {{{{input_file}}}} placeholder\n\nExamples:\n  ai.command: \"my-ai-cli --system '{{{{system}}}}' --context '{{{{input}}}}'\"\n  ai.command: \"cat {{{{input_file}}}} | my-ai-cli --system '{{{{system}}}}'\""
                    .into(),
            ));
        }
    }
    Ok(())
}

fn require_unique<'a>(
    iter: impl Iterator<Item = &'a str>,
    message: &str,
) -> Result<(), RiptaskError> {
    let mut seen = HashSet::new();
    for item in iter {
        if !seen.insert(item.to_owned()) {
            return Err(RiptaskError::Config(message.to_owned()));
        }
    }
    Ok(())
}

pub fn config_set(config: &mut Config, key: &str, value: &str) -> Result<(), RiptaskError> {
    match key {
        "auto_commit" => {
            config.auto_commit = value
                .parse()
                .map_err(|_| RiptaskError::Config("auto_commit must be true or false".into()))?
        }
        "defaults.board" => config.defaults.board = value.to_owned(),
        "defaults.status" => {
            config.defaults.status =
                parse_state(value).map_err(|error| RiptaskError::Config(error.to_string()))?
        }
        "defaults.priority" => {
            config.defaults.priority =
                parse_priority(value).map_err(|error| RiptaskError::Config(error.to_string()))?
        }
        "defaults.assignee" => config.defaults.assignee = some_if_not_empty(value),
        "defaults.template" => config.defaults.template = some_if_not_empty(value),
        "ui.opener" => config.ui.opener = some_if_not_empty(value),
        "ui.tree_depth" => {
            config.ui.tree_depth =
                if value.is_empty() {
                    None
                } else {
                    Some(value.parse().map_err(|_| {
                        RiptaskError::Config("ui.tree_depth must be a number".into())
                    })?)
                }
        }
        "ui.fzf_opts" => config.ui.fzf_opts = some_if_not_empty(value),
        "ai.enabled" => {
            config.ai.enabled = value
                .parse()
                .map_err(|_| RiptaskError::Config("ai.enabled must be true or false".into()))?
        }
        "ai.command" => config.ai.command = some_if_not_empty(value),
        "sync.conflict_detection" => {
            config.sync.conflict_detection = value.parse().map_err(|_| {
                RiptaskError::Config("sync.conflict_detection must be true or false".into())
            })?
        }
        _ => {
            return Err(RiptaskError::Config(format!(
                "unsupported config key: {key}"
            )));
        }
    }
    validate_config(config)
}

fn some_if_not_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

pub fn parse_state(value: &str) -> anyhow::Result<IssueState> {
    match value {
        "backlog" => Ok(IssueState::Backlog),
        "todo" => Ok(IssueState::Todo),
        "in-progress" => Ok(IssueState::InProgress),
        "review" => Ok(IssueState::Review),
        "done" => Ok(IssueState::Done),
        _ => Err(anyhow::anyhow!("invalid status: {value}")),
    }
}

pub fn parse_priority(value: &str) -> anyhow::Result<Priority> {
    match value {
        "low" => Ok(Priority::Low),
        "medium" => Ok(Priority::Medium),
        "high" => Ok(Priority::High),
        "urgent" => Ok(Priority::Urgent),
        _ => Err(anyhow::anyhow!("invalid priority: {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{config_set, load_config, parse_priority, parse_state};
    use crate::error::RiptaskError;
    use std::fs;
    use std::path::Path;
    use tempfile::NamedTempFile;

    #[test]
    fn parses_fixture_config() {
        let config = load_config(std::path::Path::new("tests/fixtures/riptask.yaml"))
            .expect("load fixture config");
        assert_eq!(config.version, 1);
        assert_eq!(config.projects.len(), 1);
    }

    #[test]
    fn config_set_whitelist_updates_expected_field() {
        let mut config =
            load_config(std::path::Path::new("tests/fixtures/riptask.yaml")).expect("load config");
        config_set(&mut config, "ui.opener", "less").expect("set opener");
        assert_eq!(config.ui.opener.as_deref(), Some("less"));
        config_set(&mut config, "auto_commit", "true").expect("set auto_commit");
        assert!(config.auto_commit);
    }

    #[test]
    fn config_save_round_trip_is_valid_yaml() {
        let config =
            load_config(std::path::Path::new("tests/fixtures/riptask.yaml")).expect("load config");
        let file = NamedTempFile::new().expect("temp file");
        super::save_config(file.path(), &config).expect("save config");
        let reparsed = load_config(file.path()).expect("reload config");
        assert_eq!(reparsed.version, 1);
    }

    #[test]
    fn parses_state_and_priority() {
        assert!(matches!(
            parse_state("todo").expect("state"),
            crate::domain::issue::IssueState::Todo
        ));
        assert!(matches!(
            parse_priority("high").expect("priority"),
            crate::domain::issue::Priority::High
        ));
    }

    #[test]
    fn config_set_ai_command_stores_value() {
        let mut config = load_config(Path::new("tests/fixtures/riptask.yaml")).expect("load");
        config_set(&mut config, "ai.command", "echo {{input}}").expect("set");
        assert_eq!(config.ai.command.as_deref(), Some("echo {{input}}"));
    }

    #[test]
    fn config_set_ai_command_empty_clears() {
        let mut config = load_config(Path::new("tests/fixtures/riptask.yaml")).expect("load");
        config_set(&mut config, "ai.command", "echo {{input}}").expect("set");
        config_set(&mut config, "ai.command", "").expect("clear");
        assert!(config.ai.command.is_none());
    }

    #[test]
    fn config_rejects_ai_command_without_input_placeholder() {
        let mut config = load_config(Path::new("tests/fixtures/riptask.yaml")).expect("load");
        let result = config_set(&mut config, "ai.command", "echo hello");
        assert!(result.is_err());
    }

    #[test]
    fn load_config_normalizes_legacy_key_without_rewriting_file() {
        let file = NamedTempFile::new().expect("temp file");
        let yaml = r#"version: 1
defaults:
  board: personal
  status: todo
  priority: medium
  assignee: ~
  template: task
projects:
  - name: demo
    vc_backend:
      type: github
      repo: owner/demo
    tasks_backend:
      type: github
      repo: owner/demo
    default_board: personal
boards:
  - name: personal
    statuses: [backlog, todo, in-progress, review, done]
ui:
  opener: "nvim -R"
  tree_depth: 2
  fzf_opts: "--border"
ai:
  enabled: false
  features:
    new_body_gen: true
    triage: true
    summarize: true
    ask: true
sync:
  conflict_detection: true
recurring: []
"#;
        fs::write(file.path(), yaml).expect("write yaml");
        let before = fs::read_to_string(file.path()).expect("before");

        let config = load_config(file.path()).expect("load");

        // Normalization populates the in-memory key from the derived default.
        assert_eq!(config.projects[0].key.as_deref(), Some("DEMO"));
        // But loading must never rewrite the file on disk.
        let after = fs::read_to_string(file.path()).expect("after");
        assert_eq!(before, after);
    }

    #[test]
    fn load_config_returns_structured_key_collision() {
        let file = NamedTempFile::new().expect("temp file");
        let yaml = r#"version: 1
defaults:
  board: personal
  status: todo
  priority: medium
  assignee: ~
  template: task
projects:
  - name: alpha-one
    vc_backend:
      type: github
      repo: owner/alpha
    tasks_backend:
      type: github
      repo: owner/alpha
    default_board: personal
  - name: alpha-two
    vc_backend:
      type: github
      repo: other/alpha
    tasks_backend:
      type: github
      repo: other/alpha
    default_board: personal
boards:
  - name: personal
    statuses: [backlog, todo, in-progress, review, done]
ui:
  opener: "nvim -R"
  tree_depth: 2
  fzf_opts: "--border"
ai:
  enabled: false
  features:
    new_body_gen: true
    triage: true
    summarize: true
    ask: true
sync:
  conflict_detection: true
recurring: []
"#;
        fs::write(file.path(), yaml).expect("write yaml");

        let error = load_config(file.path()).expect_err("collision");
        assert!(matches!(error, RiptaskError::KeyCollision(_)));
    }

    #[test]
    fn load_config_rejects_repo_project_label_on_non_jira_backend() {
        let file = NamedTempFile::new().expect("temp file");
        let yaml = r#"version: 1
defaults:
  board: personal
  status: todo
  priority: medium
  assignee: ~
  template: task
projects:
  - name: demo
    vc_backend:
      type: github
      repo: owner/demo
    tasks_backend:
      type: github
      repo: owner/demo
    default_board: personal
    repo_project_label: proj::demo
boards:
  - name: personal
    statuses: [backlog, todo, in-progress, review, done]
ui:
  opener: "nvim -R"
  tree_depth: 2
  fzf_opts: "--border"
ai:
  enabled: false
  features:
    new_body_gen: true
    triage: true
    summarize: true
    ask: true
sync:
  conflict_detection: true
recurring: []
"#;
        fs::write(file.path(), yaml).expect("write yaml");

        let error = load_config(file.path()).expect_err("label on non-Jira should fail");
        match error {
            RiptaskError::Config(msg) => {
                assert!(
                    msg.contains("repo_project_label") && msg.contains("Jira-only"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected RiptaskError::Config, got {other:?}"),
        }
    }
}
