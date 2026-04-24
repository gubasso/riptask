use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn view_builds_kanban_tree() {
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
    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(temp.path())
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["new", "--title", "Build views"])
        .assert()
        .success();
    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("view")
        .assert()
        .success();
    assert!(cache.join("riptask/views/kanban/personal/todo").exists());
}

#[test]
fn board_respects_explicit_multi_project_scope() {
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

    // Override tree_depth so board output includes issue filenames
    let config =
        include_str!("fixtures/config_multi.yaml").replace("tree_depth: 2", "tree_depth: 4");
    fs::write(repo.join("config.yaml"), config).expect("write config");
    fs::copy(
        format!(
            "{}/tests/fixtures/issues/GL-CHR-WOR--42.md",
            env!("CARGO_MANIFEST_DIR")
        ),
        repo.join("issues/GL-CHR-WOR--42.md"),
    )
    .expect("copy wormhole fixture");
    fs::copy(
        format!(
            "{}/tests/fixtures/issues/GH-PEN-FIS--17.md",
            env!("CARGO_MANIFEST_DIR")
        ),
        repo.join("issues/GH-PEN-FIS--17.md"),
    )
    .expect("copy github fixture");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args([
            "new",
            "--title",
            "Scoped scratch issue",
            "--project",
            "penguin-scratch",
            "--board",
            "personal",
            "--status",
            "todo",
            "--priority",
            "low",
            "--template",
            "task",
        ])
        .assert()
        .success();

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args([
            "board",
            "--all",
            "--project",
            "penguin-scratch",
            "--project",
            "wormhole-router",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("LO-PEN--1")
                .and(predicate::str::contains("GL-CHR-WOR--42"))
                .and(predicate::str::contains("GH-PEN-FIS--17").not()),
        );
}
