use crate::cli::{StoreArgs, StoreSubcommand};
use crate::error::RiptskError;
use crate::paths::AppPaths;

pub fn run(paths: &AppPaths, args: StoreArgs) -> Result<(), RiptskError> {
    match args.subcommand {
        StoreSubcommand::Commit(args) => super::sync_cmd::commit(paths, args),
        StoreSubcommand::Hooks(args) => super::hooks::run(paths, args),
    }
}
