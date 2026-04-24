use crate::adapters::prompts::DialoguerPrompts;
use crate::cli::RegisterArgs;
use crate::config::{load_effective_config, load_layer, save_layer};
use crate::error::RiptaskError;
use crate::paths::AppPaths;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table, presets::NOTHING};
use std::io::IsTerminal;

pub fn run(paths: &AppPaths, args: RegisterArgs) -> Result<(), RiptaskError> {
    paths.require_initialized()?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    let config = load_effective_config(paths, &cwd)?;
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
                Cell::new("VC")
                    .add_attribute(Attribute::Bold)
                    .add_attribute(Attribute::Dim),
                Cell::new("Tasks")
                    .add_attribute(Attribute::Bold)
                    .add_attribute(Attribute::Dim),
                Cell::new("JiraProject")
                    .add_attribute(Attribute::Bold)
                    .add_attribute(Attribute::Dim),
                Cell::new("Label")
                    .add_attribute(Attribute::Bold)
                    .add_attribute(Attribute::Dim),
            ]);
            for repo_project in &config.projects {
                table.add_row(vec![
                    Cell::new(&repo_project.name).fg(Color::Cyan),
                    Cell::new(format_backend_cell(
                        repo_project.vc_backend.kind.as_str(),
                        repo_project.vc_backend.host.as_deref(),
                        repo_project.vc_backend.repo.as_deref(),
                        repo_project.vc_backend.path.as_deref(),
                    )),
                    Cell::new(format_backend_cell(
                        repo_project.tasks_backend.kind.as_str(),
                        repo_project.tasks_backend.host.as_deref(),
                        repo_project.tasks_backend.repo.as_deref(),
                        repo_project.tasks_backend.path.as_deref(),
                    )),
                    Cell::new(
                        repo_project
                            .tasks_backend
                            .jira_project
                            .as_deref()
                            .unwrap_or("-"),
                    )
                    .fg(Color::Grey),
                    Cell::new(repo_project.repo_project_label.as_deref().unwrap_or(""))
                        .fg(Color::Grey),
                ]);
            }
            println!("{table}");
        } else {
            for repo_project in &config.projects {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    repo_project.name,
                    format_backend_cell(
                        repo_project.vc_backend.kind.as_str(),
                        repo_project.vc_backend.host.as_deref(),
                        repo_project.vc_backend.repo.as_deref(),
                        repo_project.vc_backend.path.as_deref(),
                    ),
                    format_backend_cell(
                        repo_project.tasks_backend.kind.as_str(),
                        repo_project.tasks_backend.host.as_deref(),
                        repo_project.tasks_backend.repo.as_deref(),
                        repo_project.tasks_backend.path.as_deref(),
                    ),
                    repo_project
                        .tasks_backend
                        .jira_project
                        .as_deref()
                        .unwrap_or(""),
                    repo_project.repo_project_label.as_deref().unwrap_or("")
                );
            }
        }
        return Ok(());
    }
    let mut config = config;
    let ai_backend = if config.ai.enabled {
        crate::commands::ai::optional_backend(&config)
    } else {
        None
    };
    let repo_project = crate::services::project_detection::register_project_interactive(
        &mut config,
        &DialoguerPrompts,
        ai_backend
            .as_ref()
            .map(|backend| backend as &dyn crate::adapters::ai::AiBackend),
        &cwd,
        args.repo_project_label,
    )?;
    let sys_path = paths.system_config_path();
    let mut partial = load_layer(sys_path.as_std_path())?.unwrap_or_default();
    partial.projects.push(repo_project.clone());
    save_layer(sys_path.as_std_path(), &partial)?;
    load_effective_config(paths, &cwd)?;
    // TODO(config-scope): honor --scope once vec-mutating commands accept it.
    crate::ui::success(&format!(
        "registered {} (vc: {}, tasks: {})",
        repo_project.name,
        repo_project.vc_backend.kind.as_str(),
        repo_project.tasks_backend.kind.as_str()
    ));
    Ok(())
}

fn format_backend_cell(
    kind: &str,
    host: Option<&str>,
    repo: Option<&str>,
    path: Option<&str>,
) -> String {
    if let Some(path) = path {
        return format!("{kind}@{path}");
    }
    if let Some(repo) = repo {
        if let Some(host) = host {
            return format!("{kind}@{host}/{repo}");
        }
        return format!("{kind}@{repo}");
    }
    if let Some(host) = host {
        return format!("{kind}@{host}");
    }
    kind.to_owned()
}
