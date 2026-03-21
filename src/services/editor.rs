use crate::error::RiptskError;
use anyhow::Context;
use std::path::Path;
use std::process::ExitStatus;

/// Launch `$EDITOR` on the given file path. Returns the exit status so callers
/// can decide how to handle non-zero exits.
pub fn open_in_editor(path: &Path) -> Result<ExitStatus, RiptskError> {
    let editor =
        std::env::var("EDITOR").map_err(|_| RiptskError::General("set $EDITOR to edit".into()))?;
    let parts: Vec<&str> = editor.split_whitespace().collect();
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| RiptskError::General("empty $EDITOR".into()))?;
    std::process::Command::new(program)
        .args(args)
        .arg(path)
        .status()
        .context("failed to launch editor")
        .map_err(RiptskError::Other)
}
