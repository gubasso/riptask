use crate::adapters::prompts::DialoguerPrompts;
use crate::cli::RegisterArgs;
use crate::config::{load_config, save_config};
use crate::error::RiptskError;
use crate::paths::AppPaths;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table, presets::NOTHING};
use std::io::IsTerminal;

pub fn run(paths: &AppPaths, args: RegisterArgs) -> Result<(), RiptskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(RiptskError::Other)?;
    if args.list {
        if std::io::stdout().is_terminal() {
            let mut table = Table::new();
            table
                .load_preset(NOTHING)
                .set_content_arrangement(ContentArrangement::Dynamic);
            table.set_header(vec![
                Cell::new("Name")
                    .add_attribute(Attribute::Bold)
                    .add_attribute(Attribute::Dim),
                Cell::new("Type")
                    .add_attribute(Attribute::Bold)
                    .add_attribute(Attribute::Dim),
                Cell::new("Repo")
                    .add_attribute(Attribute::Bold)
                    .add_attribute(Attribute::Dim),
            ]);
            for backend in &config.backends {
                table.add_row(vec![
                    Cell::new(&backend.name).fg(Color::Cyan),
                    Cell::new(backend.backend.as_str()),
                    Cell::new(backend.repo.as_deref().unwrap_or("-")).fg(Color::Grey),
                ]);
            }
            println!("{table}");
        } else {
            for backend in &config.backends {
                println!(
                    "{}\t{}\t{}",
                    backend.name,
                    backend.backend.as_str(),
                    backend.repo.as_deref().unwrap_or("")
                );
            }
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
    let backend = crate::services::project_detection::register_project_interactive(
        &mut config,
        &DialoguerPrompts,
        &cwd,
    )?;
    save_config(paths.config_path().as_std_path(), &config).map_err(RiptskError::Other)?;
    crate::ui::success(&format!(
        "registered {} ({})",
        backend.name,
        backend.backend.as_str()
    ));
    Ok(())
}
