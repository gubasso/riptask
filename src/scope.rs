use crate::config::{Config, RemoteConfig};
use crate::error::TskError;
use crate::services::project_detection;
use camino::Utf8Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectScope {
    CurrentProject(String),
    Explicit(Vec<String>),
    AllProjects,
}

impl ProjectScope {
    pub fn matches(&self, project: &str) -> bool {
        match self {
            Self::CurrentProject(current) => current == project,
            Self::Explicit(projects) => projects.iter().any(|candidate| candidate == project),
            Self::AllProjects => true,
        }
    }
}

pub fn resolve_scope(
    projects: &[String],
    all_projects: bool,
    cwd: &Utf8Path,
    config: &Config,
) -> Result<ProjectScope, TskError> {
    if all_projects {
        return Ok(ProjectScope::AllProjects);
    }
    if !projects.is_empty() {
        return Ok(ProjectScope::Explicit(projects.to_vec()));
    }
    match project_detection::detect_from_cwd(cwd, config)? {
        Some(remote) => Ok(ProjectScope::CurrentProject(remote.name)),
        None => Ok(ProjectScope::AllProjects),
    }
}

pub fn lookup_project<'a>(config: &'a Config, name: &str) -> Option<&'a RemoteConfig> {
    config.remotes.iter().find(|remote| remote.name == name)
}
