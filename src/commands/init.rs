use crate::assets::templates::embedded_templates;
use crate::cli::InitArgs;
use crate::config::{ConfigScope, PartialConfig, default_config, save_layer, scaffold_header};
use crate::error::RiptaskError;
use crate::paths::AppPaths;
use anyhow::Context;
use camino::{Utf8Path, Utf8PathBuf};
use std::fs;
use std::io::IsTerminal;

pub struct LayerInventory {
    pub system: LayerProbe,
    pub user: LayerProbe,
    pub local: LayerProbe,
}

pub struct LayerProbe {
    pub scope: ConfigScope,
    pub path: Option<Utf8PathBuf>,
    pub status: LayerStatus,
}

pub enum LayerStatus {
    Missing,
    Empty,
    Partial { sections: Vec<&'static str> },
    Complete,
    Invalid { error: String },
    NoProjectRoot,
}

pub fn run(paths: &AppPaths, args: InitArgs) -> Result<(), RiptaskError> {
    let cwd = crate::paths::current_cwd();
    let inventory = probe_inventory(paths, &cwd);
    print_inventory(&inventory);

    let scope = match args.scope.scope() {
        Some(scope) => scope,
        None if !std::io::stdin().is_terminal() => {
            return Err(RiptaskError::General(
                "tsk init requires one of --system|--user|--local in non-interactive contexts."
                    .into(),
            ));
        }
        None => match prompt_scope()? {
            Some(scope) => scope,
            None => return Ok(()),
        },
    };

    initialize_scope(paths, &cwd, &inventory, scope, args.force)
}

fn initialize_scope(
    paths: &AppPaths,
    cwd: &Utf8Path,
    inventory: &LayerInventory,
    scope: ConfigScope,
    force: bool,
) -> Result<(), RiptaskError> {
    let chosen = inventory.probe(scope);
    if let LayerStatus::Invalid { error } = &chosen.status {
        return Err(RiptaskError::Config(format!(
            "{} config at {} is invalid: {error}. Fix or remove it before initializing.",
            scope.label(),
            chosen
                .path
                .as_ref()
                .map(|path| path.as_str())
                .unwrap_or("(no path)")
        )));
    }
    if matches!(chosen.status, LayerStatus::Complete) && !force {
        return Err(RiptaskError::General(format!(
            "{} config is already complete at {}. Use --force to overwrite.",
            scope.label(),
            chosen
                .path
                .as_ref()
                .map(|path| path.as_str())
                .unwrap_or("(no path)")
        )));
    }

    for probe in inventory.probes() {
        if probe.scope != scope
            && matches!(probe.status, LayerStatus::Complete)
            && let Some(path) = &probe.path
        {
            crate::ui::warn(&format!(
                "note: a complete config also exists at {} ({}). Both layers will be merged on load; higher precedence wins for overlapping keys. precedence (low to high): system < user < local.",
                probe.scope.label(),
                path
            ));
        }
    }

    let path = init_path(paths, cwd, scope)?;
    match scope {
        ConfigScope::System => init_system(paths, &path)?,
        ConfigScope::User | ConfigScope::Local => init_partial(scope, &path)?,
    }

    crate::ui::success(&format!("initialized {} at {}", scope.label(), path));
    match scope {
        ConfigScope::System => {
            crate::ui::success(&format!("repo dir: {}", paths.riptask_repo));
            crate::ui::success(&format!("templates dir: {}", paths.templates_dir()));
            crate::ui::success(&format!(
                "gitignore: {}",
                paths.riptask_repo.join(".gitignore")
            ));
            crate::ui::success("git init: ok");
        }
        ConfigScope::User => {
            crate::ui::info("register the current project with: tsk register --user");
        }
        ConfigScope::Local => {
            crate::ui::info("register the current project with: tsk register --local");
        }
    }
    Ok(())
}

fn init_path(
    paths: &AppPaths,
    cwd: &Utf8Path,
    scope: ConfigScope,
) -> Result<Utf8PathBuf, RiptaskError> {
    match scope {
        ConfigScope::System => Ok(paths.system_config_path()),
        ConfigScope::User => Ok(paths.user_config_path()),
        ConfigScope::Local => paths.local_config_path(cwd).or_else(|| {
            if cwd.as_std_path().is_dir() {
                Some(cwd.join(".riptask").join("config.yaml"))
            } else {
                None
            }
        }).ok_or_else(|| {
            RiptaskError::Config(
                "cannot initialize local config because current directory is not a project root or directory".into(),
            )
        }),
    }
}

fn init_system(paths: &AppPaths, path: &Utf8Path) -> Result<(), RiptaskError> {
    paths.ensure_repo_dirs().map_err(RiptaskError::Other)?;
    save_layer(path.as_std_path(), &PartialConfig::from(default_config()))?;

    for (name, content) in embedded_templates() {
        fs::write(paths.templates_dir().join(name), content)
            .with_context(|| format!("failed to write template {name}"))
            .map_err(RiptaskError::Other)?;
    }

    fs::write(
        paths.riptask_repo.join(".gitignore"),
        "# Cache and derived state live outside $RIPTASK_REPO by design.\n",
    )
    .context("failed to write .gitignore")
    .map_err(RiptaskError::Other)?;

    let output = std::process::Command::new("git")
        .arg("init")
        .arg(paths.riptask_repo.as_str())
        .output()
        .context("failed to run git init")
        .map_err(RiptaskError::Other)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let message = if stderr.is_empty() {
            "git init failed".to_owned()
        } else {
            format!("git init failed: {stderr}")
        };
        return Err(RiptaskError::General(message));
    }
    Ok(())
}

