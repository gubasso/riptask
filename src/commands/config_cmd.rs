use crate::adapters::picker::run_fzf_with_args;
use crate::cli::{ConfigArgs, ConfigSubcommand};
use crate::config::{ConfigScope, config_set_scoped, load_effective_config, scaffold_header};
use crate::error::RiptaskError;
use crate::paths::AppPaths;
use crate::services::editor::open_in_editor;
use camino::{Utf8Path, Utf8PathBuf};
use std::fs;

pub fn run(paths: &AppPaths, args: ConfigArgs) -> Result<(), RiptaskError> {
    let cwd = current_cwd();
    match args.subcommand {
        None => {
            let config = load_effective_config(paths, &cwd)?;
            let consulted: Vec<String> = existing_layer_paths(paths, &cwd)
                .into_iter()
                .map(|(_, path)| path.to_string())
                .collect();
            println!("# consulted: {}", consulted.join(", "));
            let content = serde_yaml_ng::to_string(&config).map_err(|error| {
                RiptaskError::Config(format!("failed to serialize config: {error}"))
            })?;
            print!("{content}");
            Ok(())
        }
        Some(ConfigSubcommand::Set { key, value, scope }) => {
            let scope = scope.scope().unwrap_or_else(|| {
                if paths.local_config_path(&cwd).is_some() {
                    ConfigScope::Local
                } else {
                    ConfigScope::User
                }
            });
            config_set_scoped(paths, &cwd, scope, &key, &value)
        }
        Some(ConfigSubcommand::Edit { scope }) => {
            if std::env::var("EDITOR").is_err() {
                return Err(RiptaskError::General("set $EDITOR to edit".into()));
            }
            if let Some(scope) = scope.scope() {
                let path = scope.resolve(paths, &cwd)?;
                ensure_scaffold(&path, scope)?;
                return open_and_revalidate(paths, &cwd, &path);
            }

            let existing = existing_layer_paths(paths, &cwd);
            match existing.as_slice() {
                [] => {
                    let path = ConfigScope::Local.resolve(paths, &cwd)?;
                    ensure_scaffold(&path, ConfigScope::Local)?;
                    open_and_revalidate(paths, &cwd, &path)
                }
                [(_, path)] => open_and_revalidate(paths, &cwd, path),
                _ => {
                    let input = existing
                        .iter()
                        .map(|(_, path)| path.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let Some(selection) = run_fzf_with_args(&input, "config> ", None, &[])? else {
                        return Ok(());
                    };
                    open_and_revalidate(paths, &cwd, &Utf8PathBuf::from(selection))
                }
            }
        }
    }
}

fn current_cwd() -> Utf8PathBuf {
    Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    )
}

fn existing_layer_paths(paths: &AppPaths, cwd: &Utf8Path) -> Vec<(ConfigScope, Utf8PathBuf)> {
    let mut out = Vec::new();
    let system = paths.system_config_path();
    if system.exists() {
        out.push((ConfigScope::System, system));
    }
    let user = paths.user_config_path();
    if user.exists() {
        out.push((ConfigScope::User, user));
    }
    if let Some(local) = paths.local_config_path(cwd)
        && local.exists()
    {
        out.push((ConfigScope::Local, local));
    }
    out
}

fn ensure_scaffold(path: &Utf8Path, scope: ConfigScope) -> Result<(), RiptaskError> {
    if path.exists() {
        return Ok(());
    }
    let dir = path
        .parent()
        .ok_or_else(|| RiptaskError::Config("no parent".into()))?;
    fs::create_dir_all(dir)?;
    fs::write(path, scaffold_header(scope))?;
    Ok(())
}

fn open_and_revalidate(
    paths: &AppPaths,
    cwd: &Utf8Path,
    path: &Utf8Path,
) -> Result<(), RiptaskError> {
    let status = open_in_editor(path.as_std_path())?;
    if !status.success() {
        return Err(RiptaskError::General(format!(
            "editor exited non-zero for {path}"
        )));
    }
    if let Err(error) = load_effective_config(paths, cwd) {
        eprintln!("\n!!! configuration invalid after edit ({path}): {error}");
        eprintln!("!!! file NOT reverted; edit again with `tsk config edit` to fix.");
        return Err(error);
    }
    Ok(())
}
