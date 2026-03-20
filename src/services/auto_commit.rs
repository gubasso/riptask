use crate::adapters::git::GitBackend;
use crate::config::Config;
use crate::error::TskError;
use std::path::Path;

pub fn maybe_auto_commit(
    config: &Config,
    git: &dyn GitBackend,
    repo: &Path,
    message: &str,
    files: &[&Path],
) -> Result<(), TskError> {
    if !config.auto_commit {
        return Ok(());
    }
    git.add(repo, files)?;
    match git.commit(repo, message) {
        Ok(()) => Ok(()),
        Err(err) => {
            eprintln!("warning: auto-commit skipped: {err}");
            Ok(())
        }
    }
}
