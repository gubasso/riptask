use crate::adapters::prompts::DialoguerPrompts;
use crate::cli::RegisterArgs;
use crate::config::{load_config, save_config};
use crate::error::TskError;
use crate::paths::AppPaths;

pub fn run(paths: &AppPaths, args: RegisterArgs) -> Result<(), TskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(TskError::Other)?;
    if args.list {
        for remote in config.remotes {
            println!("{}\t{:?}", remote.name, remote.remote_type);
        }
        return Ok(());
    }
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let mut config = config;
    let remote = crate::services::project_detection::register_project_interactive(
        &mut config,
        &DialoguerPrompts,
        &cwd,
    )?;
    save_config(paths.config_path().as_std_path(), &config).map_err(TskError::Other)?;
    println!("{}\t{:?}", remote.name, remote.remote_type);
    Ok(())
}
