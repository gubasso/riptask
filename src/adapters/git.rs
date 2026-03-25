use crate::error::RiptskError;
use crate::services::backend_mapping::GitHttpAuth;
use std::io::Write;
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
    fn has_staged_changes(&self, repo: &Path) -> Result<bool, RiptskError>;
    fn is_branch_merged(&self, repo: &Path, branch: &str, base: &str) -> Result<bool, RiptskError>;
    fn fetch(&self, repo: &Path) -> Result<(), RiptskError>;
    fn commits_ahead_of_base(
        &self,
        repo: &Path,
        branch: &str,
        base: &str,
    ) -> Result<u64, RiptskError>;
    fn create_empty_commit(&self, repo: &Path, message: &str) -> Result<(), RiptskError>;
    fn log_between(&self, repo: &Path, base: &str, head: &str) -> Result<String, RiptskError>;
    fn diff_between(&self, repo: &Path, base: &str, head: &str) -> Result<String, RiptskError>;
}

#[derive(Debug, Clone, Default)]
pub struct CliGit {
    auth: Option<GitHttpAuth>,
}

impl CliGit {
    pub fn new() -> Self {
        Self { auth: None }
    }

    pub fn with_auth(auth: Option<GitHttpAuth>) -> Self {
        Self { auth }
    }

    fn run_git_remote(&self, repo: &Path, args: &[&str]) -> Result<(), RiptskError> {
        let Some(auth) = self.auth.as_ref() else {
            return run_git_dynamic(repo, args);
        };

        let mut askpass = tempfile::NamedTempFile::new().map_err(RiptskError::Io)?;
        askpass
            .write_all(build_askpass_script(&auth.token).as_bytes())
            .map_err(RiptskError::Io)?;
        askpass.flush().map_err(RiptskError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(askpass.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(RiptskError::Io)?;
        }
        // Close the write fd so the OS allows exec (avoids ETXTBSY on Linux).
        let askpass_path = askpass.into_temp_path();

        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(repo)
            .env("GIT_ASKPASS", &askpass_path)
            .env("GIT_TERMINAL_PROMPT", "0");
        for arg in args {
            command.arg(arg);
        }
        status_to_result(command.status()?)
    }
}

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
        self.run_git_remote(repo, &["pull"])
    }

    fn push(&self, repo: &Path) -> Result<(), RiptskError> {
        self.run_git_remote(repo, &["push"])
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
        self.run_git_remote(repo, &["fetch", "origin", branch])?;
        let tracking = format!("origin/{branch}");
        run_git_dynamic(repo, &["checkout", "-b", branch, "--track", &tracking])
    }

    fn push_with_upstream(&self, repo: &Path, branch: &str) -> Result<(), RiptskError> {
        self.run_git_remote(repo, &["push", "-u", "origin", branch])
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

    fn has_staged_changes(&self, repo: &Path) -> Result<bool, RiptskError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["diff", "--cached", "--quiet"])
            .status()?;
        // exit 0 = no staged changes, exit 1 = staged changes exist
        Ok(!output.success())
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
        self.run_git_remote(repo, &["fetch", "origin"])
    }

    fn commits_ahead_of_base(
        &self,
        repo: &Path,
        branch: &str,
        base: &str,
    ) -> Result<u64, RiptskError> {
        let range = format!("origin/{base}..{branch}");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-list", "--count", &range])
            .output()?;
        if !output.status.success() {
            return Err(RiptskError::General(
                "failed to inspect commits ahead of base".into(),
            ));
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .map_err(|error| {
                RiptskError::General(format!("failed to parse commit count for {range}: {error}"))
            })
    }

    fn create_empty_commit(&self, repo: &Path, message: &str) -> Result<(), RiptskError> {
        let mut args: Vec<&str> = vec!["commit", "--allow-empty"];
        for part in message.split("\n\n") {
            args.push("-m");
            args.push(part);
        }
        run_git_dynamic(repo, &args)
    }

    fn log_between(&self, repo: &Path, base: &str, head: &str) -> Result<String, RiptskError> {
        let range = format!("origin/{base}..{head}");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["log", "--oneline", &range])
            .output()?;
        if !output.status.success() {
            return Err(RiptskError::General("failed to read git log".into()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn diff_between(&self, repo: &Path, base: &str, head: &str) -> Result<String, RiptskError> {
        let range = format!("origin/{base}..{head}");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["diff", &range])
            .output()?;
        if !output.status.success() {
            return Err(RiptskError::General("failed to read git diff".into()));
        }
        let mut diff = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        const MAX_DIFF_CHARS: usize = 8000;
        if diff.len() > MAX_DIFF_CHARS {
            diff.truncate(MAX_DIFF_CHARS);
        }
        Ok(diff)
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

fn build_askpass_script(token: &str) -> String {
    let escaped = shell_escape::escape(token.into());
    format!(
        "#!/bin/sh\ncase \"$1\" in\n  Username*) echo \"oauth2\" ;;\n  *) echo {escaped} ;;\nesac\n"
    )
}

#[cfg(test)]
mod tests {
    use super::build_askpass_script;

    #[test]
    fn builds_askpass_script_with_shell_escaped_token() {
        let script = build_askpass_script("glpat-token'with-quote");

        assert_eq!(
            script,
            "#!/bin/sh\ncase \"$1\" in\n  Username*) echo \"oauth2\" ;;\n  *) echo 'glpat-token'\\''with-quote' ;;\nesac\n"
        );
    }
}
