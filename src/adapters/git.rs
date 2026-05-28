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
    /// Put the local repository on a branch named `slug`.
    ///
    /// Works for three states:
    /// - Unborn HEAD: rewrites `HEAD` as a symbolic-ref to `refs/heads/<slug>`
    ///   without making a commit. The index and working tree are untouched, so
    ///   any staged or unstaged files survive the rename.
    /// - Existing local branch: switches to it.
    /// - Otherwise: creates a new local branch off the current commit.
    fn ensure_on_local_branch(&self, repo: &Path, slug: &str) -> Result<(), RiptaskError>;
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
    /// Push a fresh orphan empty commit to `origin/<ref_name>` without
    /// touching any local refs, the index, or the working tree.
    ///
    /// Used to bootstrap an empty remote: we materialize `git`'s empty tree
    /// via `mktree`, create an orphan commit with `commit-tree`, and push
    /// the resulting SHA directly with a `<sha>:refs/heads/<ref_name>`
    /// refspec. The remote ends up with a single empty-content commit on
    /// `ref_name`; locally nothing changes.
    fn push_orphan_initial_branch(
        &self,
        repo: &Path,
        ref_name: &str,
        message: &str,
    ) -> Result<(), RiptaskError>;
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
    fn has_head_commit(&self, repo: &Path) -> Result<bool, RiptaskError>;
    /// Returns `true` if `origin` advertises at least one branch ref.
    ///
    /// Used to detect the "empty remote" state when local commits already
    /// exist (so `has_head_commit` is `true`) but the remote has no refs.
    fn remote_has_any_refs(&self, repo: &Path) -> Result<bool, RiptaskError>;
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
        let output = command.output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RiptaskError::General(git_failure_message(args, &output)))
        }
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
        let output = command.output()?;
        if output.status.success() {
            Ok(())
        } else {
            let args = std::iter::once("add")
                .chain(files.iter().map(|f| f.to_str().unwrap_or("<non-utf8>")))
                .collect::<Vec<_>>();
            Err(RiptaskError::General(git_failure_message(&args, &output)))
        }
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

    fn ensure_on_local_branch(&self, repo: &Path, slug: &str) -> Result<(), RiptaskError> {
        if !has_head_commit(repo)? {
            // Unborn HEAD: rewrite the symbolic-ref so HEAD points at
            // refs/heads/<slug> without committing anything. The index and
            // working tree are unaffected.
            let target = format!("refs/heads/{slug}");
            return run_git_dynamic(repo, &["symbolic-ref", "HEAD", &target]);
        }
        if self.branch_exists(repo, slug)? {
            return run_git_dynamic(repo, &["switch", slug]);
        }
        run_git_dynamic(repo, &["switch", "-c", slug])
    }

    fn branch_exists(&self, repo: &Path, name: &str) -> Result<bool, RiptaskError> {
        let refname = format!("refs/heads/{name}");
        // `git show-ref --quiet --verify <ref>` is silent on missing refs and
        // exits non-zero — exactly the probe semantics we want here.
        // Using `rev-parse --verify` instead leaks "fatal: Needed a single
        // revision" to stderr, which surfaces as a scary user-facing message
        // for what is really just a routine existence check.
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["show-ref", "--quiet", "--verify", &refname])
            .output()?;
        Ok(output.status.success())
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
        let symbolic = git_command()
            .arg("-C")
            .arg(repo)
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()?;
        if symbolic.status.success() {
            return Ok(String::from_utf8_lossy(&symbolic.stdout).trim().to_owned());
        }

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

    fn has_head_commit(&self, repo: &Path) -> Result<bool, RiptaskError> {
        has_head_commit(repo)
    }

    fn remote_has_any_refs(&self, repo: &Path) -> Result<bool, RiptaskError> {
        let (mut command, _askpass) = self.prepare_auth_command()?;
        let output = command
            .arg("-C")
            .arg(repo)
            .args(["ls-remote", "--heads", "origin"])
            .output()?;
        if !output.status.success() {
            return Err(RiptaskError::Unreachable(format!(
                "failed to probe remote refs: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(!output.stdout.iter().all(u8::is_ascii_whitespace))
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
        // "Staged changes" means the index differs from HEAD. The probe must be
        // HEAD-aware: against a real HEAD use `diff-index --cached --quiet HEAD`
        // (the plumbing form documented in git-diff-index(1)); on an unborn HEAD
        // there is no committed tree to compare against, so fall back to the
        // empty-tree SHA so any index entry counts as "staged". A previous
        // version used the empty-tree base unconditionally, which made the
        // predicate "is anything tracked at all?" and returned true on every
        // clean repo with at least one committed file.
        let base: &str = if has_head_commit(repo)? {
            "HEAD"
        } else {
            "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
        };
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["diff-index", "--cached", "--quiet", base])
            .output()?;
        match output.status.code() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(RiptaskError::General(git_failure_message(
                &["diff-index", "--cached", "--quiet", base],
                &output,
            ))),
        }
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
        let output = git_command()
            .arg("-C")
            .arg(repo)
            .args(["merge-base", "--is-ancestor", branch, base])
            .output()?;
        Ok(output.status.success())
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

    fn push_orphan_initial_branch(
        &self,
        repo: &Path,
        ref_name: &str,
        message: &str,
    ) -> Result<(), RiptaskError> {
        // Step 1: materialize git's empty tree as a real object. `git mktree`
        // reads tree entries from stdin; an empty stdin produces the
        // well-known empty-tree SHA `4b825dc642cb6eb9a060e54bf8d69288fbee4904`.
        let mktree = git_command()
            .arg("-C")
            .arg(repo)
            .args(["mktree"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let mktree_out = mktree.wait_with_output()?;
        if !mktree_out.status.success() {
            return Err(RiptaskError::General(git_failure_message(
                &["mktree"],
                &mktree_out,
            )));
        }
        let empty_tree = String::from_utf8_lossy(&mktree_out.stdout)
            .trim()
            .to_owned();

        // Step 2: create an orphan commit (no parents) pointing at the empty
        // tree. The object lands in the local objstore but is not yet
        // referenced by any ref.
        let mut commit_args: Vec<&str> = vec!["commit-tree", &empty_tree];
        for part in message.split("\n\n") {
            commit_args.push("-m");
            commit_args.push(part);
        }
        let commit_out = git_command()
            .arg("-C")
            .arg(repo)
            .args(&commit_args)
            .output()?;
        if !commit_out.status.success() {
            return Err(RiptaskError::General(git_failure_message(
                &commit_args,
                &commit_out,
            )));
        }
        let commit_sha = String::from_utf8_lossy(&commit_out.stdout)
            .trim()
            .to_owned();

        // Step 3: refuse to clobber an existing local branch of the same
        // name; the user may have real work there.
        if self.branch_exists(repo, ref_name)? {
            return Err(RiptaskError::General(format!(
                "cannot bootstrap remote `{ref_name}`: a local branch named \
                 `{ref_name}` already exists. Push it yourself (\
                 `git push -u origin {ref_name}`) or remove it (\
                 `git branch -D {ref_name}`) and re-run."
            )));
        }

        // Step 4: create the local ref pointing at the orphan commit, then
        // push that named ref via the normal `git push` machinery. Pushing a
        // named ref is what every git tool does and is what GitHub's smart
        // HTTP server expects for the first push to an empty repository; a
        // bare `<sha>:<dst>` refspec works against local bare remotes but
        // GitHub silently rejects it with no actionable error.
        let ref_path = format!("refs/heads/{ref_name}");
        run_git_dynamic(repo, &["update-ref", &ref_path, &commit_sha])?;

        let push_result = self.run_git_remote(repo, &["push", "-u", "origin", ref_name]);

        // If the push failed, roll back the local ref so a retry starts from
        // a clean state. We deliberately ignore the rollback error: the push
        // error is what the user needs to see.
        if push_result.is_err() {
            let _ = run_git_dynamic(repo, &["update-ref", "-d", &ref_path]);
        }
        push_result
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
        let output = command.output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RiptaskError::General(git_failure_message(
                &["clone", "--reference", "<ref>", remote_url, "<target>"],
                &output,
            )))
        }
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
        // Recoverable failures preserve the stash; user resolves manually.
        // Covers both apply conflicts ("CONFLICT", "could not apply") and
        // untracked-file path collisions ("could not restore untracked files
        // from stash"), which otherwise silently orphan the stash.
        let lower = stderr.to_ascii_lowercase();
        if lower.contains("conflict")
            || lower.contains("could not apply")
            || lower.contains("could not restore")
        {
            return Ok(false);
        }
        Err(RiptaskError::General(format!(
            "git stash pop failed: {}",
            stderr.trim()
        )))
    }
}

/// Refuse to proceed if the working tree has any uncommitted state (staged,
/// unstaged, or untracked). Names `command` in the error so the user knows
/// which step refused to run, and lists explicit recovery options.
pub fn require_clean_working_tree(
    git: &dyn GitBackend,
    repo: &Path,
    command: &str,
) -> Result<(), RiptaskError> {
    if git.has_working_tree_changes(repo)? {
        return Err(RiptaskError::General(format!(
            "`{command}` refuses to run with uncommitted changes in the working tree. \
             Inspect with `git status`, then choose one: \
             `git commit` to keep them, \
             `git stash push --include-untracked` to set them aside, or \
             `git reset --hard && git clean -fd` to discard everything (destructive: \
             drops staged, unstaged, and untracked changes)."
        )));
    }
    Ok(())
}

/// Attempt to pop an auto-stashed entry, printing a user-friendly message on success or failure.
pub fn try_stash_pop(git: &dyn GitBackend, repo: &Path) {
    match git.stash_pop(repo) {
        Ok(true) => crate::ui::success("restored auto-stashed changes"),
        Ok(false) => crate::ui::warn(
            "auto-stashed changes were not restored; inspect with `git stash list` and recover with `git stash pop` after resolving any conflicts",
        ),
        Err(e) => crate::ui::warn(&format!(
            "failed to restore stashed changes: {e}; inspect with `git stash list` and recover with `git stash pop`"
        )),
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
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RiptaskError::General(git_failure_message(args, &output)))
    }
}

fn git_failure_message(args: &[&str], output: &std::process::Output) -> String {
    let cmd = args.join(" ");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("`git {cmd}` failed (exit {})", output.status)
    } else {
        format!("`git {cmd}` failed: {stderr}")
    }
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
    use super::{
        CliGit, GitBackend, build_askpass_script, git_command, require_clean_working_tree,
    };
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

    #[test]
    fn stash_pop_returns_ok_false_on_untracked_collision() {
        // Regression for the silent-data-loss bug: when `git stash pop` cannot
        // restore an untracked entry because a same-named file already exists
        // in the working tree, git emits "could not restore untracked files
        // from stash" and exits non-zero. The classifier must treat this as
        // recoverable (Ok(false)) so callers know the stash is preserved, not
        // map it to Err which discards the recovery context.
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("tracked.txt"), "content\n").expect("write tracked");
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("collide.txt"), "from-stash\n").expect("write untracked");

        let git = CliGit::new();
        let created = git
            .stash_push(temp.path(), "untracked collision")
            .expect("stash push");
        assert!(created);
        assert!(!temp.path().join("collide.txt").exists());

        // Recreate the same path so the stash cannot be restored cleanly.
        fs::write(temp.path().join("collide.txt"), "from-tree\n").expect("recreate collision");

        let outcome = git.stash_pop(temp.path()).expect("stash pop classified");
        assert!(
            !outcome,
            "untracked collision must be reported as recoverable (Ok(false))"
        );

        // Stash must remain available for manual recovery.
        let list = git_command()
            .arg("-C")
            .arg(temp.path())
            .args(["stash", "list"])
            .output()
            .expect("git stash list");
        assert!(list.status.success());
        let stdout = String::from_utf8_lossy(&list.stdout);
        assert!(
            !stdout.trim().is_empty(),
            "stash entry should be preserved after recoverable pop failure, got: {stdout:?}"
        );
    }

    #[test]
    fn require_clean_working_tree_errors_when_dirty() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);
        fs::write(temp.path().join("tracked.txt"), "content\n").expect("write tracked");
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        // Untracked file alone must trip the gate.
        fs::write(temp.path().join("scratch.log"), "noise\n").expect("write untracked");

        let git = CliGit::new();
        let err = require_clean_working_tree(&git, temp.path(), "tsk pr merge")
            .expect_err("dirty tree must error");
        let msg = err.to_string();
        assert!(msg.contains("tsk pr merge"), "message names command: {msg}");
        assert!(msg.contains("git status"), "message hints recovery: {msg}");
    }

    #[test]
    fn require_clean_working_tree_passes_on_clean_tree() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);
        fs::write(temp.path().join("tracked.txt"), "content\n").expect("write tracked");
        run_git(temp.path(), &["add", "tracked.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        let git = CliGit::new();
        require_clean_working_tree(&git, temp.path(), "tsk done").expect("clean tree allowed");
    }

    #[test]
    fn has_head_commit_is_false_on_unborn_head() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);

        let git = CliGit::new();
        assert!(!git.has_head_commit(temp.path()).expect("has head commit"));
        assert_eq!(
            git.current_branch(temp.path()).expect("current branch"),
            "main"
        );
    }

    #[test]
    fn has_head_commit_and_current_branch_work_after_first_commit() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        let git = CliGit::new();
        assert_eq!(
            git.current_branch(temp.path()).expect("current branch"),
            "main"
        );
        assert!(!git.has_head_commit(temp.path()).expect("has head commit"));

        git.create_empty_commit(temp.path(), "initial")
            .expect("empty commit");

        assert!(git.has_head_commit(temp.path()).expect("has head commit"));
        assert_eq!(
            git.current_branch(temp.path()).expect("current branch"),
            "main"
        );
    }

    #[test]
    fn has_staged_changes_is_false_on_clean_tree_with_committed_files() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("file.txt"), "content\n").expect("write file");
        run_git(temp.path(), &["add", "file.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        let git = CliGit::new();
        assert!(
            !git.has_staged_changes(temp.path())
                .expect("has staged changes"),
            "clean tree with committed files must not be reported as staged"
        );
    }

    #[test]
    fn has_staged_changes_is_true_on_unborn_head_with_staged_file() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("staged.txt"), "hello").expect("write");
        run_git(temp.path(), &["add", "staged.txt"]);

        let git = CliGit::new();
        assert!(!git.has_head_commit(temp.path()).expect("has head commit"));
        assert!(
            git.has_staged_changes(temp.path())
                .expect("has staged changes")
        );
    }

    #[test]
    fn has_staged_changes_is_false_on_unborn_head_with_empty_index() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);

        let git = CliGit::new();
        assert!(!git.has_head_commit(temp.path()).expect("has head commit"));
        assert!(
            !git.has_staged_changes(temp.path())
                .expect("has staged changes")
        );
    }

    #[test]
    fn has_staged_changes_is_true_after_commit_then_modify_and_stage() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        fs::write(temp.path().join("file.txt"), "initial\n").expect("write file");
        run_git(temp.path(), &["add", "file.txt"]);
        run_git(temp.path(), &["commit", "-m", "initial"]);

        fs::write(temp.path().join("file.txt"), "modified\n").expect("modify file");
        run_git(temp.path(), &["add", "file.txt"]);

        let git = CliGit::new();
        assert!(
            git.has_staged_changes(temp.path())
                .expect("has staged changes")
        );
    }

    #[test]
    fn ensure_on_local_branch_on_unborn_head_preserves_staged_files() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        // User has done `git add` before tsk start.
        std::fs::write(temp.path().join("staged.txt"), "hello").expect("write");
        run_git(temp.path(), &["add", "staged.txt"]);

        let git = CliGit::new();
        assert!(!git.has_head_commit(temp.path()).expect("has head commit"));

        git.ensure_on_local_branch(temp.path(), "123-test")
            .expect("rename unborn branch");

        // HEAD is still unborn but now points at refs/heads/123-test.
        assert!(!git.has_head_commit(temp.path()).expect("has head commit"));
        assert_eq!(
            git.current_branch(temp.path()).expect("current branch"),
            "123-test"
        );

        // The staged file is still in the index, untouched.
        let cached = git_command()
            .arg("-C")
            .arg(temp.path())
            .args(["ls-files", "--cached"])
            .output()
            .expect("ls-files");
        let cached = String::from_utf8_lossy(&cached.stdout);
        assert!(
            cached.contains("staged.txt"),
            "staged file must remain in the index, got: {cached:?}"
        );
    }

    #[test]
    fn ensure_on_local_branch_with_existing_history_switches_to_new_branch() {
        let temp = tempdir().expect("temp dir");
        run_git(temp.path(), &["init", "-b", "main"]);
        run_git(temp.path(), &["config", "user.name", "Test User"]);
        run_git(temp.path(), &["config", "user.email", "test@example.com"]);

        let git = CliGit::new();
        git.create_empty_commit(temp.path(), "initial")
            .expect("initial commit");

        git.ensure_on_local_branch(temp.path(), "123-test")
            .expect("switch to new branch");
        assert_eq!(
            git.current_branch(temp.path()).expect("current branch"),
            "123-test"
        );

        // Switching to the same branch again is a no-op (idempotent).
        git.ensure_on_local_branch(temp.path(), "123-test")
            .expect("re-switch");
        assert_eq!(
            git.current_branch(temp.path()).expect("current branch"),
            "123-test"
        );
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
