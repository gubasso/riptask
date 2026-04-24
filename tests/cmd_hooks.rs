use assert_cmd::Command;
use predicates::prelude::*;
use riptask::assets::hook::{HOOK_VERSION, PRE_COMMIT_HOOK};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use tempfile::tempdir;

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
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

    (temp, repo, cache)
}

#[test]
fn hooks_status_reports_not_installed() {
    let (_temp, repo, cache) = init_repo();

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "status"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("not installed"));
}

#[test]
fn hooks_install_creates_hook_file() {
    let (_temp, repo, cache) = init_repo();
    let hook_path = repo.join(".git/hooks/pre-commit");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "install"])
        .assert()
        .success();

    assert!(hook_path.exists(), "expected hook file to exist");
    assert_eq!(
        fs::read_to_string(&hook_path).expect("read hook"),
        PRE_COMMIT_HOOK
    );
    #[cfg(unix)]
    {
        let mode = fs::metadata(&hook_path)
            .expect("hook metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "expected executable bit to be set");
    }
}

#[test]
fn hooks_status_reports_installed_after_install() {
    let (_temp, repo, cache) = init_repo();

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "install"])
        .assert()
        .success();

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "status"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("up to date")
                .and(predicate::str::contains(format!("version: {HOOK_VERSION}"))),
        );
}

#[test]
fn hooks_update_restores_modified_hook() {
    let (_temp, repo, cache) = init_repo();
    let hook_path = repo.join(".git/hooks/pre-commit");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "install"])
        .assert()
        .success();

    fs::write(&hook_path, "corrupted content").expect("write corrupted hook");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "update"])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(&hook_path).expect("read updated hook"),
        PRE_COMMIT_HOOK
    );
}

#[test]
fn hooks_update_errors_when_not_installed() {
    let (_temp, repo, cache) = init_repo();

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "update"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("hook not installed"));
}

#[test]
fn hooks_uninstall_removes_hook() {
    let (_temp, repo, cache) = init_repo();
    let hook_path = repo.join(".git/hooks/pre-commit");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "install"])
        .assert()
        .success();

    assert!(
        hook_path.exists(),
        "expected hook file to exist before uninstall"
    );

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "uninstall"])
        .assert()
        .success();

    assert!(
        !hook_path.exists(),
        "expected hook file to be removed by uninstall"
    );
}

#[test]
fn hooks_uninstall_is_idempotent() {
    let (_temp, repo, cache) = init_repo();

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["store", "hooks", "uninstall"])
        .assert()
        .success();
}
