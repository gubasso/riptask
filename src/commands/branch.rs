use crate::adapters::backend::BranchCreateOutcome;
use crate::adapters::git::{CliGit, GitBackend};
use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::BranchArgs;
use crate::config::{Config, load_effective_config};
use crate::error::RiptaskError;
use crate::models::{BackendKind, RepoProject};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::backend_mapping::{build_version_control, resolve_git_auth};
use crate::services::issue_service::generate_branch_slug;
use crate::services::{id_resolution, project_detection};
use crate::storage::{frontmatter, issue_store};

pub(crate) use crate::services::id_resolution::{current_repo, cwd_utf8, find_issue_for_branch};

pub async fn branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptaskError> {
    if args.delete || args.force_delete {
        return delete_branch(paths, args).await;
    }
    if args.adopt {
        return adopt_branch(paths, args).await;
    }
    create_branch(paths, args).await
}

async fn adopt_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let BranchArgs { id, yes, .. } = args;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let cwd = cwd_utf8();

    let repo = current_repo()?;
    let git = CliGit::new();
    let current = git.current_branch(repo.as_path())?;
    if is_protected_branch(&current) {
        return Err(RiptaskError::General(format!(
            "refusing to adopt protected branch: {current}"
        )));
    }

    let resolved_id = match id {
        Some(input) => id_resolution::resolve_id(paths, &config, &cwd, &input)?,
        None => {
            let number = parse_leading_issue_number(&current).ok_or_else(|| {
                RiptaskError::General(format!(
                    "cannot infer issue id from branch '{current}': expected '<number>-<slug>'. \
                     Pass an id explicitly: `tsk branch --adopt <id>`"
                ))
            })?;
            id_resolution::resolve_id(paths, &config, &cwd, &number.to_string())?
        }
    };

    let path = issue_store::find_issue(paths, &resolved_id)?;
    let mut issue =
        crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &resolved_id)?;

    if let Ok(existing_path) = find_issue_for_branch(paths, &current) {
        let existing_id = existing_path.file_stem().unwrap_or_default().to_string();
        if existing_id != resolved_id {
            return Err(RiptaskError::General(format!(
                "branch '{current}' is already linked to issue {existing_id}"
            )));
        }
    }

    if let Some(existing) = issue.frontmatter.branch.as_deref() {
        if existing == current {
            crate::ui::info(&format!(
                "issue {resolved_id} already linked to branch '{current}'"
            ));
            return Ok(());
        }
        if !yes {
            let prompt = format!(
                "issue {resolved_id} is already linked to branch '{existing}'. \
                 Overwrite with '{current}'?"
            );
            if !DialoguerPrompts.confirm(&prompt, false)? {
                crate::ui::warn("aborted");
                return Ok(());
            }
        }
    }

    issue.frontmatter.id_slug = Some(current.clone());
    issue.frontmatter.branch = Some(current.clone());
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
    maybe_auto_commit(
        &config,
        &git,
        paths.riptask_repo.as_std_path(),
        &format!(
            "riptask: adopt branch {} for {} - {}",
            current, issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )?;
    crate::ui::success(&format!("linked issue {resolved_id} to branch '{current}'"));
    Ok(())
}

fn parse_leading_issue_number(branch: &str) -> Option<u64> {
    let end = branch
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(branch.len());
    if end == 0 {
        return None;
    }
    branch[..end].parse::<u64>().ok()
}

async fn create_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let BranchArgs {
        scope, id, pick, ..
    } = args;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let cwd = cwd_utf8();
    let Some(id) = id_resolution::resolve_or_pick_id(paths, &config, &cwd, id, pick, &scope)?
    else {
        return Ok(());
    };
    let slug = create_branch_for_issue(paths, &id).await?;
    println!("{slug}");
    Ok(())
}

