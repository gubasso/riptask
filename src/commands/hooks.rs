use crate::assets::hook::{HOOK_VERSION, PRE_COMMIT_HOOK};
use crate::cli::{HooksArgs, HooksSubcommand};
use crate::error::RiptskError;
use crate::paths::AppPaths;
use console::style;
use std::fs;
use std::io::IsTerminal;

pub fn run(paths: &AppPaths, args: HooksArgs) -> Result<(), RiptskError> {
    let hook_path = paths.riptsk_repo.join(".git/hooks/pre-commit");
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
                if std::io::stdout().is_terminal() {
                    println!(
                        "{} up to date (version: {HOOK_VERSION})",
                        style("✓").green()
                    );
                } else {
                    println!("up to date (version: {HOOK_VERSION})");
                }
                Ok(())
            } else {
                if std::io::stdout().is_terminal() {
                    println!("{} not installed", style("✗").red());
                } else {
                    println!("not installed");
                }
                Err(RiptskError::General("hook not installed".into()))
            }
        }
        HooksSubcommand::Update { .. } => {
            if !hook_path.exists() {
                return Err(RiptskError::General("hook not installed".into()));
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
