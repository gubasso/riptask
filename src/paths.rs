use crate::error::RiptaskError;
use anyhow::{Context, Result};
use camino::{Utf8Path, Utf8PathBuf};
use std::fs;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub riptask_repo: Utf8PathBuf,
    pub user_config_root: Utf8PathBuf,
    pub cache_root: Utf8PathBuf,
    pub state_root: Utf8PathBuf,
}

impl AppPaths {
    pub fn from_env() -> Result<Self> {
        let riptask_repo = resolve_riptask_repo()?;
        let user_config_root = resolve_user_config_root()?;
        let cache_root = resolve_cache_root()?;
        let state_root = resolve_state_root()?;
        Ok(Self {
            riptask_repo,
            user_config_root,
            cache_root,
            state_root,
        })
    }

    pub fn system_config_path(&self) -> Utf8PathBuf {
        self.riptask_repo.join("config.yaml")
    }

    pub fn user_config_path(&self) -> Utf8PathBuf {
        self.user_config_root.join("config.yaml")
    }

    pub fn local_config_path(&self, cwd: &Utf8Path) -> Option<Utf8PathBuf> {
        project_root(cwd).map(|root| root.join(".riptask").join("config.yaml"))
    }

    pub fn issues_dir(&self) -> Utf8PathBuf {
        self.riptask_repo.join("issues")
    }

    pub fn templates_dir(&self) -> Utf8PathBuf {
        self.riptask_repo.join("templates")
    }

    pub fn views_root(&self) -> Utf8PathBuf {
        self.cache_root.join("views")
    }

    pub fn backend_state_path(&self) -> Utf8PathBuf {
        self.cache_root.join("backend_state.json")
    }

    pub fn id_map_path(&self) -> Utf8PathBuf {
        self.cache_root.join("id_map.json")
    }

    pub fn deleted_keys_path(&self) -> Utf8PathBuf {
        self.cache_root.join("deleted_keys.json")
    }

    pub fn session_state_path(&self) -> Utf8PathBuf {
        self.cache_root.join("session.json")
    }

    pub fn log_path(&self) -> Utf8PathBuf {
        self.state_root.join("riptask.log")
    }

    pub fn ensure_repo_dirs(&self) -> Result<()> {
        fs::create_dir_all(self.issues_dir()).context("failed to create issues dir")?;
        fs::create_dir_all(self.templates_dir()).context("failed to create templates dir")?;
        fs::create_dir_all(self.views_root()).context("failed to create views dir")?;
        Ok(())
    }

    pub fn require_initialized(&self, cwd: &Utf8Path) -> Result<(), RiptaskError> {
        if crate::config::any_config_exists(self, cwd) {
            return Ok(());
        }
        Err(RiptaskError::General(
            "tsk is not initialized. Run `tsk init --system|--user|--local` first.".into(),
        ))
    }
}

pub fn current_cwd() -> Utf8PathBuf {
    Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    )
}

fn project_root(cwd: &Utf8Path) -> Option<Utf8PathBuf> {
    if let Ok(Some(top)) = crate::services::project_detection::git_toplevel(cwd) {
        return Some(Utf8PathBuf::from(top));
    }
    cwd.ancestors()
        .find(|ancestor| ancestor.join(".riptask").is_dir())
        .map(Utf8Path::to_path_buf)
}

pub fn resolve_user_config_root() -> Result<Utf8PathBuf> {
    let xdg_dirs = xdg::BaseDirectories::new();
    let config_home = xdg_dirs
        .get_config_home()
        .map(|path| {
            Utf8PathBuf::from_path_buf(path)
                .map_err(|_| anyhow::anyhow!("XDG config home is not valid UTF-8"))
        })
        .transpose()?
        .unwrap_or_else(|| {
            Utf8PathBuf::from(format!(
                "{}/.config",
                std::env::var("HOME").unwrap_or_else(|_| ".".into())
            ))
        });
    Ok(config_home.join("riptask"))
}

pub fn resolve_riptask_repo() -> Result<Utf8PathBuf> {
    if let Ok(value) = std::env::var("RIPTASK_REPO") {
        return Ok(Utf8PathBuf::from(value));
    }

    let xdg_dirs = xdg::BaseDirectories::new();
    let config_home = xdg_dirs
        .get_config_home()
        .unwrap_or_else(|| {
            std::path::PathBuf::from(format!(
                "{}/.config",
                std::env::var("HOME").unwrap_or_else(|_| ".".into())
            ))
        })
        .join("riptask")
        .join("config.env");
    if let Ok(content) = fs::read_to_string(&config_home) {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("RIPTASK_REPO=") {
                return Ok(Utf8PathBuf::from(rest.trim_matches('"')));
            }
        }
    }

    let data_home = xdg_dirs
        .get_data_home()
        .unwrap_or_else(|| {
            std::path::PathBuf::from(format!(
                "{}/.local/share",
                std::env::var("HOME").unwrap_or_else(|_| ".".into())
            ))
        })
        .join("riptask");
    Utf8PathBuf::from_path_buf(data_home)
        .map_err(|_| anyhow::anyhow!("XDG data home is not valid UTF-8"))
}

pub fn resolve_cache_root() -> Result<Utf8PathBuf> {
    let xdg_dirs = xdg::BaseDirectories::new();
    let cache_home = xdg_dirs
        .get_cache_home()
        .unwrap_or_else(|| {
            std::path::PathBuf::from(format!(
                "{}/.cache",
                std::env::var("HOME").unwrap_or_else(|_| ".".into())
            ))
        })
        .join("riptask");
    Utf8PathBuf::from_path_buf(cache_home)
        .map_err(|_| anyhow::anyhow!("XDG cache home is not valid UTF-8"))
}

pub fn resolve_state_root() -> Result<Utf8PathBuf> {
    let xdg_dirs = xdg::BaseDirectories::new();
    let state_home = xdg_dirs
        .get_state_home()
        .unwrap_or_else(|| {
            std::path::PathBuf::from(format!(
                "{}/.local/state",
                std::env::var("HOME").unwrap_or_else(|_| ".".into())
            ))
        })
        .join("riptask");
    Utf8PathBuf::from_path_buf(state_home)
        .map_err(|_| anyhow::anyhow!("XDG state home is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::AppPaths;

    #[test]
    fn builds_standard_subpaths() {
        let paths = AppPaths {
            riptask_repo: "/tmp/riptask".into(),
            user_config_root: "/tmp/config/riptask".into(),
            cache_root: "/tmp/cache".into(),
            state_root: "/tmp/state".into(),
        };
        assert_eq!(paths.system_config_path(), "/tmp/riptask/config.yaml");
        assert_eq!(paths.user_config_path(), "/tmp/config/riptask/config.yaml");
        assert_eq!(paths.backend_state_path(), "/tmp/cache/backend_state.json");
        assert_eq!(paths.deleted_keys_path(), "/tmp/cache/deleted_keys.json");
        assert_eq!(paths.log_path(), "/tmp/state/riptask.log");
    }
}
