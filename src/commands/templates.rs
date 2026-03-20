use crate::adapters::prompts::{DialoguerPrompts, PromptBackend};
use crate::cli::{TemplateArgs, TemplateSubcommand};
use crate::config::load_config;
use crate::error::TskError;
use crate::paths::AppPaths;
use crate::services::templates::{TemplateService, seed_template};
use anyhow::Context;
use std::process::Command;

fn validate_template_name(name: &str) -> Result<(), TskError> {
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.is_empty() {
        return Err(TskError::General(format!("invalid template name: {name}")));
    }
    Ok(())
}

pub fn run(paths: &AppPaths, args: TemplateArgs) -> Result<(), TskError> {
    paths.require_initialized()?;
    let config = load_config(paths.config_path().as_std_path()).map_err(TskError::Other)?;
    let service = TemplateService::new(paths, &config);
    match args.subcommand.unwrap_or(TemplateSubcommand::List) {
        TemplateSubcommand::List => {
            for name in service.list_names()? {
                println!("{name}");
            }
            Ok(())
        }
        TemplateSubcommand::Show { name } => {
            let name = name.ok_or_else(|| TskError::General("template name required".into()))?;
            validate_template_name(&name)?;
            let document = service.load(&name).map_err(TskError::Other)?;
            println!("{}", document.body);
            Ok(())
        }
        TemplateSubcommand::New { name } => {
            let name = name.ok_or_else(|| TskError::General("template name required".into()))?;
            validate_template_name(&name)?;
            let path = paths.templates_dir().join(format!("{name}.md"));
            if path.exists() {
                return Err(TskError::Config(format!("template already exists: {name}")));
            }
            std::fs::write(&path, seed_template(&name))?;
            service.validate_file(path.as_std_path())?;
            println!("{path}");
            Ok(())
        }
        TemplateSubcommand::Edit { name } => {
            let name = name.ok_or_else(|| TskError::General("template name required".into()))?;
            validate_template_name(&name)?;
            let path = paths.templates_dir().join(format!("{name}.md"));
            if !path.exists() {
                return Err(TskError::NotFound(format!("template {name}")));
            }
            open_in_editor(path.as_std_path())?;
            service.validate_file(path.as_std_path())?;
            Ok(())
        }
        TemplateSubcommand::Rm { name } => {
            let name = name.ok_or_else(|| TskError::General("template name required".into()))?;
            validate_template_name(&name)?;
            let path = paths.templates_dir().join(format!("{name}.md"));
            if !path.exists() {
                return Err(TskError::NotFound(format!("template {name}")));
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
                    return Err(TskError::NotFound(format!("template {name}")));
                }
                service.validate_file(path.as_std_path())?;
                println!("OK");
            } else {
                for name in service.list_names()? {
                    let path = paths.templates_dir().join(format!("{name}.md"));
                    service.validate_file(path.as_std_path())?;
                    println!("{name}: OK");
                }
            }
            Ok(())
        }
    }
}

fn open_in_editor(path: &std::path::Path) -> Result<(), TskError> {
    let editor = std::env::var("EDITOR")
        .map_err(|_| TskError::General("set $EDITOR to edit templates".into()))?;
    let parts: Vec<&str> = editor.split_whitespace().collect();
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| TskError::General("empty $EDITOR".into()))?;
    let status = Command::new(program)
        .args(args)
        .arg(path)
        .status()
        .context("failed to launch editor")
        .map_err(TskError::Other)?;
    if status.success() {
        Ok(())
    } else {
        Err(TskError::General("editor exited unsuccessfully".into()))
    }
}
