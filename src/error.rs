use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKeyProjectMeta {
    pub name: String,
    pub backend: String,
    pub host: Option<String>,
    pub repo: Option<String>,
    pub path: Option<String>,
    pub existing_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKeyCollision {
    pub attempted_key: String,
    pub new_project: ProjectKeyProjectMeta,
    pub conflicting_project: ProjectKeyProjectMeta,
}

#[derive(Debug, Error)]
pub enum RiptskError {
    #[error("{0}")]
    General(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("sync conflict: {0}")]
    Conflict(String),
    #[error("remote unreachable: {0}")]
    Unreachable(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error(
        "project key collision: {} conflicts with existing project '{}'",
        .0.attempted_key,
        .0.conflicting_project.name
    )]
    KeyCollision(Box<ProjectKeyCollision>),
    #[error("project not registered: {0}")]
    Unregistered(String),
    #[error(transparent)]
    Frontmatter(#[from] FrontmatterError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl RiptskError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::General(_) | Self::Store(_) | Self::Io(_) | Self::Other(_) => 1,
            Self::NotFound(_) => 2,
            Self::Conflict(_) => 3,
            Self::Unreachable(_) => 4,
            Self::Config(_) | Self::KeyCollision(_) | Self::Frontmatter(_) => 5,
            Self::Unregistered(_) => 6,
            Self::Auth(_) => 7,
        }
    }
}

#[derive(Debug, Error)]
pub enum FrontmatterError {
    #[error("missing frontmatter delimiters in {path}")]
    MissingDelimiters { path: String },
    #[error("invalid YAML in {path}: {detail}")]
    InvalidYaml { path: String, detail: String },
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("issue file not found: {0}")]
    FileNotFound(String),
    #[error("duplicate issue ID: {0}")]
    DuplicateId(String),
}

#[cfg(test)]
mod tests {
    use super::{ProjectKeyCollision, ProjectKeyProjectMeta, RiptskError};

    #[test]
    fn exit_codes_match_contract() {
        assert_eq!(RiptskError::General("x".into()).exit_code(), 1);
        assert_eq!(RiptskError::NotFound("x".into()).exit_code(), 2);
        assert_eq!(RiptskError::Conflict("x".into()).exit_code(), 3);
        assert_eq!(RiptskError::Unreachable("x".into()).exit_code(), 4);
        assert_eq!(RiptskError::Config("x".into()).exit_code(), 5);
        assert_eq!(
            RiptskError::KeyCollision(Box::new(ProjectKeyCollision {
                attempted_key: "X".into(),
                new_project: ProjectKeyProjectMeta {
                    name: "new".into(),
                    backend: "github".into(),
                    host: None,
                    repo: None,
                    path: None,
                    existing_key: None,
                },
                conflicting_project: ProjectKeyProjectMeta {
                    name: "existing".into(),
                    backend: "github".into(),
                    host: None,
                    repo: None,
                    path: None,
                    existing_key: Some("X".into()),
                },
            }))
            .exit_code(),
            5
        );
        assert_eq!(RiptskError::Unregistered("x".into()).exit_code(), 6);
        assert_eq!(RiptskError::Auth("x".into()).exit_code(), 7);
    }
}