pub(crate) async fn create_branch_for_issue(
    paths: &AppPaths,
    id: &str,
) -> Result<String, RiptaskError> {
    paths.require_shared_layer()?;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let path = issue_store::find_issue(paths, id)?;
    let mut issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), id)?;

    let repo_project = config
        .projects
        .iter()
        .find(|rp| rp.name == issue.frontmatter.project)
        .ok_or_else(|| RiptaskError::Unregistered(issue.frontmatter.project.clone()))?;
    if repo_project.vc_backend.kind == BackendKind::Local {
        return Err(RiptaskError::Config(
            "cannot create remote branch for local-only project".into(),
        ));
    }

    let issue_number = backend_issue_number(repo_project, &issue)?;

    let slug = generate_branch_slug(issue_number, &issue.frontmatter.title);

    let repo = current_repo()?;

    // For Jira backends, resolve the VC backend for branch/PR operations
    let vc_provider = build_version_control(&repo_project.vc_backend, &repo_project.name)?;
    let vc_issue_id = if repo_project.tasks_backend.kind == BackendKind::Jira {
        None
    } else {
        Some(issue_number)
    };

    let auth = resolve_git_auth(&repo_project.vc_backend, &repo_project.name);
    let git = CliGit::with_auth(auth);
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let base = crate::ui::spin_on_async("Fetching default branch", async {
        vc_provider.default_branch(repo_name).await
    })
    .await?;

    let branch_spinner = crate::ui::spinner("Creating remote branch".to_owned());
    let create_branch_result = vc_provider
        .create_branch(repo_name, &slug, &base, vc_issue_id)
        .await;
    if let Some(ref pb) = branch_spinner {
        pb.finish_and_clear();
    }
    let outcome = create_branch_result?;
    if outcome == BranchCreateOutcome::AlreadyExists {
        crate::ui::info("remote branch already exists, continuing with local checkout");
    }
    if outcome == BranchCreateOutcome::EmptyRemote {
        crate::ui::info("remote repository is empty; deferring remote setup until `tsk pr` runs");
    }
    tracing::info!(branch = %slug, backend = %repo_project.name, ?outcome, "ensured remote branch exists");
    checkout_branch_after_create(&git, repo.as_path(), &slug, outcome)?;

    issue.frontmatter.id_slug = Some(slug.clone());
    issue.frontmatter.branch = Some(slug.clone());
    frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
    maybe_auto_commit(
        &config,
        &git,
        paths.riptask_repo.as_std_path(),
        &format!(
            "riptask: branch {} - {}",
            issue.frontmatter.id, issue.frontmatter.title
        ),
        &[path.as_std_path()],
    )?;
    Ok(slug)
}

fn checkout_branch_after_create(
    git: &dyn GitBackend,
    repo: &std::path::Path,
    slug: &str,
    outcome: BranchCreateOutcome,
) -> Result<(), RiptaskError> {
    match outcome {
        BranchCreateOutcome::Created | BranchCreateOutcome::AlreadyExists => {
            if !git.branch_exists(repo, slug)? {
                crate::ui::spin_on("Checking out branch", || {
                    git.fetch_and_checkout_tracking(repo, slug)
                })?;
            } else {
                git.checkout(repo, slug)?;
            }
        }
        BranchCreateOutcome::EmptyRemote => {
            // Local-only setup: works for both unborn HEAD (renames the
            // symbolic-ref) and existing-history (creates a new branch off
            // the current commit).
            crate::ui::spin_on("Setting up local branch", || {
                git.ensure_on_local_branch(repo, slug)
            })?;
        }
    }
    Ok(())
}

