use crate::assets::hook::{HOOK_VERSION, PRE_COMMIT_HOOK};
use crate::cli::{HooksArgs, HooksSubcommand};
use crate::error::TskError;
use crate::paths::AppPaths;
use std::fs;

pub fn run(paths: &AppPaths, args: HooksArgs) -> Result<(), TskError> {
    let hook_path = paths.tsk_repo.join(".git/hooks/pre-commit");
    match args.subcommand.unwrap_or(HooksSubcommand::Status) {
        HooksSubcommand::Install { .. } => {
            if let Some(parent) = hook_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&hook_path, PRE_COMMIT_HOOK)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&hook_path)?.permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&hook_path, perms)?;
            }
            Ok(())
        }
        HooksSubcommand::Status => {
            if hook_path.exists() {
                println!("up to date (version: {HOOK_VERSION})");
                Ok(())
            } else {
                println!("not installed");
                Err(TskError::General("hook not installed".into()))
            }
        }
        HooksSubcommand::Update { .. } => {
            if !hook_path.exists() {
                return Err(TskError::General("hook not installed".into()));
            }
            fs::write(&hook_path, PRE_COMMIT_HOOK)?;
            Ok(())
        }
        HooksSubcommand::Uninstall { .. } => {
            if hook_path.exists() {
                fs::remove_file(&hook_path)?;
            }
            Ok(())
        }
    }
}
