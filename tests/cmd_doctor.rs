use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn doctor_reports_layers_and_writes_effective_config_dump() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let cache = temp.path().join("cache");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    fs::write(repo.join("config.yaml"), "ui:\n  tree_depth: 1\n").expect("system");
    fs::write(
        project.join(".riptask/config.yaml"),
        "ui:\n  tree_depth: 7\n",
    )
    .expect("local");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&project)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", &cache)
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("riptask doctor"))
        .stdout(predicate::str::contains("Config layers"))
        .stdout(predicate::str::contains("[x] system"))
        .stdout(predicate::str::contains("(missing)"))
        .stdout(predicate::str::contains("[x] local"))
        .stdout(predicate::str::contains("Effective configuration"))
        .stdout(predicate::str::contains("tree_depth: 7"));

    let dump = cache.join("riptask").join("effective-config.yaml");
    let dumped = fs::read_to_string(&dump).expect("effective dump exists");
    assert!(dumped.contains("tree_depth: 7"));
}

#[test]
fn doctor_runs_outside_project_without_local_layer() {
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
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", &outside)
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "project_root     = (none detected)",
        ))
        .stdout(predicate::str::contains("not inside a riptask project"));
}
