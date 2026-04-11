use crate::cli::{ConfigArgs, ConfigSubcommand};
use crate::config::{config_set, load_config, save_config};
use crate::error::RiptskError;
use crate::paths::AppPaths;
use std::fs;

pub fn run(paths: &AppPaths, args: ConfigArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    match args.subcommand {
        None => {
            let content = fs::read_to_string(paths.config_path())?;
            print!("{content}");
            Ok(())
        }
        Some(ConfigSubcommand::Set { key, value }) => {
            let mut config = load_config(paths.config_path().as_std_path())?;
            config_set(&mut config, &key, &value)?;
            save_config(paths.config_path().as_std_path(), &config)
        }
    }
}
