use camino::Utf8PathBuf;
use riptsk::config::default_config;
use riptsk::error::RiptskError;
use riptsk::models::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};
use riptsk::services::project_detection::register_project_auto;
use tempfile::tempdir;

fn repo_project(
    kind: BackendKind,
    name: &str,
    path: Option<&str>,
    key: Option<&str>,
) -> RepoProject {
    RepoProject {
        name: name.into(),
        vc_backend: VCBackendSpec {
            kind: kind.clone(),
            host: None,
            repo: None,
            path: path.map(str::to_owned),
        },
        tasks_backend: TasksBackendSpec {
            kind,
            host: None,
            repo: None,
            jira_project: None,
            default_issue_type: None,
            path: path.map(str::to_owned),
        },
        default_board: Some("personal".into()),
        default_org: None,
        key: key.map(str::to_owned),
        repo_project_label: None,
    }
}

#[test]
fn auto_register_assigns_derived_key() {
    let temp = tempdir().expect("temp dir");
    let plain_dir = temp.path().join("my-proj");
    std::fs::create_dir_all(&plain_dir).expect("plain dir");
    let cwd = Utf8PathBuf::from_path_buf(plain_dir).expect("utf8 path");
    let mut config = default_config();

    let repo_project = register_project_auto(&cwd, &mut config, None, None)
        .expect("register")
        .expect("repo project");

    assert_eq!(repo_project.key.as_deref(), Some("MYPROJ"));
    assert_eq!(config.projects.len(), 1);
}

#[test]
fn auto_register_collision_without_prompts_returns_key_collision_and_does_not_mutate_config() {
    let temp = tempdir().expect("temp dir");
    let existing_dir = temp.path().join("existing-foo");
    let new_dir = temp.path().join("foo");
    std::fs::create_dir_all(&existing_dir).expect("existing dir");
    std::fs::create_dir_all(&new_dir).expect("new dir");
    let cwd = Utf8PathBuf::from_path_buf(new_dir).expect("utf8 path");
    let existing_path = existing_dir.to_string_lossy().to_string();

    let mut config = default_config();
    config.projects.push(repo_project(
        BackendKind::Local,
        "existing",
        Some(&existing_path),
        Some("FOO"),
    ));
    let before = config.projects.len();

    let error = register_project_auto(&cwd, &mut config, None, None).expect_err("collision");

    assert!(matches!(error, RiptskError::KeyCollision(_)));
    assert_eq!(config.projects.len(), before);
}
