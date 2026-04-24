use crate::assets::templates::embedded_templates;
use crate::config::{default_config, save_config};
use crate::error::RiptaskError;
use crate::paths::AppPaths;
use anyhow::Context;
use std::fs;

pub fn run(paths: &AppPaths) -> Result<(), RiptaskError> {
    if paths.config_path().exists() {
        return Err(RiptaskError::General(format!(
            "repository already initialized at {}",
            paths.riptask_repo
        )));
    }

    paths.ensure_repo_dirs().map_err(RiptaskError::Other)?;
    save_config(paths.config_path().as_std_path(), &default_config())?;

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

    // Initialize git repo
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

    crate::ui::success(&format!(
        "Initialized riptask repository at {}",
        paths.riptask_repo
    ));
    Ok(())
}
