use crate::adapters::git::GitBackend;
use crate::config::Config;
use crate::error::RiptaskError;
use std::path::Path;

pub fn maybe_auto_commit(
    config: &Config,
    git: &dyn GitBackend,
    repo: &Path,
    message: &str,
    files: &[&Path],
) -> Result<(), RiptaskError> {
    if !config.auto_commit {
        return Ok(());
    }
    git.add(repo, files)?;
    match git.commit(repo, message) {
        Ok(()) => Ok(()),
        Err(err) => {
            crate::ui::warn(&format!("auto-commit skipped: {err}"));
            Ok(())
        }
    }
}
