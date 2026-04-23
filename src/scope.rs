use crate::config::Config;
use crate::error::RiptskError;
use crate::models::RepoProject;
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
) -> Result<ProjectScope, RiptskError> {
    if all_projects {
        return Ok(ProjectScope::AllProjects);
    }
    if !projects.is_empty() {
        return Ok(ProjectScope::Explicit(projects.to_vec()));
    }
    match project_detection::detect_from_cwd(cwd, config)? {
        Some(repo_project) => Ok(ProjectScope::CurrentProject(repo_project.name.clone())),
        None => Err(RiptskError::Config(
            "could not detect project from current directory; use -p <project> or -a to target all projects".into(),
        )),
    }
}

pub fn lookup_project<'a>(config: &'a Config, name: &str) -> Option<&'a RepoProject> {
    config
        .projects
        .iter()
        .find(|repo_project| repo_project.name == name)
}

#[cfg(test)]
mod tests {
    use super::{ProjectScope, resolve_scope};
    use crate::config::default_config;
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    #[test]
    fn resolve_scope_returns_all_projects_when_flag_is_set() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).expect("utf8 path");
        let config = default_config();

        let scope = resolve_scope(&[], true, &cwd, &config).expect("scope");

        assert_eq!(scope, ProjectScope::AllProjects);
    }

    #[test]
    fn resolve_scope_returns_explicit_projects_when_provided() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).expect("utf8 path");
        let config = default_config();

        let scope = resolve_scope(&["foo".into()], false, &cwd, &config).expect("scope");

        assert_eq!(scope, ProjectScope::Explicit(vec!["foo".into()]));
    }

    #[test]
    fn resolve_scope_errors_when_detection_fails() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).expect("utf8 path");
        let config = default_config();

        let err = resolve_scope(&[], false, &cwd, &config).unwrap_err();

        assert!(
            err.to_string().contains("could not detect project"),
            "unexpected error: {err}"
        );
    }
}
