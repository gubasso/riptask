use crate::domain::issue::{IssueState, Priority};
use crate::services::issue_ids;
use anyhow::{Context, Result};
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
    pub remotes: Vec<RemoteConfig>,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefaultsConfig {
    pub board: String,
    pub state: IssueState,
    pub priority: Priority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemoteType {
    Github,
    Gitlab,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub remote_type: RemoteType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_board: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardConfig {
    pub name: String,
    pub states: Vec<IssueState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opener: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fzf_opts: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub features: AiFeatures,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AiFeatures {
    #[serde(default)]
    pub new_body_gen: bool,
    #[serde(default)]
    pub triage: bool,
    #[serde(default)]
    pub summarize: bool,
    #[serde(default)]
    pub ask: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SyncConfig {
    #[serde(default = "default_true")]
    pub conflict_detection: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecurringDef {
    pub id: String,
    pub template: String,
    pub title_pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<IssueState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub frequency: RecurrenceFrequency,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_of_week: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_of_month: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
}

pub fn default_config() -> Config {
    Config {
        version: 1,
        auto_commit: false,
        defaults: DefaultsConfig {
            board: "personal".into(),
            state: IssueState::Todo,
            priority: Priority::Medium,
            assignee: None,
            template: Some("task".into()),
        },
        remotes: Vec::new(),
        boards: vec![BoardConfig {
            name: "personal".into(),
            states: vec![
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
            model: Some("claude-haiku-4-5-20251001".into()),
            features: AiFeatures {
                new_body_gen: true,
                triage: true,
                summarize: true,
                ask: true,
            },
        },
        sync: SyncConfig {
            conflict_detection: true,
        },
        recurring: Vec::new(),
    }
}

pub fn load_config(path: &Path) -> Result<Config> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let config: Config = serde_yaml_ng::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

pub fn save_config(path: &Path, config: &Config) -> Result<()> {
    validate_config(config)?;
    let content = serde_yaml_ng::to_string(config).context("failed to serialize config")?;
    let dir = path.parent().context("config path has no parent")?;
    let mut file =
        tempfile::NamedTempFile::new_in(dir).context("failed to create temp config file")?;
    use std::io::Write;
    file.write_all(content.as_bytes())
        .context("failed to write temp config file")?;
    file.persist(path)
        .map_err(|error| anyhow::Error::from(error.error))
        .with_context(|| format!("failed to persist config to {}", path.display()))?;
    Ok(())
}

pub fn validate_config(config: &Config) -> Result<()> {
    issue_ids::validate_no_scope_collisions(&config.remotes)?;
    require_unique(
        config.remotes.iter().map(|remote| remote.name.as_str()),
        "duplicate remote name in riptsk.yaml",
    )?;
    require_unique(
        config.boards.iter().map(|board| board.name.as_str()),
        "duplicate board name in riptsk.yaml",
    )?;
    Ok(())
}

fn require_unique<'a>(iter: impl Iterator<Item = &'a str>, message: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for item in iter {
        if !seen.insert(item.to_owned()) {
            return Err(anyhow::anyhow!(message.to_owned()));
        }
    }
    Ok(())
}

pub fn config_set(config: &mut Config, key: &str, value: &str) -> Result<()> {
    match key {
        "auto_commit" => {
            config.auto_commit = value.parse().context("auto_commit must be true or false")?
        }
        "defaults.board" => config.defaults.board = value.to_owned(),
        "defaults.state" => config.defaults.state = parse_state(value)?,
        "defaults.priority" => config.defaults.priority = parse_priority(value)?,
        "defaults.assignee" => config.defaults.assignee = some_if_not_empty(value),
        "defaults.template" => config.defaults.template = some_if_not_empty(value),
        "ui.opener" => config.ui.opener = some_if_not_empty(value),
        "ui.tree_depth" => {
            config.ui.tree_depth = if value.is_empty() {
                None
            } else {
                Some(value.parse().context("ui.tree_depth must be a number")?)
            }
        }
        "ui.fzf_opts" => config.ui.fzf_opts = some_if_not_empty(value),
        "ai.enabled" => {
            config.ai.enabled = value.parse().context("ai.enabled must be true or false")?
        }
        "ai.model" => config.ai.model = some_if_not_empty(value),
        "sync.conflict_detection" => {
            config.sync.conflict_detection = value
                .parse()
                .context("sync.conflict_detection must be true or false")?
        }
        _ => return Err(anyhow::anyhow!("unsupported config key: {key}")),
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

pub fn parse_state(value: &str) -> Result<IssueState> {
    match value {
        "backlog" => Ok(IssueState::Backlog),
        "todo" => Ok(IssueState::Todo),
        "in-progress" => Ok(IssueState::InProgress),
        "review" => Ok(IssueState::Review),
        "done" => Ok(IssueState::Done),
        _ => Err(anyhow::anyhow!("invalid state: {value}")),
    }
}

pub fn parse_priority(value: &str) -> Result<Priority> {
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
    use tempfile::NamedTempFile;

    #[test]
    fn parses_fixture_config() {
        let config = load_config(std::path::Path::new("tests/fixtures/riptsk.yaml"))
            .expect("load fixture config");
        assert_eq!(config.version, 1);
        assert_eq!(config.remotes.len(), 1);
    }

    #[test]
    fn config_set_whitelist_updates_expected_field() {
        let mut config =
            load_config(std::path::Path::new("tests/fixtures/riptsk.yaml")).expect("load config");
        config_set(&mut config, "ui.opener", "less").expect("set opener");
        assert_eq!(config.ui.opener.as_deref(), Some("less"));
        config_set(&mut config, "auto_commit", "true").expect("set auto_commit");
        assert!(config.auto_commit);
    }

    #[test]
    fn config_save_round_trip_is_valid_yaml() {
        let config =
            load_config(std::path::Path::new("tests/fixtures/riptsk.yaml")).expect("load config");
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
}