fn init_partial(scope: ConfigScope, path: &Utf8Path) -> Result<(), RiptaskError> {
    let dir = path
        .parent()
        .ok_or_else(|| RiptaskError::Config("config path has no parent".into()))?;
    fs::create_dir_all(dir)?;
    fs::write(path, scaffold_header(scope))?;
    Ok(())
}

fn prompt_scope() -> Result<Option<ConfigScope>, RiptaskError> {
    let choices = ["system", "user", "local", "abort"];
    let selection = dialoguer::Select::new()
        .with_prompt("Where do you want to initialize tsk?")
        .items(choices)
        .default(3)
        .interact()
        .map_err(|error| RiptaskError::Other(anyhow::Error::new(error)))?;
    Ok(match choices[selection] {
        "system" => Some(ConfigScope::System),
        "user" => Some(ConfigScope::User),
        "local" => Some(ConfigScope::Local),
        _ => None,
    })
}

fn probe_inventory(paths: &AppPaths, cwd: &Utf8Path) -> LayerInventory {
    LayerInventory {
        system: probe_scope(paths, cwd, ConfigScope::System),
        user: probe_scope(paths, cwd, ConfigScope::User),
        local: probe_scope(paths, cwd, ConfigScope::Local),
    }
}

fn probe_scope(paths: &AppPaths, cwd: &Utf8Path, scope: ConfigScope) -> LayerProbe {
    let path = match scope.resolve(paths, cwd) {
        Ok(path) => path,
        Err(_) if scope == ConfigScope::Local => {
            return LayerProbe {
                scope,
                path: None,
                status: LayerStatus::NoProjectRoot,
            };
        }
        Err(error) => {
            return LayerProbe {
                scope,
                path: None,
                status: LayerStatus::Invalid {
                    error: error.to_string(),
                },
            };
        }
    };

    let status = match fs::read_to_string(&path) {
        Ok(content) => match serde_yaml_ng::from_str::<Option<PartialConfig>>(&content) {
            Ok(partial) => classify_partial(partial.unwrap_or_default()),
            Err(error) => LayerStatus::Invalid {
                error: error.to_string(),
            },
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LayerStatus::Missing,
        Err(error) => LayerStatus::Invalid {
            error: error.to_string(),
        },
    };

    LayerProbe {
        scope,
        path: Some(path),
        status,
    }
}

fn classify_partial(partial: PartialConfig) -> LayerStatus {
    let mut sections = Vec::new();
    if partial.defaults.is_some() {
        sections.push("defaults");
    }
    if !partial.boards.is_empty() {
        sections.push("boards");
    }
    if partial.ui.is_some() {
        sections.push("ui");
    }
    if partial.ai.is_some() {
        sections.push("ai");
    }
    if partial.sync.is_some() {
        sections.push("sync");
    }

    if sections.is_empty()
        && partial.auto_commit.is_none()
        && partial.projects.is_empty()
        && partial.recurring.is_empty()
    {
        LayerStatus::Empty
    } else if sections.len() == 5 {
        LayerStatus::Complete
    } else {
        LayerStatus::Partial { sections }
    }
}

fn print_inventory(inventory: &LayerInventory) {
    println!("tsk config inventory:");
    for probe in inventory.probes() {
        match &probe.path {
            Some(path) => println!(
                "  {:<7} {:<18} {}",
                probe.scope.label(),
                probe.status.label(),
                path
            ),
            None => println!(
                "  {:<7} {:<18} no project root",
                probe.scope.label(),
                probe.status.label()
            ),
        }
    }
}

impl LayerInventory {
    fn probe(&self, scope: ConfigScope) -> &LayerProbe {
        match scope {
            ConfigScope::System => &self.system,
            ConfigScope::User => &self.user,
            ConfigScope::Local => &self.local,
        }
    }

    fn probes(&self) -> [&LayerProbe; 3] {
        [&self.system, &self.user, &self.local]
    }
}

impl LayerStatus {
    fn label(&self) -> String {
        match self {
            LayerStatus::Missing => "Missing".into(),
            LayerStatus::Empty => "Empty".into(),
            LayerStatus::Partial { sections } => {
                if sections.is_empty() {
                    "Partial".into()
                } else {
                    format!("Partial({})", sections.join(","))
                }
            }
            LayerStatus::Complete => "Complete".into(),
            LayerStatus::Invalid { .. } => "Invalid".into(),
            LayerStatus::NoProjectRoot => "NoProjectRoot".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LayerStatus, classify_partial};
    use crate::config::{PartialAi, PartialConfig, PartialDefaults, PartialSync, PartialUi};
    use crate::models::BoardConfig;

    #[test]
    fn classify_empty_partial() {
        assert!(matches!(
            classify_partial(PartialConfig::default()),
            LayerStatus::Empty
        ));
    }

    #[test]
    fn classify_complete_partial_ignores_empty_projects_and_recurring() {
        let status = classify_partial(PartialConfig {
            defaults: Some(PartialDefaults::default()),
            boards: vec![BoardConfig {
                name: "personal".into(),
                statuses: Vec::new(),
            }],
            ui: Some(PartialUi::default()),
            ai: Some(PartialAi::default()),
            sync: Some(PartialSync::default()),
            ..Default::default()
        });
        assert!(matches!(status, LayerStatus::Complete));
    }

    #[test]
    fn classify_partial_sections() {
        let status = classify_partial(PartialConfig {
            defaults: Some(PartialDefaults::default()),
            ..Default::default()
        });
        assert!(matches!(status, LayerStatus::Partial { .. }));
    }
}
