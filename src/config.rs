use crate::domain::issue::{IssueState, Priority};
use crate::error::RiptaskError;
use crate::models::{
    AiConfig, AiFeatures, BackendKind, BoardConfig, DefaultsConfig, RecurringDef, RepoProject,
    SyncConfig, UiConfig,
};
use crate::paths::AppPaths;
use crate::services::{issue_ids, repo_project_label};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    System,
    User,
    Local,
}

#[derive(Debug, Clone)]
pub struct ConfigWriteReceipt {
    pub scope: ConfigScope,
    pub path: Utf8PathBuf,
}

impl ConfigScope {
    pub fn resolve(self, paths: &AppPaths, cwd: &Utf8Path) -> Result<Utf8PathBuf, RiptaskError> {
        match self {
            ConfigScope::System => Ok(paths.system_config_path()),
            ConfigScope::User => Ok(paths.user_config_path()),
            ConfigScope::Local => paths.local_config_path(cwd).ok_or_else(|| {
                RiptaskError::Config("not inside a riptask project; use --system or --user".into())
            }),
        }
    }

    pub fn exists(self, paths: &AppPaths, cwd: &Utf8Path) -> bool {
        self.resolve(paths, cwd)
            .map(|path| path.exists())
            .unwrap_or(false)
    }

    pub fn label(self) -> &'static str {
        match self {
            ConfigScope::System => "system",
            ConfigScope::User => "user",
            ConfigScope::Local => "local",
        }
    }
}