async fn delete_branch(paths: &AppPaths, args: BranchArgs) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let cwd = cwd_utf8();
    let repo = current_repo()?;
    let current = CliGit::new().current_branch(repo.as_path())?;
    let force = args.force_delete;

    let target_branch = if let Some(ref input) = args.id {
        resolve_delete_target(paths, &config, &cwd, input)?
    } else {
        if is_protected_branch(&current) {
            crate::ui::warn(&format!("refusing to delete protected branch: {current}"));
            return Ok(());
        }
        current.clone()
    };

    if args.id.is_some() && is_protected_branch(&target_branch) {
        return Err(RiptaskError::General(format!(
            "cannot delete protected branch: {target_branch}"
        )));
    }

    let deleting_current = target_branch == current;

    let issue_path = match find_issue_for_branch(paths, &target_branch) {
        Ok(path) => Some(path),
        Err(RiptaskError::NotFound(_)) => None,
        Err(e) => return Err(e),
    };
    let (repo_project, default_branch) =
        resolve_backend_and_default(&config, &cwd, issue_path.as_ref()).await?;
    let auth = resolve_git_auth(&repo_project.vc_backend, &repo_project.name);
    let git = CliGit::with_auth(auth);
    let provider = build_version_control(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();

    let fetch_spinner = crate::ui::spinner("Fetching remote branches".to_owned());
    let fetch_result = git.fetch(repo.as_path());
    if let Some(ref pb) = fetch_spinner {
        pb.finish_and_clear();
    }
    let _ = fetch_result;

    if !force {
        let local_exists = git.branch_exists(repo.as_path(), &target_branch)?;
        if !local_exists {
            return Err(RiptaskError::General(format!(
                "branch '{}' not found locally; cannot verify merge status for safe delete. Use -D to force",
                target_branch
            )));
        }
        let base_ref = format!("origin/{default_branch}");
        if !git.is_branch_merged(repo.as_path(), &target_branch, &base_ref)? {
            return Err(RiptaskError::General(format!(
                "branch '{}' is not fully merged into '{}'; use -D to force",
                target_branch, default_branch
            )));
        }
    }

    if deleting_current && !args.yes {
        let prompt =
            format!("Delete current branch '{target_branch}' and switch to '{default_branch}'?");
        if !DialoguerPrompts.confirm(&prompt, false)? {
            crate::ui::warn("aborted");
            return Ok(());
        }
    }

    let delete_spinner = crate::ui::spinner("Deleting remote branch".to_owned());
    let delete_result = provider.delete_branch(repo_name, &target_branch).await;
    if let Some(ref pb) = delete_spinner {
        pb.finish_and_clear();
    }
    match delete_result {
        Ok(()) => crate::ui::success(&format!("deleted remote branch: {target_branch}")),
        Err(ref e)
            if e.to_string().contains("404")
                || e.to_string().to_lowercase().contains("not found") =>
        {
            crate::ui::info("remote branch not found, continuing with local cleanup");
        }
        Err(e) => return Err(e),
    }
    tracing::info!(branch = %target_branch, backend = %repo_project.name, "remote branch deletion finished");

    if deleting_current && let Err(e) = git.checkout(repo.as_path(), &default_branch) {
        return Err(RiptaskError::General(format!(
            "remote branch deleted but failed to switch to '{default_branch}': {e}. \
             Recover with: git checkout -b {default_branch} origin/{default_branch}"
        )));
    }

    if git.branch_exists(repo.as_path(), &target_branch)? {
        git.delete_local_branch(repo.as_path(), &target_branch, force)?;
        crate::ui::success(&format!("deleted local branch: {target_branch}"));
    }

    if let Some(path) = issue_path {
        let id = path.file_stem().unwrap_or_default().to_string();
        let mut issue =
            crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        issue.frontmatter.branch = None;
        issue.frontmatter.id_slug = None;
        frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptaskError::Other)?;
        maybe_auto_commit(
            &config,
            &git,
            paths.riptask_repo.as_std_path(),
            &format!(
                "riptask: delete branch {} - {}",
                issue.frontmatter.id, issue.frontmatter.title
            ),
            &[path.as_std_path()],
        )?;
    }

    Ok(())
}

fn resolve_delete_target(
    paths: &AppPaths,
    config: &Config,
    cwd: &camino::Utf8Path,
    input: &str,
) -> Result<String, RiptaskError> {
    if input.chars().all(|c| c.is_ascii_digit()) || input.contains("--") {
        let id = id_resolution::resolve_id(paths, config, cwd, input)?;
        let path = issue_store::find_issue(paths, &id)?;
        let issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        if let Some(branch) = issue.frontmatter.branch {
            return Ok(branch);
        }
        return Err(RiptaskError::General(format!(
            "issue {id} has no associated branch"
        )));
    }
    Ok(input.to_owned())
}

async fn resolve_backend_and_default(
    config: &Config,
    cwd: &camino::Utf8Path,
    issue_path: Option<&camino::Utf8PathBuf>,
) -> Result<(RepoProject, String), RiptaskError> {
    let repo_project = if let Some(path) = issue_path {
        let id = path.file_stem().unwrap_or_default().to_string();
        let issue = crate::commands::issues::load_issue_or_conflict_error(path.as_std_path(), &id)?;
        config
            .projects
            .iter()
            .find(|rp| rp.name == issue.frontmatter.project)
            .ok_or_else(|| RiptaskError::Unregistered(issue.frontmatter.project.clone()))?
            .clone()
    } else {
        project_detection::detect_from_cwd(cwd, config)?
            .cloned()
            .ok_or_else(|| {
                RiptaskError::Config("cannot determine project for branch deletion".into())
            })?
    };

    let provider = build_version_control(&repo_project.vc_backend, &repo_project.name)?;
    let repo_name = repo_project.vc_backend.repo.as_deref().unwrap_or_default();
    let default = provider.default_branch(repo_name).await?;
    Ok((repo_project, default))
}

