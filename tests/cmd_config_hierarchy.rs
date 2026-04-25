use assert_cmd::Command;
use predicates::prelude::*;
use riptask::config::load_effective_config;
use riptask::paths::AppPaths;
use std::fs;
use tempfile::tempdir;

#[test]
fn effective_config_uses_local_scalar_over_user_and_system() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let user = temp.path().join("xdg").join("riptask");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&user).expect("user dir");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    fs::write(repo.join("config.yaml"), "auto_commit: false\n").expect("system");
    fs::write(user.join("config.yaml"), "auto_commit: false\n").expect("user");
    fs::write(project.join(".riptask/config.yaml"), "auto_commit: true\n").expect("local");
    let paths = app_paths(&temp, &repo, &user);

    let config = load_effective_config(&paths, camino::Utf8Path::from_path(&project).unwrap())
        .expect("effective config");

    assert!(config.auto_commit);
}

#[test]
fn config_set_global_writes_only_user_layer() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let user = xdg.join("riptask");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    fs::write(repo.join("config.yaml"), "auto_commit: false\n").expect("system");
    fs::write(project.join(".riptask/config.yaml"), "auto_commit: false\n").expect("local");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&project)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .args(["config", "set", "--global", "auto_commit", "true"])
        .assert()
        .success();

    assert!(
        fs::read_to_string(user.join("config.yaml"))
            .expect("user config")
            .contains("auto_commit: true")
    );
    assert!(
        fs::read_to_string(repo.join("config.yaml"))
            .expect("system config")
            .contains("auto_commit: false")
    );
    assert!(
        fs::read_to_string(project.join(".riptask/config.yaml"))
            .expect("local config")
            .contains("auto_commit: false")
    );
}

#[test]
fn config_set_without_scope_inside_project_writes_local() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    fs::write(repo.join("config.yaml"), "").expect("system");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&project)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .args(["config", "set", "auto_commit", "true"])
        .assert()
        .success();

    assert!(
        fs::read_to_string(project.join(".riptask/config.yaml"))
            .expect("local config")
            .contains("auto_commit: true")
    );
    assert!(!xdg.join("riptask/config.yaml").exists());
}

#[test]
fn config_set_without_scope_outside_project_writes_user() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&outside).expect("outside dir");
    fs::write(repo.join("config.yaml"), "").expect("system");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&outside)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", &outside)
        .args(["config", "set", "auto_commit", "true"])
        .assert()
        .success();

    assert!(
        fs::read_to_string(xdg.join("riptask/config.yaml"))
            .expect("user config")
            .contains("auto_commit: true")
    );
}

#[test]
fn vec_dedupe_across_layers_keeps_higher_priority_project() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let user = temp.path().join("xdg").join("riptask");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&user).expect("user dir");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    fs::write(repo.join("config.yaml"), project_yaml("foo", "ALPHA")).expect("system");
    fs::write(
        project.join(".riptask/config.yaml"),
        project_yaml("foo", "BETA"),
    )
    .expect("local");
    let paths = app_paths(&temp, &repo, &user);

    let config = load_effective_config(&paths, camino::Utf8Path::from_path(&project).unwrap())
        .expect("effective config");

    assert_eq!(config.projects.len(), 1);
    assert_eq!(config.projects[0].name, "foo");
    assert_eq!(config.projects[0].key.as_deref(), Some("BETA"));
}

#[test]
fn bare_config_prints_consulted_comment_and_yaml() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&outside).expect("outside dir");
    fs::write(repo.join("config.yaml"), "auto_commit: true\n").expect("system");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&outside)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", &outside)
        .arg("config")
        .assert()
        .success()
        .stdout(predicate::str::contains("# consulted:"))
        .stdout(predicate::str::contains("auto_commit: true"));
}

fn app_paths(temp: &tempfile::TempDir, repo: &std::path::Path, user: &std::path::Path) -> AppPaths {
    AppPaths {
        riptask_repo: repo.to_string_lossy().as_ref().into(),
        user_config_root: user.to_string_lossy().as_ref().into(),
        cache_root: temp.path().join("cache").to_string_lossy().as_ref().into(),
        state_root: temp.path().join("state").to_string_lossy().as_ref().into(),
    }
}

fn project_yaml(name: &str, key: &str) -> String {
    format!(
        r#"projects:
  - name: {name}
    vc_backend:
      type: github
      repo: owner/{name}
    tasks_backend:
      type: github
      repo: owner/{name}
    default_board: personal
    key: {key}
"#
    )
}