pub fn scaffold_header(scope: ConfigScope) -> String {
    format!("# riptask config (scope: {})\n", scope.label())
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartialConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_commit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<PartialDefaults>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<RepoProject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boards: Vec<BoardConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<PartialUi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai: Option<PartialAi>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<PartialSync>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recurring: Vec<RecurringDef>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartialDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<IssueState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_option_value"
    )]
    pub assignee: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_option_value"
    )]
    pub template: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartialUi {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_option_value"
    )]
    pub opener: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_option_value"
    )]
    pub tree_depth: Option<Option<u32>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_option_value"
    )]
    pub fzf_opts: Option<Option<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartialAi {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_option_value"
    )]
    pub command: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<PartialAiFeatures>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartialAiFeatures {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_body_gen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summarize: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PartialSync {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_detection: Option<bool>,
}

impl PartialConfig {
    pub fn merge(lower: Self, higher: Self) -> Self {
        Self {
            auto_commit: higher.auto_commit.or(lower.auto_commit),
            defaults: merge_option_with(lower.defaults, higher.defaults, PartialDefaults::merge),
            projects: merge_vec(lower.projects, higher.projects, |project| {
                project.name.clone()
            }),
            boards: merge_vec(lower.boards, higher.boards, |board| board.name.clone()),
            ui: merge_option_with(lower.ui, higher.ui, PartialUi::merge),
            ai: merge_option_with(lower.ai, higher.ai, PartialAi::merge),
            sync: merge_option_with(lower.sync, higher.sync, PartialSync::merge),
            recurring: merge_vec(lower.recurring, higher.recurring, |recurring| {
                recurring.id.clone()
            }),
        }
    }

    pub fn finalize(self) -> Result<Config, RiptaskError> {
        let defaults = default_config();
        let partial_defaults = self.defaults.unwrap_or_default();
        let partial_ui = self.ui.unwrap_or_default();
        let partial_ai = self.ai.unwrap_or_default();
        let partial_features = partial_ai.features.unwrap_or_default();
        let partial_sync = self.sync.unwrap_or_default();
        let mut config = Config {
            auto_commit: self.auto_commit.unwrap_or(defaults.auto_commit),
            defaults: DefaultsConfig {
                board: partial_defaults.board.unwrap_or(defaults.defaults.board),
                status: partial_defaults.status.unwrap_or(defaults.defaults.status),
                priority: partial_defaults
                    .priority
                    .unwrap_or(defaults.defaults.priority),
                assignee: partial_defaults
                    .assignee
                    .unwrap_or(defaults.defaults.assignee),
                template: partial_defaults
                    .template
                    .unwrap_or(defaults.defaults.template),
            },
            projects: self.projects,
            boards: if self.boards.is_empty() {
                defaults.boards
            } else {
                self.boards
            },
            ui: UiConfig {
                opener: partial_ui.opener.unwrap_or(defaults.ui.opener),
                tree_depth: partial_ui.tree_depth.unwrap_or(defaults.ui.tree_depth),
                fzf_opts: partial_ui.fzf_opts.unwrap_or(defaults.ui.fzf_opts),
            },
            ai: AiConfig {
                enabled: partial_ai.enabled.unwrap_or(defaults.ai.enabled),
                command: partial_ai.command.unwrap_or(defaults.ai.command),
                features: AiFeatures {
                    new_body_gen: partial_features
                        .new_body_gen
                        .unwrap_or(defaults.ai.features.new_body_gen),
                    triage: partial_features
                        .triage
                        .unwrap_or(defaults.ai.features.triage),
                    summarize: partial_features
                        .summarize
                        .unwrap_or(defaults.ai.features.summarize),
                    ask: partial_features.ask.unwrap_or(defaults.ai.features.ask),
                    commit: partial_features
                        .commit
                        .unwrap_or(defaults.ai.features.commit),
                },
            },
            sync: SyncConfig {
                conflict_detection: partial_sync
                    .conflict_detection
                    .unwrap_or(defaults.sync.conflict_detection),
            },
            recurring: self.recurring,
        };
        validate_config(&mut config)?;
        Ok(config)
    }
}

impl From<Config> for PartialConfig {
    fn from(config: Config) -> Self {
        Self {
            auto_commit: Some(config.auto_commit),
            defaults: Some(PartialDefaults {
                board: Some(config.defaults.board),
                status: Some(config.defaults.status),
                priority: Some(config.defaults.priority),
                assignee: config.defaults.assignee.map(Some),
                template: config.defaults.template.map(Some),
            }),
            projects: config.projects,
            boards: config.boards,
            ui: Some(PartialUi {
                opener: config.ui.opener.map(Some),
                tree_depth: config.ui.tree_depth.map(Some),
                fzf_opts: config.ui.fzf_opts.map(Some),
            }),
            ai: Some(PartialAi {
                enabled: Some(config.ai.enabled),
                command: config.ai.command.map(Some),
                features: Some(PartialAiFeatures {
                    new_body_gen: Some(config.ai.features.new_body_gen),
                    triage: Some(config.ai.features.triage),
                    summarize: Some(config.ai.features.summarize),
                    ask: Some(config.ai.features.ask),
                    commit: Some(config.ai.features.commit),
                }),
            }),
            sync: Some(PartialSync {
                conflict_detection: Some(config.sync.conflict_detection),
            }),
            recurring: config.recurring,
        }
    }
}

impl PartialDefaults {
    fn merge(lower: Self, higher: Self) -> Self {
        Self {
            board: higher.board.or(lower.board),
            status: higher.status.or(lower.status),
            priority: higher.priority.or(lower.priority),
            assignee: higher.assignee.or(lower.assignee),
            template: higher.template.or(lower.template),
        }
    }
}

impl PartialUi {
    fn merge(lower: Self, higher: Self) -> Self {
        Self {
            opener: higher.opener.or(lower.opener),
            tree_depth: higher.tree_depth.or(lower.tree_depth),
            fzf_opts: higher.fzf_opts.or(lower.fzf_opts),
        }
    }
}

impl PartialAi {
    fn merge(lower: Self, higher: Self) -> Self {
        Self {
            enabled: higher.enabled.or(lower.enabled),
            command: higher.command.or(lower.command),
            features: merge_option_with(lower.features, higher.features, PartialAiFeatures::merge),
        }
    }
}

impl PartialAiFeatures {
    fn merge(lower: Self, higher: Self) -> Self {
        Self {
            new_body_gen: higher.new_body_gen.or(lower.new_body_gen),
            triage: higher.triage.or(lower.triage),
            summarize: higher.summarize.or(lower.summarize),
            ask: higher.ask.or(lower.ask),
            commit: higher.commit.or(lower.commit),
        }
    }
}

impl PartialSync {
    fn merge(lower: Self, higher: Self) -> Self {
        Self {
            conflict_detection: higher.conflict_detection.or(lower.conflict_detection),
        }
    }
}

fn merge_option_with<T>(lower: Option<T>, higher: Option<T>, merge: fn(T, T) -> T) -> Option<T> {
    match (lower, higher) {
        (Some(lower), Some(higher)) => Some(merge(lower, higher)),
        (lower, None) => lower,
        (None, higher) => higher,
    }
}

fn merge_vec<T, K, F>(lower: Vec<T>, higher: Vec<T>, key: F) -> Vec<T>
where
    K: Eq + std::hash::Hash,
    F: Fn(&T) -> K,
{
    let combined: Vec<T> = lower.into_iter().chain(higher).collect();
    let mut seen = HashSet::new();
    let mut reversed: Vec<T> = combined
        .into_iter()
        .rev()
        .filter(|item| seen.insert(key(item)))
        .collect();
    reversed.reverse();
    reversed
}

fn deserialize_option_value<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

pub fn default_config() -> Config {
    Config {
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

pub fn load_layer(path: &Path) -> Result<Option<PartialConfig>, RiptaskError> {
    match fs::read_to_string(path) {
        Ok(content) => {
            let partial: Option<PartialConfig> =
                serde_yaml_ng::from_str(&content).map_err(|error| {
                    RiptaskError::Config(format!("failed to parse {}: {error}", path.display()))
                })?;
            Ok(Some(partial.unwrap_or_default()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RiptaskError::Io(error)),
    }
}

pub fn load_effective_config(paths: &AppPaths, cwd: &Utf8Path) -> Result<Config, RiptaskError> {
    let mut merged = PartialConfig::default();
    for path in [paths.system_config_path(), paths.user_config_path()]
        .into_iter()
        .chain(paths.local_config_path(cwd))
    {
        if let Some(layer) = load_layer(path.as_std_path())? {
            merged = PartialConfig::merge(merged, layer);
        }
    }
    merged.finalize()
}

pub fn save_layer(path: &Path, partial: &PartialConfig) -> Result<(), RiptaskError> {
    save_layer_with_header(path, partial, None)
}

fn save_layer_with_header(
    path: &Path,
    partial: &PartialConfig,
    header: Option<&str>,
) -> Result<(), RiptaskError> {
    let body = serde_yaml_ng::to_string(partial)
        .map_err(|error| RiptaskError::Config(format!("failed to serialize config: {error}")))?;
    let content = match header {
        Some(header) => format!("{header}{body}"),
        None => body,
    };
    let dir = path
        .parent()
        .ok_or_else(|| RiptaskError::Config("config path has no parent".into()))?;
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    use std::io::Write;
    file.write_all(content.as_bytes())?;
    file.persist(path)
        .map_err(|error| RiptaskError::Io(error.error))?;
    Ok(())
}

pub fn config_set_scoped(
    paths: &AppPaths,
    cwd: &Utf8Path,
    scope: ConfigScope,
    key: &str,
    value: &str,
) -> Result<ConfigWriteReceipt, RiptaskError> {
    config_mutate_scoped(paths, cwd, scope, |partial| {
        partial_config_set(partial, key, value)
    })
}

pub fn config_mutate_scoped<F>(
    paths: &AppPaths,
    cwd: &Utf8Path,
    scope: ConfigScope,
    mutate: F,
) -> Result<ConfigWriteReceipt, RiptaskError>
where
    F: FnOnce(&mut PartialConfig) -> Result<(), RiptaskError>,
{
    let target = scope.resolve(paths, cwd)?;
    let creating = !target.exists();
    // Snapshot the existing file (if any) so we can roll back if validation fails after write.
    let prior_contents = if creating {
        None
    } else {
        Some(std::fs::read(target.as_std_path()).map_err(|error| {
            RiptaskError::Config(format!("failed to read {}: {}", target, error))
        })?)
    };
    let mut partial = load_layer(target.as_std_path())?.unwrap_or_default();
    mutate(&mut partial)?;
    let header = if creating {
        Some(scaffold_header(scope))
    } else {
        None
    };
    save_layer_with_header(target.as_std_path(), &partial, header.as_deref())?;
    if let Err(error) = load_effective_config(paths, cwd) {
        // Roll back the write so the on-disk config is not left in a broken state.
        let rollback = match &prior_contents {
            Some(bytes) => std::fs::write(target.as_std_path(), bytes),
            None => std::fs::remove_file(target.as_std_path()),
        };
        let rollback_note = match rollback {
            Ok(()) => String::new(),
            Err(rollback_error) => format!(
                " (rollback also failed: {}; file may be left in an inconsistent state)",
                rollback_error
            ),
        };
        return Err(RiptaskError::Config(format!(
            "validation failed after writing {}: {}{}",
            target, error, rollback_note
        )));
    }
    Ok(ConfigWriteReceipt {
        scope,
        path: target,
    })
}

pub fn default_write_scope(paths: &AppPaths, cwd: &Utf8Path) -> ConfigScope {
    if paths.local_config_path(cwd).is_some() {
        ConfigScope::Local
    } else {
        ConfigScope::User
    }
}

pub fn any_config_exists(paths: &AppPaths, cwd: &Utf8Path) -> bool {
    ConfigScope::System.exists(paths, cwd)
        || ConfigScope::User.exists(paths, cwd)
        || ConfigScope::Local.exists(paths, cwd)
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
        "duplicate RepoProject name in config.yaml",
    )?;
    require_unique(
        config.boards.iter().map(|board| board.name.as_str()),
        "duplicate board name in config.yaml",
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

fn partial_config_set(
    config: &mut PartialConfig,
    key: &str,
    value: &str,
) -> Result<(), RiptaskError> {
    match key {
        "auto_commit" => {
            config.auto_commit =
                Some(value.parse().map_err(|_| {
                    RiptaskError::Config("auto_commit must be true or false".into())
                })?)
        }
        "defaults.board" => {
            config.defaults.get_or_insert_with(Default::default).board = Some(value.to_owned())
        }
        "defaults.status" => {
            config.defaults.get_or_insert_with(Default::default).status =
                Some(parse_state(value).map_err(|error| RiptaskError::Config(error.to_string()))?)
        }
        "defaults.priority" => {
            config
                .defaults
                .get_or_insert_with(Default::default)
                .priority = Some(
                parse_priority(value).map_err(|error| RiptaskError::Config(error.to_string()))?,
            )
        }
        "defaults.assignee" => {
            config
                .defaults
                .get_or_insert_with(Default::default)
                .assignee = Some(some_if_not_empty(value))
        }
        "defaults.template" => {
            config
                .defaults
                .get_or_insert_with(Default::default)
                .template = Some(some_if_not_empty(value))
        }
        "ui.opener" => {
            config.ui.get_or_insert_with(Default::default).opener = Some(some_if_not_empty(value))
        }
        "ui.tree_depth" => {
            config.ui.get_or_insert_with(Default::default).tree_depth =
                Some(if value.is_empty() {
                    None
                } else {
                    Some(value.parse().map_err(|_| {
                        RiptaskError::Config("ui.tree_depth must be a number".into())
                    })?)
                })
        }
        "ui.fzf_opts" => {
            config.ui.get_or_insert_with(Default::default).fzf_opts = Some(some_if_not_empty(value))
        }
        "ai.enabled" => {
            config.ai.get_or_insert_with(Default::default).enabled = Some(
                value
                    .parse()
                    .map_err(|_| RiptaskError::Config("ai.enabled must be true or false".into()))?,
            )
        }
        "ai.command" => {
            config.ai.get_or_insert_with(Default::default).command = Some(some_if_not_empty(value))
        }
        "sync.conflict_detection" => {
            config
                .sync
                .get_or_insert_with(Default::default)
                .conflict_detection = Some(value.parse().map_err(|_| {
                RiptaskError::Config("sync.conflict_detection must be true or false".into())
            })?)
        }
        _ => {
            return Err(RiptaskError::Config(format!(
                "unsupported config key: {key}"
            )));
        }
    }
    Ok(())
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
    use super::{
        ConfigScope, PartialConfig, PartialUi, config_mutate_scoped, load_effective_config,
        load_layer, parse_priority, parse_state, save_layer,
    };
    use crate::error::RiptaskError;
    use crate::models::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};
    use crate::paths::AppPaths;
    use std::fs;
    use std::path::Path;
    use tempfile::{NamedTempFile, tempdir};

    #[test]
    fn parses_fixture_config() {
        let config = load_layer(Path::new("tests/fixtures/config.yaml"))
            .expect("load layer")
            .expect("layer")
            .finalize()
            .expect("finalize fixture config");
        assert_eq!(config.projects.len(), 1);
    }

    #[test]
    fn merge_scalars_higher_wins() {
        let lower = PartialConfig {
            auto_commit: Some(false),
            ..Default::default()
        };
        let higher = PartialConfig {
            auto_commit: Some(true),
            ..Default::default()
        };
        let merged = PartialConfig::merge(lower, higher);
        assert_eq!(merged.auto_commit, Some(true));
    }

    #[test]
    fn merge_option_none_preserves_lower() {
        let lower = PartialConfig {
            ui: Some(PartialUi {
                opener: Some(Some("vim".into())),
                ..Default::default()
            }),
            ..Default::default()
        };
        let higher = PartialConfig {
            ui: Some(PartialUi::default()),
            ..Default::default()
        };
        let merged = PartialConfig::merge(lower, higher);
        assert_eq!(merged.ui.unwrap().opener, Some(Some("vim".into())));
    }

    #[test]
    fn merge_vec_concat_dedupe_higher_wins() {
        let lower_project = project("foo", Some("ALPHA"));
        let higher_project = project("foo", Some("BETA"));
        let merged = PartialConfig::merge(
            PartialConfig {
                projects: vec![lower_project],
                ..Default::default()
            },
            PartialConfig {
                projects: vec![higher_project],
                ..Default::default()
            },
        );
        assert_eq!(merged.projects.len(), 1);
        assert_eq!(merged.projects[0].key.as_deref(), Some("BETA"));
    }

    #[test]
    fn merge_empty_vecs() {
        let merged = PartialConfig::merge(PartialConfig::default(), PartialConfig::default());
        assert!(merged.projects.is_empty());
        assert!(merged.boards.is_empty());
        assert!(merged.recurring.is_empty());
    }

    #[test]
    fn finalize_runs_validate_config() {
        let config = PartialConfig {
            projects: vec![project("demo", None)],
            ..Default::default()
        }
        .finalize()
        .expect("finalize");
        assert_eq!(config.projects[0].key.as_deref(), Some("DEMO"));
    }

    #[test]
    fn finalize_rejects_invalid_merged_result() {
        let error = PartialConfig {
            projects: vec![project("one", Some("DUP")), project("two", Some("DUP"))],
            ..Default::default()
        }
        .finalize()
        .expect_err("collision");
        assert!(matches!(error, RiptaskError::KeyCollision(_)));
    }

    #[test]
    fn load_layer_rejects_unknown_field_with_file_in_message() {
        let file = NamedTempFile::new().expect("temp file");
        fs::write(file.path(), "unknown: true\n").expect("write yaml");
        let error = load_layer(file.path()).expect_err("unknown field");
        match error {
            RiptaskError::Config(message) => {
                assert!(message.contains(&file.path().display().to_string()));
                assert!(message.contains("unknown"));
            }
            other => panic!("expected config error, got {other:?}"),
        }
    }

    #[test]
    fn save_layer_round_trip_is_valid_yaml() {
        let config = load_layer(Path::new("tests/fixtures/config.yaml"))
            .expect("load layer")
            .expect("layer")
            .finalize()
            .expect("finalize");
        let file = NamedTempFile::new().expect("temp file");
        save_layer(file.path(), &PartialConfig::from(config)).expect("save layer");
        let reparsed = load_layer(file.path())
            .expect("reload layer")
            .expect("layer")
            .finalize()
            .expect("finalize");
        assert_eq!(reparsed.projects.len(), 1);
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
    fn load_effective_config_normalizes_legacy_key_without_rewriting_file() {
        let temp = tempdir().expect("temp dir");
        let system = temp.path().join("repo");
        let user = temp.path().join("user");
        fs::create_dir_all(&system).expect("system dir");
        let config_path = system.join("config.yaml");
        let yaml = r#"defaults:
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
        fs::write(&config_path, yaml).expect("write yaml");
        let before = fs::read_to_string(&config_path).expect("before");
        let paths = AppPaths {
            riptask_repo: system.to_string_lossy().to_string().into(),
            user_config_root: user.to_string_lossy().to_string().into(),
            cache_root: temp
                .path()
                .join("cache")
                .to_string_lossy()
                .to_string()
                .into(),
            state_root: temp
                .path()
                .join("state")
                .to_string_lossy()
                .to_string()
                .into(),
        };

        let config =
            load_effective_config(&paths, camino::Utf8Path::new("/tmp")).expect("load effective");

        assert_eq!(config.projects[0].key.as_deref(), Some("DEMO"));
        let after = fs::read_to_string(&config_path).expect("after");
        assert_eq!(before, after);
    }

    #[test]
    fn load_effective_config_returns_structured_key_collision() {
        let mut alpha_one = project("alpha-one", None);
        alpha_one.vc_backend.repo = Some("owner/alpha".into());
        alpha_one.tasks_backend.repo = Some("owner/alpha".into());
        let mut alpha_two = project("alpha-two", None);
        alpha_two.vc_backend.repo = Some("other/alpha".into());
        alpha_two.tasks_backend.repo = Some("other/alpha".into());
        let error = PartialConfig {
            projects: vec![alpha_one, alpha_two],
            ..Default::default()
        }
        .finalize()
        .expect_err("collision");
        assert!(matches!(error, RiptaskError::KeyCollision(_)));
    }

    #[test]
    fn finalize_rejects_repo_project_label_on_non_jira_backend() {
        let mut repo_project = project("demo", None);
        repo_project.repo_project_label = Some("proj::demo".into());
        let error = PartialConfig {
            projects: vec![repo_project],
            ..Default::default()
        }
        .finalize()
        .expect_err("label on non-Jira should fail");
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

    #[test]
    fn config_mutate_scoped_creates_local_with_header() {
        let temp = tempdir().expect("temp dir");
        let system = temp.path().join("repo");
        let user = temp.path().join("user");
        let project_root = temp.path().join("project");
        fs::create_dir_all(&system).expect("system dir");
        fs::create_dir_all(&user).expect("user dir");
        fs::create_dir_all(project_root.join(".riptask")).expect("local dir marker");
        let paths = AppPaths {
            riptask_repo: system.to_string_lossy().to_string().into(),
            user_config_root: user.to_string_lossy().to_string().into(),
            cache_root: temp
                .path()
                .join("cache")
                .to_string_lossy()
                .to_string()
                .into(),
            state_root: temp
                .path()
                .join("state")
                .to_string_lossy()
                .to_string()
                .into(),
        };
        let cwd = camino::Utf8Path::from_path(&project_root).expect("utf8 cwd");

        let receipt = config_mutate_scoped(&paths, cwd, ConfigScope::Local, |partial| {
            partial.auto_commit = Some(true);
            Ok(())
        })
        .expect("mutate local");

        let content = fs::read_to_string(receipt.path).expect("local config");
        assert!(content.starts_with("# riptask config (scope: local)\n"));
        assert!(content.contains("auto_commit: true"));
    }

    #[test]
    fn config_mutate_scoped_preserves_other_layers() {
        let temp = tempdir().expect("temp dir");
        let system = temp.path().join("repo");
        let user = temp.path().join("user");
        fs::create_dir_all(&system).expect("system dir");
        fs::create_dir_all(&user).expect("user dir");
        let system_config = system.join("config.yaml");
        fs::write(&system_config, "auto_commit: false\n").expect("system config");
        let before = fs::read_to_string(&system_config).expect("before");
        let paths = AppPaths {
            riptask_repo: system.to_string_lossy().to_string().into(),
            user_config_root: user.to_string_lossy().to_string().into(),
            cache_root: temp
                .path()
                .join("cache")
                .to_string_lossy()
                .to_string()
                .into(),
            state_root: temp
                .path()
                .join("state")
                .to_string_lossy()
                .to_string()
                .into(),
        };

        config_mutate_scoped(
            &paths,
            camino::Utf8Path::new("/tmp"),
            ConfigScope::User,
            |partial| {
                partial.auto_commit = Some(true);
                Ok(())
            },
        )
        .expect("mutate user");

        let after = fs::read_to_string(system_config).expect("after");
        assert_eq!(before, after);
    }

    #[test]
    fn config_mutate_scoped_revalidates_after_write() {
        let temp = tempdir().expect("temp dir");
        let system = temp.path().join("repo");
        let user = temp.path().join("user");
        fs::create_dir_all(&system).expect("system dir");
        fs::create_dir_all(&user).expect("user dir");
        let paths = AppPaths {
            riptask_repo: system.to_string_lossy().to_string().into(),
            user_config_root: user.to_string_lossy().to_string().into(),
            cache_root: temp
                .path()
                .join("cache")
                .to_string_lossy()
                .to_string()
                .into(),
            state_root: temp
                .path()
                .join("state")
                .to_string_lossy()
                .to_string()
                .into(),
        };

        let error = config_mutate_scoped(
            &paths,
            camino::Utf8Path::new("/tmp"),
            ConfigScope::System,
            |partial| {
                let mut invalid = project("invalid", None);
                invalid.vc_backend.kind = BackendKind::Jira;
                partial.projects.push(invalid);
                Ok(())
            },
        )
        .expect_err("validation failure");

        assert!(matches!(error, RiptaskError::Config(_)));
        // The newly-created system file must be rolled back since the write failed validation.
        assert!(
            !system.join("config.yaml").exists(),
            "rollback should remove the file created during a failed write"
        );
    }

    #[test]
    fn config_mutate_scoped_rolls_back_existing_file_on_validation_failure() {
        let temp = tempdir().expect("temp dir");
        let system = temp.path().join("repo");
        let user = temp.path().join("user");
        fs::create_dir_all(&system).expect("system dir");
        fs::create_dir_all(&user).expect("user dir");
        let system_config = system.join("config.yaml");
        let original = "# riptask config (scope: system)\nauto_commit: false\n";
        fs::write(&system_config, original).expect("seed system config");
        let paths = AppPaths {
            riptask_repo: system.to_string_lossy().to_string().into(),
            user_config_root: user.to_string_lossy().to_string().into(),
            cache_root: temp
                .path()
                .join("cache")
                .to_string_lossy()
                .to_string()
                .into(),
            state_root: temp
                .path()
                .join("state")
                .to_string_lossy()
                .to_string()
                .into(),
        };

        let error = config_mutate_scoped(
            &paths,
            camino::Utf8Path::new("/tmp"),
            ConfigScope::System,
            |partial| {
                let mut invalid = project("invalid", None);
                invalid.vc_backend.kind = BackendKind::Jira;
                partial.projects.push(invalid);
                Ok(())
            },
        )
        .expect_err("validation failure");

        assert!(matches!(error, RiptaskError::Config(_)));
        let after = fs::read_to_string(&system_config).expect("config still present");
        assert_eq!(
            after, original,
            "rollback should restore the prior file contents byte-for-byte"
        );
    }

    fn project(name: &str, key: Option<&str>) -> RepoProject {
        RepoProject {
            name: name.into(),
            vc_backend: VCBackendSpec {
                kind: BackendKind::Github,
                host: None,
                repo: Some(format!("owner/{name}")),
                path: None,
            },
            tasks_backend: TasksBackendSpec {
                kind: BackendKind::Github,
                host: None,
                repo: Some(format!("owner/{name}")),
                jira_project: None,
                default_issue_type: None,
                path: None,
            },
            default_board: Some("personal".into()),
            default_org: None,
            key: key.map(str::to_owned),
            repo_project_label: None,
        }
    }
}
