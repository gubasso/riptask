use crate::error::RiptaskError;
use crate::services::backend_mapping::GitHttpAuth;
use std::io::Write;
use std::path::Path;
use std::process::Command;

pub trait GitBackend {
    fn init(&self, path: &Path) -> Result<(), RiptaskError>;
    fn add(&self, repo: &Path, files: &[&Path]) -> Result<(), RiptaskError>;
    fn commit(&self, repo: &Path, message: &str) -> Result<(), RiptaskError>;
    fn repo_root(&self, cwd: &Path) -> Result<std::path::PathBuf, RiptaskError>;
    fn merge_file(&self, local: &Path, base: &Path, remote: &Path) -> Result<String, RiptaskError>;
    fn has_changes(&self, repo: &Path) -> Result<bool, RiptaskError>;
    fn has_uncommitted_changes(&self, repo: &Path) -> Result<bool, RiptaskError>;
    fn pull(&self, repo: &Path) -> Result<(), RiptaskError>;
    fn push(&self, repo: &Path) -> Result<(), RiptaskError>;
    fn checkout(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError>;
    fn create_branch(&self, repo: &Path, name: &str) -> Result<(), RiptaskError>;
    fn branch_exists(&self, repo: &Path, name: &str) -> Result<bool, RiptaskError>;
    fn fetch_and_checkout_tracking(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError>;
    fn push_with_upstream(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError>;
    fn has_working_tree_changes(&self, repo: &Path) -> Result<bool, RiptaskError>;
    fn diff_names(&self, repo: &Path) -> Result<Vec<String>, RiptaskError>;
    fn current_branch(&self, repo: &Path) -> Result<String, RiptaskError>;
    fn remote_url(&self, repo: &Path, remote: &str) -> Result<String, RiptaskError>;
    fn delete_local_branch(
        &self,
        repo: &Path,
        branch: &str,
        force: bool,
    ) -> Result<(), RiptaskError>;
    fn has_staged_changes(&self, repo: &Path) -> Result<bool, RiptaskError>;
    fn stage_all(&self, repo: &Path) -> Result<(), RiptaskError>;
    fn is_branch_merged(&self, repo: &Path, branch: &str, base: &str)
    -> Result<bool, RiptaskError>;
    fn fetch(&self, repo: &Path) -> Result<(), RiptaskError>;
    fn commits_ahead_of_base(
        &self,
        repo: &Path,
        branch: &str,
        base: &str,
    ) -> Result<u64, RiptaskError>;
    fn create_empty_commit(&self, repo: &Path, message: &str) -> Result<(), RiptaskError>;
    fn find_commit_by_subject(
        &self,
        repo: &Path,
        branch: &str,
        base: &str,
        subject: &str,
    ) -> Result<Option<String>, RiptaskError>;
    fn rebase_drop_commit(
        &self,
        repo: &Path,
        commit_sha: &str,
        branch: &str,
    ) -> Result<(), RiptaskError>;
    fn force_push_with_lease(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError>;
    fn force_push(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError>;
    fn log_between(&self, repo: &Path, base: &str, head: &str) -> Result<String, RiptaskError>;
    fn diff_between(&self, repo: &Path, base: &str, head: &str) -> Result<String, RiptaskError>;
    fn working_tree_diff(&self, repo: &Path) -> Result<String, RiptaskError>;
    fn staged_diff(&self, repo: &Path) -> Result<String, RiptaskError>;
    fn head_sha(&self, repo: &Path) -> Result<String, RiptaskError>;
    fn clone_with_reference(
        &self,
        reference_repo: &Path,
        remote_url: &str,
        target_dir: &Path,
    ) -> Result<(), RiptaskError>;
    /// Stash uncommitted changes (staged, unstaged, and untracked) with a message.
    /// Returns `true` if a stash entry was created, `false` if there was nothing to stash.
    fn stash_push(&self, repo: &Path, message: &str) -> Result<bool, RiptaskError>;
    /// Pop the most recent stash entry.
    /// Returns `true` on clean apply, `false` if conflicts occurred (stash is preserved).
    fn stash_pop(&self, repo: &Path) -> Result<bool, RiptaskError>;
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

    fn run_git_remote(&self, repo: &Path, args: &[&str]) -> Result<(), RiptaskError> {
        let Some(auth) = self.auth.as_ref() else {
            return run_git_dynamic(repo, args);
        };

        let mut askpass = tempfile::NamedTempFile::new().map_err(RiptaskError::Io)?;
        askpass
            .write_all(build_askpass_script(&auth.token).as_bytes())
            .map_err(RiptaskError::Io)?;
        askpass.flush().map_err(RiptaskError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(askpass.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(RiptaskError::Io)?;
        }
        // Close the write fd so the OS allows exec (avoids ETXTBSY on Linux).
        let askpass_path = askpass.into_temp_path();

        let mut command = git_command();
        command
            .arg("-C")
            .arg(repo)
            .env("GIT_ASKPASS", &askpass_path)
            .env("GIT_TERMINAL_PROMPT", "0");
        for arg in args {
            command.arg(arg);
        }
        status_to_result(command.output()?.status)
    }

    fn prepare_auth_command(&self) -> Result<(Command, Option<tempfile::TempPath>), RiptaskError> {
        let mut command = git_command();
        let Some(auth) = self.auth.as_ref() else {
            return Ok((command, None));
        };

        let mut askpass = tempfile::NamedTempFile::new().map_err(RiptaskError::Io)?;
        askpass
            .write_all(build_askpass_script(&auth.token).as_bytes())
            .map_err(RiptaskError::Io)?;
        askpass.flush().map_err(RiptaskError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(askpass.path(), std::fs::Permissions::from_mode(0o700))
                .map_err(RiptaskError::Io)?;
        }
        let askpass_path = askpass.into_temp_path();
        command
            .env("GIT_ASKPASS", &askpass_path)
            .env("GIT_TERMINAL_PROMPT", "0");
        Ok((command, Some(askpass_path)))
    }
}

impl GitBackend for CliGit {
    fn init(&self, path: &Path) -> Result<(), RiptaskError> {
        run_git(path, ["init"])
    }

    fn repo_root(&self, cwd: &Path) -> Result<std::path::PathBuf, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--show-toplevel"])
            .output()?;
        if output.status.success() {
            Ok(std::path::PathBuf::from(
                String::from_utf8_lossy(&output.stdout).trim(),
            ))
        } else {
            Err(RiptaskError::General(
                "failed to determine git repository root".into(),
            ))
        }
    }

    fn add(&self, repo: &Path, files: &[&Path]) -> Result<(), RiptaskError> {
        let mut command = git_command();
        command.arg("-C").arg(repo).arg("add");
        for file in files {
            command.arg(file);
        }
        status_to_result(command.output()?.status)
    }

    fn commit(&self, repo: &Path, message: &str) -> Result<(), RiptaskError> {
        run_git(repo, ["commit", "-m", message])
    }

    fn merge_file(&self, local: &Path, base: &Path, remote: &Path) -> Result<String, RiptaskError> {
        let output = git_command()
            .args([
                "merge-file",
                "-p",
                "-L",
                "LOCAL",
                "-L",
                "BASE",
                "-L",
                "REMOTE",
            ])
            .arg(local)
            .arg(base)
            .arg(remote)
            .output()?;
        match output.status.code() {
            Some(code) if code <= 127 => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
            _ => Err(RiptaskError::General(format!(
                "git merge-file failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))),
        }
    }

    fn has_changes(&self, repo: &Path) -> Result<bool, RiptaskError> {
        self.has_uncommitted_changes(repo)
    }

    fn has_uncommitted_changes(&self, repo: &Path) -> Result<bool, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["status", "--porcelain"])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::General("failed to inspect git status".into()));
        }
        Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    }

    fn pull(&self, repo: &Path) -> Result<(), RiptaskError> {
        self.run_git_remote(repo, &["pull"])
    }

    fn push(&self, repo: &Path) -> Result<(), RiptaskError> {
        self.run_git_remote(repo, &["push"])
    }

    fn checkout(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError> {
        run_git_dynamic(repo, ["checkout", branch].as_slice())
    }

    fn create_branch(&self, repo: &Path, name: &str) -> Result<(), RiptaskError> {
        run_git(repo, ["checkout", "-b", name])
    }

    fn branch_exists(&self, repo: &Path, name: &str) -> Result<bool, RiptaskError> {
        let refname = format!("refs/heads/{name}");
        let status = git_command()
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "--verify", &refname])
            .status()?;
        Ok(status.success())
    }

    fn fetch_and_checkout_tracking(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError> {
        self.run_git_remote(repo, &["fetch", "origin", branch])?;
        let tracking = format!("origin/{branch}");
        run_git_dynamic(repo, &["checkout", "-b", branch, "--track", &tracking])
    }

    fn push_with_upstream(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError> {
        self.run_git_remote(repo, &["push", "-u", "origin", branch])
    }

    fn has_working_tree_changes(&self, repo: &Path) -> Result<bool, RiptaskError> {
        self.has_changes(repo)
    }

    fn diff_names(&self, repo: &Path) -> Result<Vec<String>, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["diff", "--name-only"])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::General("failed to inspect git diff".into()));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(ToOwned::to_owned)
            .collect())
    }

    fn current_branch(&self, repo: &Path) -> Result<String, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .arg("rev-parse")
            .arg("--abbrev-ref")
            .arg("HEAD")
            .output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(RiptaskError::General(
                "failed to determine current branch".into(),
            ))
        }
    }

    fn remote_url(&self, repo: &Path, remote: &str) -> Result<String, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .arg("remote")
            .arg("get-url")
            .arg(remote)
            .output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(RiptaskError::General(format!(
                "failed to read git remote {remote}"
            )))
        }
    }

    fn head_sha(&self, repo: &Path) -> Result<String, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .arg("rev-parse")
            .arg("HEAD")
            .output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(RiptaskError::General("failed to determine HEAD SHA".into()))
        }
    }

    fn delete_local_branch(
        &self,
        repo: &Path,
        branch: &str,
        force: bool,
    ) -> Result<(), RiptaskError> {
        let flag = if force { "-D" } else { "-d" };
        run_git_dynamic(repo, &["branch", flag, branch])
    }

    fn has_staged_changes(&self, repo: &Path) -> Result<bool, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["diff", "--cached", "--quiet"])
            .status()?;
        // exit 0 = no staged changes, exit 1 = staged changes exist
        Ok(!output.success())
    }

    fn stage_all(&self, repo: &Path) -> Result<(), RiptaskError> {
        run_git_dynamic(repo, &["add", "-A"])
    }

    fn is_branch_merged(
        &self,
        repo: &Path,
        branch: &str,
        base: &str,
    ) -> Result<bool, RiptaskError> {
        let status = git_command()
            .arg("-C")
            .arg(repo)
            .args(["merge-base", "--is-ancestor", branch, base])
            .status()?;
        Ok(status.success())
    }

    fn fetch(&self, repo: &Path) -> Result<(), RiptaskError> {
        self.run_git_remote(repo, &["fetch", "origin"])
    }

    fn commits_ahead_of_base(
        &self,
        repo: &Path,
        branch: &str,
        base: &str,
    ) -> Result<u64, RiptaskError> {
        let range = format!("origin/{base}..{branch}");
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["rev-list", "--count", &range])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::General(
                "failed to inspect commits ahead of base".into(),
            ));
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .map_err(|error| {
                RiptaskError::General(format!("failed to parse commit count for {range}: {error}"))
            })
    }

    fn create_empty_commit(&self, repo: &Path, message: &str) -> Result<(), RiptaskError> {
        let mut args: Vec<&str> = vec!["commit", "--allow-empty"];
        for part in message.split("\n\n") {
            args.push("-m");
            args.push(part);
        }
        run_git_dynamic(repo, &args)
    }

    fn find_commit_by_subject(
        &self,
        repo: &Path,
        branch: &str,
        base: &str,
        subject: &str,
    ) -> Result<Option<String>, RiptaskError> {
        let range = format!("origin/{base}..{branch}");
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["log", "--format=%H%n%s", &range])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::General(
                "failed to inspect commit subjects".into(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        while let Some(sha) = lines.next() {
            let Some(commit_subject) = lines.next() else {
                break;
            };
            if commit_subject == subject {
                return Ok(Some(sha.to_owned()));
            }
        }

        Ok(None)
    }

    fn rebase_drop_commit(
        &self,
        repo: &Path,
        commit_sha: &str,
        branch: &str,
    ) -> Result<(), RiptaskError> {
        let onto = format!("{commit_sha}^");
        let status = git_command()
            .arg("-C")
            .arg(repo)
            .args(["rebase", "--onto", &onto, commit_sha, branch])
            .status()?;
        if status.success() {
            return Ok(());
        }

        let _ = run_git_dynamic(repo, &["rebase", "--abort"]);
        Err(RiptaskError::General(format!(
            "failed to drop commit {commit_sha} via rebase"
        )))
    }

    fn force_push_with_lease(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError> {
        self.run_git_remote(repo, &["push", "--force-with-lease", "origin", branch])
    }

    fn force_push(&self, repo: &Path, branch: &str) -> Result<(), RiptaskError> {
        self.run_git_remote(repo, &["push", "--force", "origin", branch])
    }

    fn log_between(&self, repo: &Path, base: &str, head: &str) -> Result<String, RiptaskError> {
        let range = format!("origin/{base}..{head}");
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["log", "--oneline", &range])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::General("failed to read git log".into()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn diff_between(&self, repo: &Path, base: &str, head: &str) -> Result<String, RiptaskError> {
        let range = format!("origin/{base}..{head}");
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["diff", &range])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::General("failed to read git diff".into()));
        }
        Ok(truncate_diff(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ))
    }

    fn working_tree_diff(&self, repo: &Path) -> Result<String, RiptaskError> {
        // Tracked changes vs HEAD (or staged when no HEAD exists yet).
        let tracked = if has_head_commit(repo)? {
            git_command()
                .arg("-C")
                .arg(repo)
                .args(["diff", "HEAD"])
                .output()?
        } else {
            git_command()
                .arg("-C")
                .arg(repo)
                .args(["diff", "--cached"])
                .output()?
        };
        if !tracked.status.success() {
            return Err(RiptaskError::General(
                "failed to read working tree git diff".into(),
            ));
        }
        let mut combined = String::from_utf8_lossy(&tracked.stdout).into_owned();

        // Untracked files: `git diff HEAD` ignores them, so synthesize a diff
        // against /dev/null for each so AI flows see new files too. Stop once
        // we're past the truncation budget — no point spawning more subprocesses
        // for content truncate_diff will drop.
        for path in list_untracked_files(repo)? {
            if combined.len() >= MAX_DIFF_CHARS {
                break;
            }
            let abs = repo.join(&path);
            let untracked = git_command()
                .arg("-C")
                .arg(repo)
                .args(["diff", "--no-index", "--"])
                .arg("/dev/null")
                .arg(&abs)
                .output()?;
            // `git diff --no-index` exits 0 when files are identical and 1 when
            // they differ; both are success for us.
            match untracked.status.code() {
                Some(0) | Some(1) => {}
                _ => {
                    return Err(RiptaskError::General(
                        "failed to read untracked file diff".into(),
                    ));
                }
            }
            combined.push_str(&String::from_utf8_lossy(&untracked.stdout));
        }

        Ok(truncate_diff(combined.trim().to_owned()))
    }

    fn staged_diff(&self, repo: &Path) -> Result<String, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["diff", "--cached"])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::General(
                "failed to read staged git diff".into(),
            ));
        }
        Ok(truncate_diff(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        ))
    }

    fn clone_with_reference(
        &self,
        reference_repo: &Path,
        remote_url: &str,
        target_dir: &Path,
    ) -> Result<(), RiptaskError> {
        let (mut command, _askpass_path) = self.prepare_auth_command()?;
        command
            .arg("clone")
            .arg("--reference")
            .arg(reference_repo)
            .arg(remote_url)
            .arg(target_dir);
        status_to_result(command.output()?.status)
    }

    fn stash_push(&self, repo: &Path, message: &str) -> Result<bool, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["stash", "push", "--include-untracked", "-m", message])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::General("git stash push failed".into()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(!stdout.contains("No local changes to save"))
    }

    fn stash_pop(&self, repo: &Path) -> Result<bool, RiptaskError> {
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["stash", "pop"])
            .output()?;
        if output.status.success() {
            return Ok(true);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Conflicts during apply: stash is preserved, user must resolve manually
        if stderr.contains("CONFLICT") || stderr.contains("could not apply") {
            return Ok(false);
        }
        Err(RiptaskError::General(format!(
            "git stash pop failed: {}",
            stderr.trim()
        )))
    }
}

