use crate::error::RiptskError;
use std::path::Path;
use std::process::Command;

pub trait GitBackend {
    fn init(&self, path: &Path) -> Result<(), RiptskError>;
    fn add(&self, repo: &Path, files: &[&Path]) -> Result<(), RiptskError>;
    fn commit(&self, repo: &Path, message: &str) -> Result<(), RiptskError>;
    fn has_changes(&self, repo: &Path) -> Result<bool, RiptskError>;
    fn pull(&self, repo: &Path) -> Result<(), RiptskError>;
    fn push(&self, repo: &Path) -> Result<(), RiptskError>;
    fn checkout(&self, repo: &Path, branch: &str) -> Result<(), RiptskError>;
    fn create_branch(&self, repo: &Path, name: &str) -> Result<(), RiptskError>;
    fn branch_exists(&self, repo: &Path, name: &str) -> Result<bool, RiptskError>;
    fn fetch_and_checkout_tracking(&self, repo: &Path, branch: &str) -> Result<(), RiptskError>;
    fn push_with_upstream(&self, repo: &Path, branch: &str) -> Result<(), RiptskError>;
    fn has_working_tree_changes(&self, repo: &Path) -> Result<bool, RiptskError>;
    fn diff_names(&self, repo: &Path) -> Result<Vec<String>, RiptskError>;
    fn current_branch(&self, repo: &Path) -> Result<String, RiptskError>;
    fn remote_url(&self, repo: &Path, remote: &str) -> Result<String, RiptskError>;
    fn delete_local_branch(
        &self,
        repo: &Path,
        branch: &str,
        force: bool,
    ) -> Result<(), RiptskError>;
    fn is_branch_merged(&self, repo: &Path, branch: &str, base: &str) -> Result<bool, RiptskError>;
    fn fetch(&self, repo: &Path) -> Result<(), RiptskError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CliGit;

impl GitBackend for CliGit {
    fn init(&self, path: &Path) -> Result<(), RiptskError> {
        run_git(path, ["init"])
    }

    fn add(&self, repo: &Path, files: &[&Path]) -> Result<(), RiptskError> {
        let mut command = Command::new("git");
        command.arg("-C").arg(repo).arg("add");
        for file in files {
            command.arg(file);
        }
        status_to_result(command.status()?)
    }

    fn commit(&self, repo: &Path, message: &str) -> Result<(), RiptskError> {
        run_git(repo, ["commit", "-m", message])
    }

    fn has_changes(&self, repo: &Path) -> Result<bool, RiptskError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["status", "--porcelain"])
            .output()?;
        if !output.status.success() {
            return Err(RiptskError::General("failed to inspect git status".into()));
        }
        Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    }

    fn pull(&self, repo: &Path) -> Result<(), RiptskError> {
        run_git(repo, ["pull"])
    }

    fn push(&self, repo: &Path) -> Result<(), RiptskError> {
        run_git(repo, ["push"])
    }

    fn checkout(&self, repo: &Path, branch: &str) -> Result<(), RiptskError> {
        run_git_dynamic(repo, ["checkout", branch].as_slice())
    }

    fn create_branch(&self, repo: &Path, name: &str) -> Result<(), RiptskError> {
        run_git(repo, ["checkout", "-b", name])
    }

    fn branch_exists(&self, repo: &Path, name: &str) -> Result<bool, RiptskError> {
        let refname = format!("refs/heads/{name}");
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", &refname])
            .status()?;
        Ok(status.success())
    }

    fn fetch_and_checkout_tracking(&self, repo: &Path, branch: &str) -> Result<(), RiptskError> {
        run_git(repo, ["fetch", "origin", branch])?;
        let tracking = format!("origin/{branch}");
        run_git_dynamic(repo, &["checkout", "-b", branch, "--track", &tracking])
    }

    fn push_with_upstream(&self, repo: &Path, branch: &str) -> Result<(), RiptskError> {
        run_git(repo, ["push", "-u", "origin", branch])
    }

    fn has_working_tree_changes(&self, repo: &Path) -> Result<bool, RiptskError> {
        self.has_changes(repo)
    }

    fn diff_names(&self, repo: &Path) -> Result<Vec<String>, RiptskError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["diff", "--name-only"])
            .output()?;
        if !output.status.success() {
            return Err(RiptskError::General("failed to inspect git diff".into()));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(ToOwned::to_owned)
            .collect())
    }

    fn current_branch(&self, repo: &Path) -> Result<String, RiptskError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("rev-parse")
            .arg("--abbrev-ref")
            .arg("HEAD")
            .output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(RiptskError::General(
                "failed to determine current branch".into(),
            ))
        }
    }

    fn remote_url(&self, repo: &Path, remote: &str) -> Result<String, RiptskError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("remote")
            .arg("get-url")
            .arg(remote)
            .output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(RiptskError::General(format!(
                "failed to read git remote {remote}"
            )))
        }
    }

    fn delete_local_branch(
        &self,
        repo: &Path,
        branch: &str,
        force: bool,
    ) -> Result<(), RiptskError> {
        let flag = if force { "-D" } else { "-d" };
        run_git_dynamic(repo, &["branch", flag, branch])
    }

    fn is_branch_merged(&self, repo: &Path, branch: &str, base: &str) -> Result<bool, RiptskError> {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["merge-base", "--is-ancestor", branch, base])
            .status()?;
        Ok(status.success())
    }

    fn fetch(&self, repo: &Path) -> Result<(), RiptskError> {
        run_git(repo, ["fetch", "origin"])
    }
}

fn run_git<const N: usize>(repo: &Path, args: [&str; N]) -> Result<(), RiptskError> {
    run_git_dynamic(repo, &args)
}

fn run_git_dynamic(repo: &Path, args: &[&str]) -> Result<(), RiptskError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo);
    for arg in args {
        command.arg(arg);
    }
    status_to_result(command.status()?)
}

fn status_to_result(status: std::process::ExitStatus) -> Result<(), RiptskError> {
    if status.success() {
        Ok(())
    } else {
        Err(RiptskError::General("git command failed".into()))
    }
}
