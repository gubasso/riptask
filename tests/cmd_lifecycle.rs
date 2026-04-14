use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command as StdCommand;
use tempfile::tempdir;

#[test]
fn new_creates_issue_file() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    // Use --project personal to target the default project explicitly,
    // since cwd (a non-git dir) would be auto-registered as its own project.
    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "Test issue", "--project", "personal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PERSONAL--1"));
}

#[cfg(unix)]
#[test]
fn new_edit_opens_editor_after_creation() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let editor_script = temp.path().join("fake-editor.sh");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    fs::write(
        &editor_script,
        "#!/bin/sh\nprintf '\\nEDITOR_MARKER\\n' >> \"$1\"\n",
    )
    .expect("write editor script");
    let mut permissions = fs::metadata(&editor_script)
        .expect("editor metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&editor_script, permissions).expect("chmod editor script");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .env("EDITOR", &editor_script)
        .args([
            "new",
            "--title",
            "Edit test",
            "--project",
            "personal",
            "--edit",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("PERSONAL--1"));

    let issue_path = fs::read_dir(repo.join("issues"))
        .expect("issues dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .expect("issue file");
    let issue = fs::read_to_string(issue_path).expect("read issue");
    assert!(issue.contains("EDITOR_MARKER"));
}

#[test]
fn new_auto_registers_unregistered_repo() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let worktree = temp.path().join("worktree");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    fs::create_dir_all(&worktree).expect("worktree dir");
    let status = StdCommand::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&worktree)
        .status()
        .expect("git init");
    assert!(status.success());

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&worktree)
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "Auto registered"])
        .assert()
        .success()
        .stdout(predicate::str::contains("WORKTREE--1"));

    let config = fs::read_to_string(repo.join("riptsk.yaml")).expect("read config");
    assert!(config.contains("name: worktree"));

    let issue_path = fs::read_dir(repo.join("issues"))
        .expect("issues dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .expect("issue file");
    let issue = fs::read_to_string(issue_path).expect("read issue");
    assert!(issue.contains("project: worktree"));
}

#[test]
fn new_with_title_skips_ai_body_by_default() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "ai test"])
        .assert()
        .success()
        .stderr(predicate::str::contains("AI body generation failed").not());

    let issues: Vec<_> = fs::read_dir(repo.join("issues"))
        .expect("issues dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert_eq!(issues.len(), 1, "expected exactly one issue file");

    let issue_content = fs::read_to_string(issues[0].path()).expect("read issue");
    assert!(
        issue_content.contains("## Description"),
        "issue body should contain template content"
    );
}

#[test]
fn new_with_title_and_description_skips_ai() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "manual test", "--description", "my desc"])
        .assert()
        .success()
        .stderr(predicate::str::contains("AI body generation failed").not());

    let issues: Vec<_> = fs::read_dir(repo.join("issues"))
        .expect("issues dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert_eq!(issues.len(), 1, "expected exactly one issue file");

    let issue_content = fs::read_to_string(issues[0].path()).expect("read issue");
    assert!(
        issue_content.contains("my desc"),
        "issue body should contain the provided description"
    );
}

#[test]
fn new_requires_initialization() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "Test issue"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "not initialized — run 'tsk init' first",
        ));
}

#[test]
fn new_without_args_non_tty_errors_clearly() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("new")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "AI issue generation failed and no --title provided",
        ));
}

#[test]
fn new_with_title_and_ai_flag_attempts_ai_body() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "ai test", "--ai"])
        .assert()
        .success()
        .stderr(predicate::str::contains("AI body generation failed"));

    let issues: Vec<_> = fs::read_dir(repo.join("issues"))
        .expect("issues dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert_eq!(issues.len(), 1, "expected exactly one issue file");
}

#[test]
fn new_auto_registers_non_git_directory() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let plain_dir = temp.path().join("my-proj");
    fs::create_dir_all(&plain_dir).expect("plain dir");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    let assert = Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&plain_dir)
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "Non-git issue"])
        .assert()
        .success();

    // Issue ID should use the derived local project key.
    assert.stdout(predicate::str::starts_with("MYPROJ--"));

    // Config should contain the auto-registered backend
    let config = fs::read_to_string(repo.join("riptsk.yaml")).expect("read config");
    assert!(
        config.contains("name: my-proj"),
        "backend name missing from config"
    );
    assert!(
        config.contains("type: local"),
        "backend type missing from config"
    );
}

#[test]
fn new_non_git_no_stderr_leak() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let plain_dir = temp.path().join("noisy-dir");
    fs::create_dir_all(&plain_dir).expect("plain dir");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&plain_dir)
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "Quiet issue"])
        .assert()
        .success()
        .stderr(predicate::str::contains("fatal:").not());
}

#[test]
fn new_does_not_register_home_as_project() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let fake_home = temp.path().join("fakehome");
    let subdir = fake_home.join("subdir");
    fs::create_dir_all(&subdir).expect("subdir");

    // Put a git repo at fake $HOME
    let status = StdCommand::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(&fake_home)
        .status()
        .expect("git init");
    assert!(status.success());

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    // Run from subdir with HOME set to fake_home
    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&subdir)
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .env("HOME", &fake_home)
        .args(["new", "--title", "Home boundary test"])
        .assert()
        .success();

    // The registered project should be "subdir", not "fakehome"
    let config = fs::read_to_string(repo.join("riptsk.yaml")).expect("read config");
    assert!(
        config.contains("name: subdir"),
        "should register subdir, not $HOME. Config:\n{config}"
    );
    assert!(
        !config.contains("name: fakehome"),
        "should not register $HOME as project"
    );
}
