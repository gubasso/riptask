use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

fn command_with_env(repo: &std::path::Path, temp: &tempfile::TempDir) -> Command {
    let mut command = Command::cargo_bin("tsk").expect("binary");
    command
        .env("RIPTASK_REPO", repo)
        .env("XDG_CONFIG_HOME", temp.path().join("xdg"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", temp.path())
        .current_dir(temp.path());
    command
}

fn init_system(repo: &std::path::Path, temp: &tempfile::TempDir) {
    command_with_env(repo, temp)
        .args(["init", "--system"])
        .assert()
        .success();
}

fn recur_new_args(scope: &str) -> Vec<&str> {
    vec![
        "recur",
        "new",
        scope,
        "--id",
        "daily",
        "--frequency",
        "daily",
        "--title-pattern",
        "daily {{date}}",
    ]
}

#[test]
fn recur_new_system_writes_system_layer() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    init_system(&repo, &temp);

    command_with_env(&repo, &temp)
        .args(recur_new_args("--system"))
        .assert()
        .success()
        .stderr(predicate::str::contains("wrote recurrence to system"));

    assert!(
        fs::read_to_string(repo.join("config.yaml"))
            .expect("system")
            .contains("id: daily")
    );
}

#[test]
fn recur_new_user_alias_writes_user_layer() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    init_system(&repo, &temp);

    command_with_env(&repo, &temp)
        .args(recur_new_args("--user"))
        .assert()
        .success()
        .stderr(predicate::str::contains("wrote recurrence to user"));

    assert!(
        fs::read_to_string(temp.path().join("xdg/riptask/config.yaml"))
            .expect("user")
            .contains("id: daily")
    );
}

#[test]
fn recur_new_local_writes_local_layer() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let project = temp.path().join("project");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    init_system(&repo, &temp);

    command_with_env(&repo, &temp)
        .current_dir(&project)
        .args(recur_new_args("--local"))
        .assert()
        .success()
        .stderr(predicate::str::contains("wrote recurrence to local"));

    assert!(
        fs::read_to_string(project.join(".riptask/config.yaml"))
            .expect("local")
            .contains("id: daily")
    );
}

#[test]
fn recur_skip_writes_resolving_local_layer() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let project = temp.path().join("project");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    init_system(&repo, &temp);
    fs::write(
        project.join(".riptask/config.yaml"),
        r#"recurring:
  - id: daily
    template: task
    title_pattern: daily
    frequency: daily
"#,
    )
    .expect("local recurring");

    command_with_env(&repo, &temp)
        .current_dir(&project)
        .args(["recur", "skip", "daily"])
        .assert()
        .success()
        .stderr(predicate::str::contains("wrote recurrence to local"));

    assert!(
        fs::read_to_string(project.join(".riptask/config.yaml"))
            .expect("local")
            .contains("last_run:")
    );
}

#[test]
fn recur_skip_unknown_errors() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    init_system(&repo, &temp);

    command_with_env(&repo, &temp)
        .args(["recur", "skip", "missing"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown recurrence id: missing"));
}

#[test]
fn recur_run_updates_system_when_only_system_defines_id() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    init_system(&repo, &temp);

    let mut new_args = recur_new_args("--system");
    new_args.extend(["--start", "2026-04-24"]);
    command_with_env(&repo, &temp)
        .current_dir(temp.path())
        .args(new_args)
        .assert()
        .success();

    command_with_env(&repo, &temp)
        .current_dir(temp.path())
        .args(["recur", "run", "2026-04-25"])
        .assert()
        .success()
        .stderr(predicate::str::contains("wrote 1 recurrence(s) to system"));

    assert!(
        fs::read_to_string(repo.join("config.yaml"))
            .expect("system")
            .contains("last_run:")
    );
}

#[test]
fn recur_run_updates_local_when_local_shadows_system() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let project = temp.path().join("project");
    fs::create_dir_all(project.join(".riptask")).expect("local dir");
    init_system(&repo, &temp);
    fs::write(
        project.join(".riptask/config.yaml"),
        r#"recurring:
  - id: daily
    template: task
    title_pattern: local daily
    frequency: daily
    start: 2026-04-24
"#,
    )
    .expect("local recurring");
    let system = fs::read_to_string(repo.join("config.yaml")).expect("system");
    fs::write(
        repo.join("config.yaml"),
        system.replace(
            "recurring: []",
            "recurring:\n  - id: daily\n    template: task\n    title_pattern: system daily\n    frequency: daily\n    start: 2026-04-24",
        ),
    )
    .expect("system recurring");

    command_with_env(&repo, &temp)
        .current_dir(&project)
        .args(["recur", "run", "2026-04-25"])
        .assert()
        .success()
        .stderr(predicate::str::contains("wrote 1 recurrence(s) to local"));

    assert!(
        fs::read_to_string(project.join(".riptask/config.yaml"))
            .expect("local")
            .contains("last_run:")
    );
    assert!(
        !fs::read_to_string(repo.join("config.yaml"))
            .expect("system")
            .contains("last_run:")
    );
}
