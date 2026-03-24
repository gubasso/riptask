use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn path_resolves_numeric_id_from_detected_project() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let worktree = temp.path().join("worktree");
    fs::create_dir_all(&worktree).expect("worktree dir");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    fs::write(
        repo.join("riptsk.yaml"),
        config_with_backend_path(&worktree),
    )
    .expect("config");
    fs::write(repo.join("issues/GH-GUB-DEV--61.md"), "").expect("issue");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&worktree)
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["path", "61"])
        .assert()
        .success()
        .stdout(predicate::str::contains("GH-GUB-DEV--61.md"));
}

#[test]
fn path_resolves_numeric_id_by_unique_suffix_without_detected_project() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).expect("outside dir");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

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
        .current_dir(&outside)
        .env("RIPTSK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["path", "42"])
        .assert()
        .success()
        .stdout(predicate::str::contains("GL-CHR-WOR--42.md"));
}

fn config_with_backend_path(path: &std::path::Path) -> String {
    format!(
        "version: 1
defaults:
  board: personal
  status: todo
  priority: medium
  assignee: ~
  template: task
backends:
  - name: dev-tools
    type: github
    repo: GubCorp/dev-tools
    default_board: personal
    default_org: ~
    path: {}
boards:
  - name: personal
    statuses: [backlog, todo, in-progress, review, done]
ui:
  opener: \"nvim -R\"
  tree_depth: 2
  fzf_opts: \"--border\"
ai:
  enabled: false
  features:
    new_body_gen: true
    triage: true
    summarize: true
    ask: true
sync:
  conflict_detection: true
recurring: []
",
        path.display()
    )
}
