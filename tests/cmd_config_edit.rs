use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::tempdir;

#[test]
fn edit_zero_files_inside_project_creates_local_scaffold() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    let editor = shim(&temp, "editor", "exit 0\n");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&project)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .env("EDITOR", &editor)
        .args(["config", "edit"])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(project.join(".riptask/config.yaml")).expect("local config"),
        "# riptask config (scope: local)\nversion: 1\n"
    );
}

#[test]
fn edit_zero_files_outside_project_errors() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&outside).expect("outside dir");
    let editor = shim(&temp, "editor", "exit 0\n");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&outside)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .env("EDITOR", &editor)
        .args(["config", "edit"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not inside a riptask project"));
}

#[test]
fn edit_one_file_opens_directly_without_fzf() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let marker = temp.path().join("opened");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::write(repo.join("config.yaml"), "version: 1\n").expect("system");
    let editor = shim(
        &temp,
        "editor",
        &format!("printf '%s' \"$1\" > '{}'\n", marker.display()),
    );
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("bin");
    shim_in(&bin, "fzf", "exit 7\n");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .env("EDITOR", &editor)
        .env("PATH", &bin)
        .args(["config", "edit"])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(marker).expect("marker"),
        repo.join("config.yaml").to_string_lossy()
    );
}

#[test]
fn edit_multiple_files_invokes_fzf_then_editor() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let user = xdg.join("riptask");
    let marker = temp.path().join("opened");
    let fzf_marker = temp.path().join("fzf-called");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&user).expect("user dir");
    fs::write(repo.join("config.yaml"), "version: 1\n").expect("system");
    fs::write(user.join("config.yaml"), "version: 1\n").expect("user");
    let editor = shim(
        &temp,
        "editor",
        &format!("printf '%s' \"$1\" > '{}'\n", marker.display()),
    );
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("bin");
    shim_in(
        &bin,
        "fzf",
        &format!(
            "IFS= read -r line\n: > '{}'\nprintf '%s\\n' \"$line\"\n",
            fzf_marker.display()
        ),
    );

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .env("EDITOR", &editor)
        .env("PATH", &bin)
        .args(["config", "edit"])
        .assert()
        .success();

    assert!(fzf_marker.exists());
    assert_eq!(
        fs::read_to_string(marker).expect("marker"),
        repo.join("config.yaml").to_string_lossy()
    );
}

#[test]
fn edit_missing_editor_errors_before_fzf() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let user = xdg.join("riptask");
    let fzf_marker = temp.path().join("fzf-called");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&user).expect("user dir");
    fs::write(repo.join("config.yaml"), "version: 1\n").expect("system");
    fs::write(user.join("config.yaml"), "version: 1\n").expect("user");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("bin");
    shim_in(
        &bin,
        "fzf",
        &format!(
            "IFS= read -r line\n: > '{}'\nprintf '%s\\n' \"$line\"\n",
            fzf_marker.display()
        ),
    );

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .env_remove("EDITOR")
        .env("PATH", &bin)
        .args(["config", "edit"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("set $EDITOR to edit"));

    assert!(!fzf_marker.exists());
}

#[test]
fn edit_missing_fzf_errors_when_picker_needed() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let user = xdg.join("riptask");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(&user).expect("user dir");
    fs::write(repo.join("config.yaml"), "version: 1\n").expect("system");
    fs::write(user.join("config.yaml"), "version: 1\n").expect("user");
    let editor = shim(&temp, "editor", "exit 0\n");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .env("EDITOR", &editor)
        .env("PATH", temp.path().join("empty"))
        .args(["config", "edit"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "fzf not found (required for interactive selection)",
        ));
}

#[test]
fn edit_validation_failure_leaves_file_on_disk() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::write(repo.join("config.yaml"), "version: 1\n").expect("system");
    let editor = shim(&temp, "editor", "printf 'unknown: true\\n' > \"$1\"\n");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .env("EDITOR", &editor)
        .args(["config", "edit"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("file NOT reverted"));

    assert_eq!(
        fs::read_to_string(repo.join("config.yaml")).expect("system"),
        "unknown: true\n"
    );
}

fn shim(temp: &tempfile::TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("bin");
    shim_in(&bin, name, body)
}

fn shim_in(bin: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = bin.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write shim");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("chmod");
    path
}
