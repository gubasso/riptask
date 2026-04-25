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

#[test]
fn shared_layer_commands_error_clearly_when_only_user_layer_exists() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let user = xdg.join("riptask");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&repo).expect("repo dir (empty, no config.yaml)");
    fs::create_dir_all(&user).expect("user dir");
    fs::create_dir_all(&outside).expect("outside dir");
    fs::write(user.join("config.yaml"), "auto_commit: false\n").expect("user config");

    let expected = "shared task store not initialized";
    let invocations: &[&[&str]] = &[
        // issues
        &["ls"],
        &["show", "foo"],
        &["path", "foo"],
        &["id"],
        &["new", "--title", "x"],
        &["edit", "foo"],
        &["status", "foo", "doing"],
        &["close", "foo"],
        &["reopen", "foo"],
        &["rm", "foo"],
        // lifecycle
        &["done", "foo"],
        &["start", "foo"],
        &["clone", "foo"],
        &["branch", "foo"],
        &["branch", "--adopt", "foo"],
        &["branch", "-d", "foo"],
        // pr
        &["pr", "foo"],
        &["pr", "create", "foo"],
        &["pr", "show", "foo"],
        &["pr", "merge", "foo"],
        &["pr", "edit", "foo"],
        // views
        &["view"],
        &["board"],
        &["reorder", "todo", "a,b"],
        &["reorder-up", "foo"],
        &["reorder-down", "foo"],
        // templates / recur / sync / store / session
        &["template", "list"],
        &["recur", "run"],
        &["recur", "skip", "foo"],
        &["sync"],
        &["store", "commit"],
        &["session", "start", "foo"],
        // ai
        &["summarize"],
        &["ask", "q"],
    ];

    for argv in invocations {
        Command::cargo_bin("tsk")
            .expect("binary")
            .current_dir(&outside)
            .env("RIPTASK_REPO", &repo)
            .env("XDG_CONFIG_HOME", &xdg)
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("HOME", &outside)
            .args(*argv)
            .assert()
            .failure()
            .stderr(predicate::str::contains(expected));
    }
}

#[test]
fn shared_layer_failure_does_not_persist_orphan_project_in_user_layer() {
    // Regression: pre-dispatch auto-registration must not write a project
    // entry into the user layer when the shared task store is missing.
    // Set up a non-home git project so that the auto-register path would
    // otherwise discover and persist a project before the dispatched
    // command's `require_shared_layer` gate runs.
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let user = xdg.join("riptask");
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir (empty, no config.yaml)");
    fs::create_dir_all(&user).expect("user dir");
    fs::create_dir_all(&home).expect("home dir");
    fs::create_dir_all(&project).expect("project dir");
    let user_config_before = "auto_commit: false\n";
    fs::write(user.join("config.yaml"), user_config_before).expect("user config");

    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&project)
        .status()
        .expect("git init");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&project)
        .status()
        .expect("git config email");
    std::process::Command::new("git")
        .args(["config", "user.name", "test"])
        .current_dir(&project)
        .status()
        .expect("git config name");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&project)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", &home)
        .args(["ls"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "shared task store not initialized",
        ));

    // The user-layer config must be byte-identical: no project entry was
    // appended by the aborted auto-registration path.
    let user_config_after = fs::read_to_string(user.join("config.yaml")).expect("read user");
    assert_eq!(
        user_config_after, user_config_before,
        "auto-registration must not mutate user config when shared layer is missing"
    );
    // No local layer must be created either: `default_write_scope` would have
    // chosen Local for a cwd inside a git project, so the orphan write would
    // land under <project>/.riptask/config.yaml without the gate.
    assert!(
        !project.join(".riptask").join("config.yaml").exists(),
        "auto-registration must not create a local config when shared layer is missing"
    );
}

#[test]
fn commit_succeeds_without_any_riptask_layer() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let project = temp.path().join("project");
    fs::create_dir_all(&repo).expect("repo dir (empty)");
    fs::create_dir_all(&project).expect("project dir");

    std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(&project)
        .status()
        .expect("git init");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&project)
        .status()
        .expect("git config email");
    std::process::Command::new("git")
        .args(["config", "user.name", "test"])
        .current_dir(&project)
        .status()
        .expect("git config name");
    fs::write(project.join("a.txt"), "hello").expect("write file");
    std::process::Command::new("git")
        .args(["add", "a.txt"])
        .current_dir(&project)
        .status()
        .expect("git add");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&project)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", &project)
        .args(["commit", "-m", "test"])
        .assert()
        .success();
}

#[test]
fn register_list_and_recur_list_succeed_with_only_user_layer() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let xdg = temp.path().join("xdg");
    let user = xdg.join("riptask");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&repo).expect("repo dir (empty, no config.yaml)");
    fs::create_dir_all(&user).expect("user dir");
    fs::create_dir_all(&outside).expect("outside dir");
    fs::write(user.join("config.yaml"), "auto_commit: false\n").expect("user config");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&outside)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", &outside)
        .args(["register", "--list"])
        .assert()
        .success();

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&outside)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("HOME", &outside)
        .args(["recur", "list"])
        .assert()
        .success();
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