/// Attempt to pop an auto-stashed entry, printing a user-friendly message on success or failure.
pub fn try_stash_pop(git: &dyn GitBackend, repo: &Path) {
    match git.stash_pop(repo) {
        Ok(true) => crate::ui::success("restored auto-stashed changes"),
        Ok(false) => crate::ui::warn(
            "auto-stashed changes conflicted; resolve the conflicts, then run `git stash drop` to remove the stash entry",
        ),
        Err(e) => crate::ui::warn(&format!("failed to restore stashed changes: {e}")),
    }
}

fn run_git<const N: usize>(repo: &Path, args: [&str; N]) -> Result<(), RiptaskError> {
    run_git_dynamic(repo, &args)
}

fn run_git_dynamic(repo: &Path, args: &[&str]) -> Result<(), RiptaskError> {
    let mut command = git_command();
    command.arg("-C").arg(repo);
    for arg in args {
        command.arg(arg);
    }
    status_to_result(command.output()?.status)
}

/// Build a `git` `Command` with inherited override environment variables
/// removed.
///
/// When riptask or its test suite runs inside a `git commit` context (for
/// example under a pre-commit hook invoking `cargo nextest`), Git sets
/// variables such as `GIT_DIR`, `GIT_INDEX_FILE`, `GIT_WORK_TREE`, and
/// `GIT_OBJECT_DIRECTORY` to point at the outer repository. Those vars
/// take precedence over any `-C <path>` we pass on the command line, so
/// spawned `git` processes silently operate on the parent repo and hit
/// "invalid object" / index-lock errors. Strip them so `-C <path>` is
/// always authoritative for subprocess scope.
fn git_command() -> Command {
    let mut cmd = Command::new("git");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
        "GIT_PREFIX",
        "GIT_CEILING_DIRECTORIES",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

fn status_to_result(status: std::process::ExitStatus) -> Result<(), RiptaskError> {
    if status.success() {
        Ok(())
    } else {
        Err(RiptaskError::General("git command failed".into()))
    }
}

fn has_head_commit(repo: &Path) -> Result<bool, RiptaskError> {
    let status = git_command()
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?
        .status;
    Ok(status.success())
}

const MAX_DIFF_CHARS: usize = 8000;

fn truncate_diff(mut diff: String) -> String {
    if diff.len() > MAX_DIFF_CHARS {
        // Find the nearest char boundary at or before MAX_DIFF_CHARS to avoid
        // panicking on multi-byte UTF-8 sequences.
        let boundary = diff.floor_char_boundary(MAX_DIFF_CHARS);
        diff.truncate(boundary);
    }
    diff
}

fn list_untracked_files(repo: &Path) -> Result<Vec<std::path::PathBuf>, RiptaskError> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let output = git_command()
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;
    if !output.status.success() {
        return Err(RiptaskError::General(
            "failed to list untracked files".into(),
        ));
    }
    let mut paths = Vec::new();
    for chunk in output.stdout.split(|b| *b == 0) {
        if chunk.is_empty() {
            continue;
        }
        paths.push(std::path::PathBuf::from(OsStr::from_bytes(chunk)));
    }
    Ok(paths)
}

