use crate::adapters::git::{CliGit, GitBackend};
use crate::cli::UncloneArgs;
use crate::error::RiptskError;
use crate::paths::AppPaths;
use crate::storage::work_clone;
use std::path::PathBuf;

pub fn run(_paths: &AppPaths, args: UncloneArgs) -> Result<(), RiptskError> {
    let git = CliGit::new();
    let repo_root = git.repo_root(std::env::current_dir()?.as_path())?;
    let marker = work_clone::load_marker(repo_root.as_path())?
        .ok_or_else(|| RiptskError::General("not a work-clone".into()))?;
    let main_repo_path = PathBuf::from(&marker.main_repo_path);

    if !main_repo_path.exists() {
        return Err(RiptskError::General(format!(
            "main repo path does not exist: {}",
            main_repo_path.display()
        )));
    }

    if !args.force && git.has_uncommitted_changes(repo_root.as_path())? {
        return Err(RiptskError::General(
            "working tree is dirty; commit or stash changes before running `tsk unclone`, or use --force to discard them".into(),
        ));
    }

    git.push(repo_root.as_path())?;
    crate::ui::success(&format!("pushed {} and removing work-clone", marker.branch));
    std::fs::remove_dir_all(&repo_root)?;
    println!("cd {}", main_repo_path.display());
    Ok(())
}
