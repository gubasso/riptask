use crate::cli::{StoreArgs, StoreSubcommand};
use crate::error::RiptaskError;
use crate::paths::AppPaths;

pub fn run(paths: &AppPaths, args: StoreArgs) -> Result<(), RiptaskError> {
    match args.subcommand {
        StoreSubcommand::Commit(args) => super::sync_cmd::commit(paths, args),
        StoreSubcommand::Hooks(args) => super::hooks::run(paths, args),
    }
}