fn build_askpass_script(token: &str) -> String {
    let escaped = shell_escape::escape(token.into());
    format!(
        "#!/bin/sh\ncase \"$1\" in\n  Username*) echo \"oauth2\" ;;\n  *) echo {escaped} ;;\nesac\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{CliGit, GitBackend, build_askpass_script, git_command};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn builds_askpass_script_with_shell_escaped_token() {
        let script = build_askpass_script("glpat-token'with-quote");

        assert_eq!(
            script,
            "#!/bin/sh\ncase \"$1\" in\n  Username*) echo \"oauth2\" ;;\n  *) echo 'glpat-token'\\''with-quote' ;;\nesac\n"
        );
    }

    // Regression: when riptask tests run under `git commit` (e.g. a
    // pre-commit hook invoking cargo nextest), git exports override
    // variables like GIT_DIR / GIT_INDEX_FILE pointing at the outer
    // repository. If `git_command` does not scrub these, every
    // subprocess `git` call silently targets the outer repo instead
    // of the temp dir, producing "invalid object" and index-lock
    // failures. This test forces the same conditions and asserts that
    // a temp-repo workflow still succeeds.
    #[test]
    fn git_command_scrubs_inherited_git_env_vars() {
        let outer = tempdir().expect("outer repo");
        run_git(outer.path(), &["init"]);
        run_git(outer.path(), &["config", "user.name", "Test User"]);
        run_git(outer.path(), &["config", "user.email", "test@example.com"]);
        fs::write(outer.path().join("a.txt"), "outer\n").expect("write a");
        run_git(outer.path(), &["add", "a.txt"]);
        run_git(outer.path(), &["commit", "-m", "outer initial"]);

        // SAFETY: tests run in the same process; set the hostile env
        // vars, invoke git_command(), and remove them afterwards. This
        // mirrors how a pre-commit hook would launch us.
        // `std::env::set_var` is !Send-hostile but cargo-nextest runs
        // each test in its own process, so intra-test mutation is safe.
        let outer_git_dir = outer.path().join(".git");
        unsafe {
            std::env::set_var("GIT_DIR", &outer_git_dir);
            std::env::set_var("GIT_INDEX_FILE", outer_git_dir.join("index"));
            std::env::set_var("GIT_WORK_TREE", outer.path());
        }
        let _cleanup = EnvCleanup;

        let inner = tempdir().expect("inner repo");
        run_git(inner.path(), &["init"]);
        run_git(inner.path(), &["config", "user.name", "Test User"]);
        run_git(inner.path(), &["config", "user.email", "test@example.com"]);
        fs::write(inner.path().join("b.txt"), "inner\n").expect("write b");
        run_git(inner.path(), &["add", "b.txt"]);
        run_git(inner.path(), &["commit", "-m", "inner initial"]);

        // The inner repo should have exactly one commit with `b.txt`,
        // not the outer repo's `a.txt` from its index/work tree.
        let ls_out = git_command()
            .arg("-C")
            .arg(inner.path())
            .args(["ls-files"])
            .output()
            .expect("ls-files");
        assert!(ls_out.status.success(), "ls-files failed");
        let listed = String::from_utf8_lossy(&ls_out.stdout);
        assert_eq!(listed.trim(), "b.txt", "inner repo index leaked to outer");

        struct EnvCleanup;
        impl Drop for EnvCleanup {
            fn drop(&mut self) {
                unsafe {
                    std::env::remove_var("GIT_DIR");
                    std::env::remove_var("GIT_INDEX_FILE");
                    std::env::remove_var("GIT_WORK_TREE");
                }
            }
        }
    }

    #[test]
    fn working_tree_diff_returns_diff_when_changes_exist() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("note.txt"), "before\n").expect("write file");
        run_git(temp.path(), &["add", "note.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("note.txt"), "after\n").expect("modify file");

        let diff = CliGit::new()
            .working_tree_diff(temp.path())
            .expect("working tree diff");

        assert!(diff.contains("-before"));
        assert!(diff.contains("+after"));
    }

    #[test]
    fn working_tree_diff_includes_untracked_files() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("tracked.txt"), "kept\n").expect("write file");
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("brand_new.rs"), "fn main() {}\n").expect("write untracked");

        let diff = CliGit::new()
            .working_tree_diff(temp.path())
            .expect("working tree diff");

        assert!(
            diff.contains("brand_new.rs"),
            "diff missing untracked filename: {diff}"
        );
        assert!(
            diff.contains("+fn main() {}"),
            "diff missing untracked content: {diff}"
        );
    }

    #[test]
    fn working_tree_diff_combines_modified_and_untracked() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("note.txt"), "before\n").expect("write file");
        run_git(temp.path(), &["add", "note.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("note.txt"), "after\n").expect("modify file");
        fs::write(temp.path().join("extra.rs"), "fn extra() {}\n").expect("write untracked");

        let diff = CliGit::new()
            .working_tree_diff(temp.path())
            .expect("working tree diff");

        assert!(diff.contains("-before"), "missing modified -before: {diff}");
        assert!(diff.contains("+after"), "missing modified +after: {diff}");
        assert!(
            diff.contains("extra.rs"),
            "missing untracked filename: {diff}"
        );
        assert!(
            diff.contains("+fn extra() {}"),
            "missing untracked content: {diff}"
        );
    }

    #[test]
    fn working_tree_diff_returns_untracked_when_no_head() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        // No commits yet — HEAD does not resolve.
        fs::write(temp.path().join("seed.rs"), "fn seed() {}\n").expect("write untracked");

        let diff = CliGit::new()
            .working_tree_diff(temp.path())
            .expect("working tree diff");

        assert!(
            !diff.is_empty(),
            "expected non-empty diff for HEAD-less repo with untracked file"
        );
        assert!(
            diff.contains("seed.rs"),
            "missing untracked filename: {diff}"
        );
        assert!(
            diff.contains("+fn seed() {}"),
            "missing untracked content: {diff}"
        );
    }

    #[test]
    fn working_tree_diff_returns_empty_when_clean() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("note.txt"), "stable\n").expect("write file");
        run_git(temp.path(), &["add", "note.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        let diff = CliGit::new()
            .working_tree_diff(temp.path())
            .expect("working tree diff");

        assert!(diff.is_empty());
    }

    #[test]
    fn stash_push_returns_true_when_changes_exist() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("file.txt"), "initial\n").expect("write file");
        run_git(temp.path(), &["add", "file.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("file.txt"), "modified\n").expect("modify file");

        let created = CliGit::new()
            .stash_push(temp.path(), "test stash")
            .expect("stash push");
        assert!(created);
    }

    #[test]
    fn stash_push_returns_false_when_clean() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("file.txt"), "stable\n").expect("write file");
        run_git(temp.path(), &["add", "file.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        let created = CliGit::new()
            .stash_push(temp.path(), "test stash")
            .expect("stash push");
        assert!(!created);
    }

    #[test]
    fn stash_pop_restores_changes() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("file.txt"), "initial\n").expect("write file");
        run_git(temp.path(), &["add", "file.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("file.txt"), "modified\n").expect("modify file");

        let git = CliGit::new();
        git.stash_push(temp.path(), "test stash")
            .expect("stash push");

        // Working tree should be clean after stash
        let content = fs::read_to_string(temp.path().join("file.txt")).expect("read file");
        assert_eq!(content, "initial\n");

        // Pop should succeed and restore changes
        let ok = git.stash_pop(temp.path()).expect("stash pop");
        assert!(ok);
        let content = fs::read_to_string(temp.path().join("file.txt")).expect("read file");
        assert_eq!(content, "modified\n");
    }

    #[test]
    fn stash_push_includes_untracked_files() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("tracked.txt"), "content\n").expect("write file");
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        // Create an untracked file
        fs::write(temp.path().join("untracked.txt"), "new\n").expect("write file");

        let git = CliGit::new();
        let created = git
            .stash_push(temp.path(), "test stash")
            .expect("stash push");
        assert!(created);

        // Untracked file should be gone after stash
        assert!(!temp.path().join("untracked.txt").exists());

        // Pop should restore it
        let ok = git.stash_pop(temp.path()).expect("stash pop");
        assert!(ok);
        assert!(temp.path().join("untracked.txt").exists());
    }

    fn run_git(repo: &std::path::Path, args: &[&str]) {
        let status = git_command()
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .expect("git command");
        assert!(status.success(), "git {:?} failed", args);
    }
}
