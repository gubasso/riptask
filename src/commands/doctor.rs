use crate::config::{ConfigScope, load_effective_config};
use crate::error::RiptaskError;
use crate::paths::AppPaths;
use camino::{Utf8Path, Utf8PathBuf};
use std::fs;

pub fn run(paths: &AppPaths) -> Result<(), RiptaskError> {
    let cwd = current_cwd();
    let mut out = String::new();

    out.push_str("riptask doctor\n\n");

    out.push_str("Environment:\n");
    for var in [
        "RIPTASK_REPO",
        "XDG_CONFIG_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "HOME",
        "EDITOR",
    ] {
        let value = std::env::var(var).unwrap_or_else(|_| "(unset)".into());
        out.push_str(&format!("  {var:<16} = {value}\n"));
    }
    out.push('\n');

    out.push_str("Resolved paths:\n");
    out.push_str(&format!("  riptask_repo     = {}\n", paths.riptask_repo));
    out.push_str(&format!(
        "  user_config_root = {}\n",
        paths.user_config_root
    ));
    out.push_str(&format!("  cache_root       = {}\n", paths.cache_root));
    out.push_str(&format!("  state_root       = {}\n", paths.state_root));
    out.push_str(&format!("  current_dir      = {cwd}\n"));
    let project_root = paths.local_config_path(&cwd).and_then(|p| {
        p.parent()
            .and_then(Utf8Path::parent)
            .map(Utf8Path::to_path_buf)
    });
    out.push_str(&format!(
        "  project_root     = {}\n",
        project_root
            .as_ref()
            .map(|p| p.as_str())
            .unwrap_or("(none detected)")
    ));
    out.push('\n');

    out.push_str("Config layers (load order, lowest \u{2192} highest precedence):\n");
    let layers = [
        (ConfigScope::System, Some(paths.system_config_path())),
        (ConfigScope::User, Some(paths.user_config_path())),
        (ConfigScope::Local, paths.local_config_path(&cwd)),
    ];
    for (scope, path) in &layers {
        match path {
            Some(path) if path.exists() => {
                out.push_str(&format!("  [x] {:<7} {}\n", scope.label(), path));
            }
            Some(path) => {
                out.push_str(&format!("  [ ] {:<7} {}  (missing)\n", scope.label(), path));
            }
            None => {
                out.push_str(&format!(
                    "  [ ] {:<7} (no path — not inside a riptask project)\n",
                    scope.label()
                ));
            }
        }
    }
    out.push('\n');

    let dump_path = paths.cache_root.join("effective-config.yaml");
    match load_effective_config(paths, &cwd) {
        Ok(config) => {
            let yaml = serde_yaml_ng::to_string(&config).map_err(|error| {
                RiptaskError::Config(format!("failed to serialize effective config: {error}"))
            })?;
            fs::create_dir_all(&paths.cache_root)?;
            fs::write(&dump_path, &yaml)?;
            out.push_str("Effective configuration:\n");
            out.push_str(&format!("  dump: {dump_path}\n\n"));
            for line in yaml.lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
        Err(error) => {
            out.push_str("Effective configuration:\n");
            out.push_str(&format!("  ERROR: {error}\n"));
        }
    }

    print!("{out}");
    Ok(())
}

fn current_cwd() -> Utf8PathBuf {
    Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    )
}
