use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

fn complete_config() -> &'static str {
    r#"defaults: {}
boards:
  - name: personal
    statuses: []
ui: {}
ai: {}
sync: {}
"#
}

fn command_with_env(repo: &std::path::Path, temp: &tempfile::TempDir) -> Command {
    let mut command = Command::cargo_bin("tsk").expect("binary");
    command
        .env("RIPTASK_REPO", repo)
        .env("XDG_CONFIG_HOME", temp.path().join("xdg"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path());
    command
}

#[test]
fn init_user_warns_when_system_complete_exists() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::write(repo.join("config.yaml"), complete_config()).expect("system config");

    command_with_env(&repo, &temp)
        .args(["init", "--user"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tsk config inventory:"))
        .stderr(predicate::str::contains(
            "a complete config also exists at system",
        ))
        .stderr(predicate::str::contains("initialized user at"));

    let user_config = temp.path().join("xdg/riptask/config.yaml");
    assert_eq!(
        fs::read_to_string(user_config).expect("user config"),
        "# riptask config (scope: user)\n"
    );
}

#[test]
fn init_user_errors_when_user_complete_without_force() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let user = temp.path().join("xdg/riptask");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&user).expect("user dir");
    fs::write(user.join("config.yaml"), complete_config()).expect("user config");

    command_with_env(&repo, &temp)
        .args(["init", "--user"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("user config is already complete"));
}

#[test]
fn init_user_force_overwrites_complete_with_header() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let user = temp.path().join("xdg/riptask");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&user).expect("user dir");
    let path = user.join("config.yaml");
    fs::write(&path, complete_config()).expect("user config");

    command_with_env(&repo, &temp)
        .args(["init", "--user", "--force"])
        .assert()
        .success()
        .stderr(predicate::str::contains("initialized user at"));

    assert_eq!(
        fs::read_to_string(path).expect("user config"),
        "# riptask config (scope: user)\n"
    );
}

#[test]
fn init_local_scaffolds_fresh_directory_without_git() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&project).expect("project dir");

    command_with_env(&repo, &temp)
        .current_dir(&project)
        .args(["init", "--local"])
        .assert()
        .success()
        .stderr(predicate::str::contains("initialized local at"));

    assert_eq!(
        fs::read_to_string(project.join(".riptask/config.yaml")).expect("local config"),
        "# riptask config (scope: local)\n"
    );
}

#[test]
fn init_without_scope_errors_in_non_tty() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");

    command_with_env(&repo, &temp)
        .arg("init")
        .assert()
        .failure()
        .stdout(predicate::str::contains("tsk config inventory:"))
        .stderr(predicate::str::contains(
            "requires one of --system|--user|--local",
        ));
}

#[test]
fn init_system_errors_when_system_complete_without_force() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::write(repo.join("config.yaml"), complete_config()).expect("system config");

    command_with_env(&repo, &temp)
        .args(["init", "--system"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "system config is already complete",
        ));
}