pub(crate) fn backend_issue_number(
    repo_project: &RepoProject,
    issue: &crate::domain::issue::IssueDocument,
) -> Result<u64, RiptaskError> {
    match repo_project.tasks_backend.kind {
        BackendKind::Github => issue
            .frontmatter
            .github
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Gitlab => issue
            .frontmatter
            .gitlab
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Jira => issue
            .frontmatter
            .jira
            .as_ref()
            .and_then(|meta| meta.issue_id),
        BackendKind::Local => None,
    }
    .ok_or_else(|| {
        RiptaskError::Config(format!(
            "issue {} is missing backend metadata",
            issue.frontmatter.id
        ))
    })
}

pub(crate) fn is_protected_branch(branch: &str) -> bool {
    matches!(
        branch,
        "main" | "master" | "develop" | "devel" | "dev" | "trunk"
    )
}

#[cfg(test)]
mod tests {
    use super::{checkout_branch_after_create, parse_leading_issue_number};
    use crate::adapters::backend::BranchCreateOutcome;
    use crate::adapters::git::GitBackend;
    use crate::error::RiptaskError;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeGit {
        branch_exists: bool,
        has_head_commit: bool,
        checkout_calls: Arc<Mutex<usize>>,
        fetch_tracking_calls: Arc<Mutex<usize>>,
        ensure_on_local_branch_calls: Arc<Mutex<usize>>,
    }

    impl FakeGit {
        fn with_state(branch_exists: bool, has_head_commit: bool) -> Self {
            Self {
                branch_exists,
                has_head_commit,
                ..Self::default()
            }
        }
    }

