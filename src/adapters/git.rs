use crate::error::TskError;
use std::path::Path;
use std::process::Command;

pub trait GitBackend {
    fn init(&self, path: &Path) -> Result<(), TskError>;
    fn add(&self, repo: &Path, files: &[&Path]) -> Result<(), TskError>;
    fn commit(&self, repo: &Path, message: &str) -> Result<(), TskError>;
    fn has_changes(&self, repo: &Path) -> Result<bool, TskError>;
    fn pull(&self, repo: &Path) -> Result<(), TskError>;
    fn push(&self, repo: &Path) -> Result<(), TskError>;
    fn checkout(&self, repo: &Path, branch: &str) -> Result<(), TskError>;
    fn create_branch(&self, repo: &Path, name: &str) -> Result<(), TskError>;
    fn branch_exists(&self, repo: &Path, name: &str) -> Result<bool, TskError>;
    fn push_with_upstream(&self, repo: &Path, branch: &str) -> Result<(), TskError>;
    fn has_working_tree_changes(&self, repo: &Path) -> Result<bool, TskError>;
    fn diff_names(&self, repo: &Path) -> Result<Vec<String>, TskError>;
    fn current_branch(&self, repo: &Path) -> Result<String, TskError>;
    fn remote_url(&self, repo: &Path, remote: &str) -> Result<String, TskError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CliGit;

impl GitBackend for CliGit {
    fn init(&self, path: &Path) -> Result<(), TskError> {
        run_git(path, ["init"])
    }

    fn add(&self, repo: &Path, files: &[&Path]) -> Result<(), TskError> {
        let mut command = Command::new("git");
        command.arg("-C").arg(repo).arg("add");
        for file in files {
            command.arg(file);
        }
        status_to_result(command.status()?)
    }

    fn commit(&self, repo: &Path, message: &str) -> Result<(), TskError> {
        run_git(repo, ["commit", "-m", message])
    }

    fn has_changes(&self, repo: &Path) -> Result<bool, TskError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["status", "--porcelain"])
            .output()?;
        if !output.status.success() {
            return Err(TskError::General("failed to inspect git status".into()));
        }
        Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    }

    fn pull(&self, repo: &Path) -> Result<(), TskError> {
        run_git(repo, ["pull"])
    }

    fn push(&self, repo: &Path) -> Result<(), TskError> {
        run_git(repo, ["push"])
    }

    fn checkout(&self, repo: &Path, branch: &str) -> Result<(), TskError> {
        run_git_dynamic(repo, ["checkout", branch].as_slice())
    }

    fn create_branch(&self, repo: &Path, name: &str) -> Result<(), TskError> {
        run_git(repo, ["checkout", "-b", name])
    }

    fn branch_exists(&self, repo: &Path, name: &str) -> Result<bool, TskError> {
        let status = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", name])
            .status()?;
        Ok(status.success())
    }

    fn push_with_upstream(&self, repo: &Path, branch: &str) -> Result<(), TskError> {
        run_git(repo, ["push", "-u", "origin", branch])
    }

    fn has_working_tree_changes(&self, repo: &Path) -> Result<bool, TskError> {
        self.has_changes(repo)
    }

    fn diff_names(&self, repo: &Path) -> Result<Vec<String>, TskError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["diff", "--name-only"])
            .output()?;
        if !output.status.success() {
            return Err(TskError::General("failed to inspect git diff".into()));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(ToOwned::to_owned)
            .collect())
    }

    fn current_branch(&self, repo: &Path) -> Result<String, TskError> {
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
            Err(TskError::General(
                "failed to determine current branch".into(),
            ))
        }
    }

    fn remote_url(&self, repo: &Path, remote: &str) -> Result<String, TskError> {
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
            Err(TskError::General(format!(
                "failed to read git remote {remote}"
            )))
        }
    }
}

fn run_git<const N: usize>(repo: &Path, args: [&str; N]) -> Result<(), TskError> {
    run_git_dynamic(repo, &args)
}

fn run_git_dynamic(repo: &Path, args: &[&str]) -> Result<(), TskError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo);
    for arg in args {
        command.arg(arg);
    }
    status_to_result(command.status()?)
}

fn status_to_result(status: std::process::ExitStatus) -> Result<(), TskError> {
    if status.success() {
        Ok(())
    } else {
        Err(TskError::General("git command failed".into()))
    }
}
