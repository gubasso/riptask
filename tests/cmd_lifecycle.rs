use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
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

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "Test issue"])
        .assert()
        .success()
        .stdout(predicate::str::contains("LO-PER-PER--1"));
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
        .stdout(predicate::str::contains("LO-WOR-WOR--1"));

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
fn new_ai_falls_back_to_template_body_when_backend_unavailable() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let empty_path = temp.path().join("empty-path");
    fs::create_dir_all(&empty_path).expect("empty path dir");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    // Enable AI in config
    let config_path = repo.join("riptsk.yaml");
    let config = fs::read_to_string(&config_path).expect("read config");
    fs::write(
        &config_path,
        config.replace("enabled: false", "enabled: true"),
    )
    .expect("write config");

    // Run new --ai with no AI binary in PATH — should succeed with fallback.
    // Keep git in PATH so auto-registration doesn't fail.
    let git_dir = std::path::PathBuf::from(
        StdCommand::new("which")
            .arg("git")
            .output()
            .expect("which git")
            .stdout
            .iter()
            .map(|&b| b as char)
            .collect::<String>()
            .trim()
            .to_string(),
    )
    .parent()
    .unwrap()
    .to_owned();
    let restricted_path = format!("{}:{}", empty_path.display(), git_dir.display());
    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .env("PATH", &restricted_path)
        .args(["new", "--title", "AI fallback test", "--ai"])
        .assert()
        .success()
        .stderr(predicate::str::contains("AI body generation failed"));

    // Exactly one issue should exist
    let issues: Vec<_> = fs::read_dir(repo.join("issues"))
        .expect("issues dir")
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .collect();
    assert_eq!(issues.len(), 1, "expected exactly one issue file");

    // Verify the issue uses the template body (not an AI-generated one)
    let issue_content = fs::read_to_string(issues[0].path()).expect("read issue");
    assert!(
        issue_content.contains("## Description"),
        "issue body should contain template content"
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
fn new_without_title_non_tty_errors() {
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
        .stderr(predicate::str::contains("missing title"));
}
