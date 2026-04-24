use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::{TemplateArgs, TemplateSubcommand};
use crate::config::load_effective_config;
use crate::error::RiptaskError;
use crate::paths::AppPaths;
use crate::services::templates::{TemplateService, seed_template};

fn validate_template_name(name: &str) -> Result<(), RiptaskError> {
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.is_empty() {
        return Err(RiptaskError::General(format!(
            "invalid template name: {name}"
        )));
    }
    Ok(())
}

pub fn run(paths: &AppPaths, args: TemplateArgs) -> Result<(), RiptaskError> {
    paths.require_initialized()?;
    let config = load_effective_config(
        paths,
        &camino::Utf8PathBuf::from(
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
        ),
    )?;
    let service = TemplateService::new(paths, &config);
    match args.subcommand.unwrap_or(TemplateSubcommand::List) {
        TemplateSubcommand::List => {
            for name in service.list_names()? {
                println!("{name}");
            }
            Ok(())
        }
        TemplateSubcommand::Show { name } => {
            let name =
                name.ok_or_else(|| RiptaskError::General("template name required".into()))?;
            validate_template_name(&name)?;
            let document = service.load(&name).map_err(RiptaskError::Other)?;
            println!("{}", document.body);
            Ok(())
        }
        TemplateSubcommand::New { name } => {
            let name =
                name.ok_or_else(|| RiptaskError::General("template name required".into()))?;
            validate_template_name(&name)?;
            let path = paths.templates_dir().join(format!("{name}.md"));
            if path.exists() {
                return Err(RiptaskError::Config(format!(
                    "template already exists: {name}"
                )));
            }
            std::fs::write(&path, seed_template(&name))?;
            service.validate_file(path.as_std_path())?;
            println!("{path}");
            Ok(())
        }
        TemplateSubcommand::Edit { name } => {
            let name =
                name.ok_or_else(|| RiptaskError::General("template name required".into()))?;
            validate_template_name(&name)?;
            let path = paths.templates_dir().join(format!("{name}.md"));
            if !path.exists() {
                return Err(RiptaskError::NotFound(format!("template {name}")));
            }
            open_in_editor(path.as_std_path())?;
            service.validate_file(path.as_std_path())?;
            Ok(())
        }
        TemplateSubcommand::Rm { name } => {
            let name =
                name.ok_or_else(|| RiptaskError::General("template name required".into()))?;
            validate_template_name(&name)?;
            let path = paths.templates_dir().join(format!("{name}.md"));
            if !path.exists() {
                return Err(RiptaskError::NotFound(format!("template {name}")));
            }
            let prompts = DialoguerPrompts;
            if prompts.confirm(&format!("Remove template \"{name}\"?"), false)? {
                std::fs::remove_file(path)?;
            }
            Ok(())
        }
        TemplateSubcommand::Validate { name } => {
            if let Some(name) = name {
                validate_template_name(&name)?;
                let path = paths.templates_dir().join(format!("{name}.md"));
                if !path.exists() {
                    return Err(RiptaskError::NotFound(format!("template {name}")));
                }
                service.validate_file(path.as_std_path())?;
                crate::ui::success(&format!("{name}: valid"));
            } else {
                for name in service.list_names()? {
                    let path = paths.templates_dir().join(format!("{name}.md"));
                    service.validate_file(path.as_std_path())?;
                    crate::ui::success(&format!("{name}: valid"));
                }
            }
            Ok(())
        }
    }
}

fn open_in_editor(path: &std::path::Path) -> Result<(), RiptaskError> {
    let status = crate::services::editor::open_in_editor(path)?;
    if status.success() {
        Ok(())
    } else {
        Err(RiptaskError::General("editor exited unsuccessfully".into()))
    }
}