    impl GitBackend for FakeGit {
        fn init(&self, _path: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn add(&self, _repo: &Path, _files: &[&Path]) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn commit(&self, _repo: &Path, _message: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn repo_root(&self, _cwd: &Path) -> Result<std::path::PathBuf, RiptaskError> {
            unimplemented!()
        }
        fn merge_file(
            &self,
            _local: &Path,
            _base: &Path,
            _remote: &Path,
        ) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn has_changes(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn has_uncommitted_changes(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn pull(&self, _repo: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn push(&self, _repo: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn checkout(&self, _repo: &Path, _branch: &str) -> Result<(), RiptaskError> {
            *self.checkout_calls.lock().expect("lock") += 1;
            Ok(())
        }
        fn create_branch(&self, _repo: &Path, _name: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn ensure_on_local_branch(&self, _repo: &Path, _slug: &str) -> Result<(), RiptaskError> {
            *self.ensure_on_local_branch_calls.lock().expect("lock") += 1;
            Ok(())
        }
        fn branch_exists(&self, _repo: &Path, _name: &str) -> Result<bool, RiptaskError> {
            Ok(self.branch_exists)
        }
        fn fetch_and_checkout_tracking(
            &self,
            _repo: &Path,
            _branch: &str,
        ) -> Result<(), RiptaskError> {
            *self.fetch_tracking_calls.lock().expect("lock") += 1;
            Ok(())
        }
        fn push_with_upstream(&self, _repo: &Path, _branch: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn has_working_tree_changes(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn diff_names(&self, _repo: &Path) -> Result<Vec<String>, RiptaskError> {
            unimplemented!()
        }
        fn current_branch(&self, _repo: &Path) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn remote_url(&self, _repo: &Path, _remote: &str) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn delete_local_branch(
            &self,
            _repo: &Path,
            _branch: &str,
            _force: bool,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn has_staged_changes(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn stage_all(&self, _repo: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn is_branch_merged(
            &self,
            _repo: &Path,
            _branch: &str,
            _base: &str,
        ) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn fetch(&self, _repo: &Path) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn commits_ahead_of_base(
            &self,
            _repo: &Path,
            _branch: &str,
            _base: &str,
        ) -> Result<u64, RiptaskError> {
            unimplemented!()
        }
        fn create_empty_commit(&self, _repo: &Path, _message: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn push_orphan_initial_branch(
            &self,
            _repo: &Path,
            _ref_name: &str,
            _message: &str,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn find_commit_by_subject(
            &self,
            _repo: &Path,
            _branch: &str,
            _base: &str,
            _subject: &str,
        ) -> Result<Option<String>, RiptaskError> {
            unimplemented!()
        }
        fn rebase_drop_commit(
            &self,
            _repo: &Path,
            _commit_sha: &str,
            _branch: &str,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn force_push_with_lease(&self, _repo: &Path, _branch: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn force_push(&self, _repo: &Path, _branch: &str) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn log_between(
            &self,
            _repo: &Path,
            _base: &str,
            _head: &str,
        ) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn diff_between(
            &self,
            _repo: &Path,
            _base: &str,
            _head: &str,
        ) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn working_tree_diff(&self, _repo: &Path) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn staged_diff(&self, _repo: &Path) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn head_sha(&self, _repo: &Path) -> Result<String, RiptaskError> {
            unimplemented!()
        }
        fn has_head_commit(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            Ok(self.has_head_commit)
        }
        fn remote_has_any_refs(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn clone_with_reference(
            &self,
            _reference_repo: &Path,
            _remote_url: &str,
            _target_dir: &Path,
        ) -> Result<(), RiptaskError> {
            unimplemented!()
        }
        fn stash_push(&self, _repo: &Path, _message: &str) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
        fn stash_pop(&self, _repo: &Path) -> Result<bool, RiptaskError> {
            unimplemented!()
        }
    }

    #[test]
    fn parses_number_prefix_with_slug() {
        assert_eq!(parse_leading_issue_number("139-foo-bar"), Some(139));
    }

    #[test]
    fn parses_bare_number() {
        assert_eq!(parse_leading_issue_number("42"), Some(42));
    }

    #[test]
    fn rejects_non_numeric_prefix() {
        assert_eq!(parse_leading_issue_number("main"), None);
        assert_eq!(parse_leading_issue_number("feature/foo"), None);
        assert_eq!(parse_leading_issue_number("RIPTASK--139"), None);
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(parse_leading_issue_number(""), None);
    }

    #[test]
    fn created_branch_fetches_tracking_branch() {
        let git = FakeGit::with_state(false, true);
        checkout_branch_after_create(
            &git,
            Path::new("."),
            "123-test",
            BranchCreateOutcome::Created,
        )
        .expect("checkout");
        assert_eq!(*git.fetch_tracking_calls.lock().expect("lock"), 1);
        assert_eq!(*git.checkout_calls.lock().expect("lock"), 0);
    }

    #[test]
    fn already_existing_local_branch_checks_it_out() {
        let git = FakeGit::with_state(true, true);
        checkout_branch_after_create(
            &git,
            Path::new("."),
            "123-test",
            BranchCreateOutcome::AlreadyExists,
        )
        .expect("checkout");
        assert_eq!(*git.fetch_tracking_calls.lock().expect("lock"), 0);
        assert_eq!(*git.checkout_calls.lock().expect("lock"), 1);
    }

    #[test]
    fn empty_remote_with_unborn_head_renames_to_slug() {
        // On unborn HEAD, ensure_on_local_branch rewrites HEAD's symref so
        // the user is on the issue branch without making a commit.
        let git = FakeGit::with_state(false, false);
        checkout_branch_after_create(
            &git,
            Path::new("."),
            "123-test",
            BranchCreateOutcome::EmptyRemote,
        )
        .expect("checkout");
        assert_eq!(*git.fetch_tracking_calls.lock().expect("lock"), 0);
        assert_eq!(*git.ensure_on_local_branch_calls.lock().expect("lock"), 1);
    }

    #[test]
    fn empty_remote_with_existing_history_creates_local_branch() {
        let git = FakeGit::with_state(false, true);
        checkout_branch_after_create(
            &git,
            Path::new("."),
            "123-test",
            BranchCreateOutcome::EmptyRemote,
        )
        .expect("checkout");
        assert_eq!(*git.ensure_on_local_branch_calls.lock().expect("lock"), 1);
        assert_eq!(*git.fetch_tracking_calls.lock().expect("lock"), 0);
    }
}
