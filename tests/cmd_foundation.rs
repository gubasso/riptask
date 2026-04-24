use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn version_prints_package_version() {
    Command::cargo_bin("tsk")
        .expect("binary")
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains("tsk 0.1.0"));
}

#[test]
fn help_prints_usage() {
    Command::cargo_bin("tsk")
        .expect("binary")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Plaintext issue tracker"));
}

#[test]
fn done_help_prints_expected_flags() {
    Command::cargo_bin("tsk")
        .expect("binary")
        .args(["done", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--merge-method"))
        .stdout(predicate::str::contains("--auto-merge"))
        .stdout(predicate::str::contains("--force-push"))
        .stdout(predicate::str::contains("--timeout"))
        .stdout(predicate::str::contains("--yes"));
}

#[test]
fn pr_merge_help_prints_expected_flags() {
    Command::cargo_bin("tsk")
        .expect("binary")
        .args(["pr", "merge", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--merge-method"))
        .stdout(predicate::str::contains("--auto-merge"))
        .stdout(predicate::str::contains("--force-push"))
        .stdout(predicate::str::contains("--timeout"))
        .stdout(predicate::str::contains("--yes"));
}

#[test]
fn completions_bash_produces_valid_output() {
    Command::cargo_bin("tsk")
        .expect("binary")
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_tsk()"));
}

#[test]
fn completions_zsh_produces_valid_output() {
    Command::cargo_bin("tsk")
        .expect("binary")
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("#compdef tsk"));
}

#[test]
fn completions_fish_produces_valid_output() {
    Command::cargo_bin("tsk")
        .expect("binary")
        .args(["completions", "fish"])
        .assert()
        .success()
        .stdout(predicate::str::contains("__fish_tsk"));
}

#[test]
fn completions_rejects_invalid_shell() {
    Command::cargo_bin("tsk")
        .expect("binary")
        .args(["completions", "invalid"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn init_creates_repository_layout() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();
    assert!(repo.join("issues").exists());
    assert!(repo.join("templates").exists());
    assert!(repo.join("riptsk.yaml").exists());
}

#[test]
fn summarize_warns_when_ai_command_not_configured() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    let config_path = repo.join("riptsk.yaml");
    let config = fs::read_to_string(&config_path).expect("read config");
    fs::write(
        &config_path,
        config.replace("enabled: false", "enabled: true"),
    )
    .expect("write config");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("summarize")
        .assert()
        .success()
        .stderr(predicate::str::contains("ai.command is not configured"));
}

#[test]
fn summarize_uses_echo_ai_command_end_to_end() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    let config_path = repo.join("riptsk.yaml");
    let config = fs::read_to_string(&config_path).expect("read config");
    fs::write(
        &config_path,
        config.replace(
            "enabled: false",
            "enabled: true\n  command: \"echo {{input}}\"",
        ),
    )
    .expect("write config");

    fs::copy(
        format!(
            "{}/tests/fixtures/issues/GL-CHR-WOR--42.md",
            env!("CARGO_MANIFEST_DIR")
        ),
        repo.join("issues/GL-CHR-WOR--42.md"),
    )
    .expect("copy issue");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("summarize")
        .assert()
        .success()
        .stdout(predicate::str::contains("GL-CHR-WOR--42"));
}
