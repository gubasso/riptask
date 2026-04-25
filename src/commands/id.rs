use crate::error::RiptaskError;
use crate::paths::AppPaths;
use crate::services::id_resolution;

/// Implementation of `tsk id`: print the issue id associated with the current
/// git branch, or error with a non-zero exit code.
pub fn run(paths: &AppPaths) -> Result<(), RiptaskError> {
    paths.require_shared_layer()?;
    let id = id_resolution::id_for_current_branch(paths)?;
    println!("{id}");
    Ok(())
}
