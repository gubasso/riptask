use crate::assets::templates::embedded_templates;
use crate::config::{default_config, save_config};
use crate::error::TskError;
use crate::paths::AppPaths;
use anyhow::Context;
use std::fs;

pub fn run(paths: &AppPaths) -> Result<(), TskError> {
    if paths.config_path().exists() {
        return Err(TskError::General(format!(
            "repository already initialized at {}",
            paths.tsk_repo
        )));
    }

    paths.ensure_repo_dirs().map_err(TskError::Other)?;
    save_config(paths.config_path().as_std_path(), &default_config()).map_err(TskError::Other)?;

    for (name, content) in embedded_templates() {
        fs::write(paths.templates_dir().join(name), content)
            .with_context(|| format!("failed to write template {name}"))
            .map_err(TskError::Other)?;
    }

    fs::write(
        paths.tsk_repo.join(".gitignore"),
        "# Cache and derived state live outside $TSK_REPO by design.\n",
    )
    .context("failed to write .gitignore")
    .map_err(TskError::Other)?;

    // Initialize git repo
    let output = std::process::Command::new("git")
        .arg("init")
        .arg(paths.tsk_repo.as_str())
        .output()
        .context("failed to run git init")
        .map_err(TskError::Other)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let message = if stderr.is_empty() {
            "git init failed".to_owned()
        } else {
            format!("git init failed: {stderr}")
        };
        return Err(TskError::General(message));
    }

    println!("Initialized tsk repository at {}", paths.tsk_repo);
    Ok(())
}
