use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::process::Command as StdCommand;
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
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    fs::write(
        repo.join("riptask.yaml"),
        config_with_backend_path(&worktree),
    )
    .expect("config");
    fs::write(repo.join("issues/GH-GUB-DEV--61.md"), "").expect("issue");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&worktree)
        .env("RIPTASK_REPO", &repo)
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
        .env("RIPTASK_REPO", &repo)
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
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .args(["path", "42"])
        .assert()
        .success()
        .stdout(predicate::str::contains("GL-CHR-WOR--42.md"));
}

#[test]
fn id_prints_issue_id_for_current_branch() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let worktree = temp.path().join("worktree");
    fs::create_dir_all(&worktree).expect("worktree dir");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    init_worktree_branch(&worktree, "feature/hello");

    fs::write(
        repo.join("riptask.yaml"),
        config_with_backend_path(&worktree),
    )
    .expect("config");
    write_issue_with_branch(&repo, "GH-GUB-DEV--61", "feature/hello");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&worktree)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("id")
        .assert()
        .success()
        .stdout(predicate::eq("GH-GUB-DEV--61\n"));
}

#[test]
fn id_errors_when_current_branch_has_no_issue() {
    let temp = tempdir().expect("temp dir");
    let repo = temp.path().join("repo");
    let cache = temp.path().join("cache");
    let worktree = temp.path().join("worktree");
    fs::create_dir_all(&worktree).expect("worktree dir");

    Command::cargo_bin("tsk")
        .expect("binary")
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("init")
        .assert()
        .success();

    init_worktree_branch(&worktree, "feature/missing");

    fs::write(
        repo.join("riptask.yaml"),
        config_with_backend_path(&worktree),
    )
    .expect("config");

    Command::cargo_bin("tsk")
        .expect("binary")
        .current_dir(&worktree)
        .env("RIPTASK_REPO", &repo)
        .env("XDG_CACHE_HOME", &cache)
        .arg("id")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no issue found for branch"));
}

fn init_worktree_branch(worktree: &std::path::Path, branch: &str) {
    let status = StdCommand::new("git")
        .arg("-c")
        .arg("init.defaultBranch=main")
        .arg("init")
        .arg("--quiet")
        .arg(worktree)
        .status()
        .expect("git init");
    assert!(status.success());

    let status = StdCommand::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("-c")
        .arg("user.name=Test User")
        .arg("-c")
        .arg("user.email=test@example.com")
        .arg("commit")
        .arg("--allow-empty")
        .arg("-m")
        .arg("init")
        .status()
        .expect("git commit");
    assert!(status.success());

    let status = StdCommand::new("git")
        .arg("-C")
        .arg(worktree)
        .arg("checkout")
        .arg("-b")
        .arg(branch)
        .status()
        .expect("git checkout");
    assert!(status.success());
}

fn write_issue_with_branch(repo: &std::path::Path, id: &str, branch: &str) {
    let contents = format!(
        "---\nid: {id}\ntitle: Branch issue\nstatus: in-progress\nboard: personal\nproject: dev-tools\norg: ~\npriority: medium\nlabels: []\nassignees: []\nmilestone: ~\ncycle: ~\norder: 1\nbranch: {branch}\ngithub: ~\ngitlab: ~\njira: ~\nlocal_updated_at: 2026-03-13T11:05:00Z\ndue: ~\nrecurring: ~\nremote_deleted: false\n---\n\nTest body.\n"
    );
    fs::write(repo.join(format!("issues/{id}.md")), contents).expect("issue");
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
projects:
  - name: dev-tools
    vc_backend:
      type: github
      repo: GubCorp/dev-tools
      path: {}
    tasks_backend:
      type: github
      repo: GubCorp/dev-tools
    key: GH-GUB-DEV
    default_board: personal
    default_org: ~
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
