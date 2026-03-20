use crate::adapters::prompts::PromptBackend;
use crate::config::{Config, RemoteConfig, RemoteType};
use crate::error::RiptskError;
use anyhow::Context;
use camino::Utf8Path;
use std::process::Command;

pub fn detect_from_cwd(
    cwd: &Utf8Path,
    config: &Config,
) -> Result<Option<RemoteConfig>, RiptskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()
        .context("failed to detect git remote")
        .map_err(RiptskError::Other)?;

    if !output.status.success() {
        // No git remote — try matching by path for local projects
        return Ok(config
            .remotes
            .iter()
            .find(|remote| {
                remote
                    .path
                    .as_ref()
                    .is_some_and(|p| path_is_under(cwd.as_str(), p))
            })
            .cloned());
    }

    let url = String::from_utf8_lossy(&output.stdout);
    let normalized = normalize_url(url.trim());
    // First try URL-based matching
    if let Some(remote) = config
        .remotes
        .iter()
        .find(|remote| normalized == normalized_remote(remote))
    {
        return Ok(Some(remote.clone()));
    }
    // Fallback: match local projects by path
    Ok(config
        .remotes
        .iter()
        .find(|remote| {
            remote
                .path
                .as_ref()
                .is_some_and(|p| path_is_under(cwd.as_str(), p))
        })
        .cloned())
}

/// Check if `cwd` is the same directory or a subdirectory of `base`, using canonical paths when possible.
fn path_is_under(cwd: &str, base: &str) -> bool {
    let cwd_canon = std::fs::canonicalize(cwd)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.to_owned());
    let base_canon = std::fs::canonicalize(base)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| base.to_owned());
    cwd_canon == base_canon || cwd_canon.starts_with(&format!("{base_canon}/"))
}

pub fn normalize_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
    if let Some(rest) = trimmed.strip_prefix("git@")
        && let Some((host, repo)) = rest.split_once(':')
    {
        return format!("{host}:{repo}");
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://git@")
        && let Some((host, repo)) = rest.split_once('/')
    {
        return format!("{host}:{repo}");
    }
    let stripped = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    if let Some((host, repo)) = stripped.split_once('/') {
        format!("{host}:{repo}")
    } else {
        stripped.to_owned()
    }
}

pub fn infer_type(host: &str) -> RemoteType {
    match host {
        "github.com" => RemoteType::Github,
        value if value.contains("gitlab") => RemoteType::Gitlab,
        _ => RemoteType::Local,
    }
}

pub fn normalized_remote(remote: &RemoteConfig) -> String {
    match (&remote.host, &remote.repo) {
        (Some(host), Some(repo)) => {
            let host = host
                .strip_prefix("https://")
                .or_else(|| host.strip_prefix("http://"))
                .unwrap_or(host);
            format!("{}:{}", host.trim_end_matches('/'), repo)
        }
        (None, Some(repo)) if remote.remote_type == RemoteType::Github => {
            format!("github.com:{repo}")
        }
        (None, Some(repo)) if remote.remote_type == RemoteType::Gitlab => {
            format!("gitlab.com:{repo}")
        }
        _ => String::new(),
    }
}

pub fn register_project_auto(
    cwd: &Utf8Path,
    config: &mut Config,
) -> Result<Option<RemoteConfig>, RiptskError> {
    if let Some(existing) = detect_from_cwd(cwd, config)? {
        return Ok(Some(existing));
    }

    if let Some(url) = git_origin_url(cwd)? {
        let normalized = normalize_url(url.trim());
        // normalized format is "host:repo" or "host:port:repo" for ported URLs.
        // Use rsplit to find the repo part (everything after the last ':' that
        // contains a '/'), falling back to split_once for the common case.
        let (host, repo) = split_host_repo(&normalized)
            .ok_or_else(|| RiptskError::General("failed to normalize git remote URL".into()))?;
        let name = repo.rsplit('/').next().unwrap_or(repo).to_owned();
        let remote = RemoteConfig {
            name: name.clone(),
            remote_type: infer_type(host),
            host: Some(format!("https://{}", host.trim_end_matches('/'))),
            repo: Some(repo.to_owned()),
            default_board: Some("personal".into()),
            default_org: None,
            path: None,
        };
        config.remotes.push(remote.clone());
        return Ok(Some(remote));
    }

    if is_inside_work_tree(cwd)? {
        let name = cwd.file_name().unwrap_or("project").to_owned();
        let path = std::fs::canonicalize(cwd.as_std_path())
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|_| cwd.as_str().to_owned());
        let remote = RemoteConfig {
            name: name.clone(),
            remote_type: RemoteType::Local,
            host: None,
            repo: None,
            default_board: Some("personal".into()),
            default_org: None,
            path: Some(path),
        };
        config.remotes.push(remote.clone());
        return Ok(Some(remote));
    }

    Ok(None)
}

/// Split a normalized "host:repo" or "host:port:repo" string into (host, repo).
/// The repo part is identified as the segment after the last ':' that contains '/'.
fn split_host_repo(normalized: &str) -> Option<(&str, &str)> {
    // Try splitting from the right: if the part after the last ':' contains '/',
    // it's the repo path. Otherwise fall back to split_once for "host:repo".
    if let Some(pos) = normalized.rfind(':') {
        let candidate = &normalized[pos + 1..];
        if candidate.contains('/') {
            return Some((&normalized[..pos], candidate));
        }
    }
    normalized.split_once(':')
}

fn git_origin_url(cwd: &Utf8Path) -> Result<Option<String>, RiptskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()
        .context("failed to detect git remote")
        .map_err(RiptskError::Other)?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

pub fn register_project_interactive(
    config: &mut Config,
    prompts: &dyn PromptBackend,
    cwd: &Utf8Path,
) -> Result<RemoteConfig, RiptskError> {
    if let Some(existing) = detect_from_cwd(cwd, config)? {
        return Ok(existing);
    }

    let repo_url = git_origin_url(cwd)?;
    let (default_name, default_type, default_host, default_repo, default_path) =
        if let Some(url) = repo_url {
            let normalized = normalize_url(&url);
            let (host, repo) = split_host_repo(&normalized)
                .ok_or_else(|| RiptskError::General("failed to normalize git remote URL".into()))?;
            (
                repo.rsplit('/').next().unwrap_or(repo).to_owned(),
                infer_type(host),
                Some(host.to_owned()),
                Some(repo.to_owned()),
                None,
            )
        } else {
            (
                cwd.file_name().unwrap_or("project").to_owned(),
                RemoteType::Local,
                None,
                None,
                Some(
                    std::fs::canonicalize(cwd.as_std_path())
                        .map(|path| path.to_string_lossy().to_string())
                        .unwrap_or_else(|_| cwd.as_str().to_owned()),
                ),
            )
        };

    let name = prompts.input("Remote name", Some(&default_name))?;
    let remote_type = match prompts
        .select(
            "Remote type",
            &["github".into(), "gitlab".into(), "local".into()],
            match default_type {
                RemoteType::Github => 0,
                RemoteType::Gitlab => 1,
                RemoteType::Local => 2,
            },
        )?
        .as_str()
    {
        "github" => RemoteType::Github,
        "gitlab" => RemoteType::Gitlab,
        _ => RemoteType::Local,
    };
    let host = if remote_type == RemoteType::Local {
        None
    } else {
        let default_host = default_host.as_deref().unwrap_or(match remote_type {
            RemoteType::Github => "github.com",
            RemoteType::Gitlab => "gitlab.com",
            RemoteType::Local => "",
        });
        let host = prompts.input("Host", Some(default_host))?;
        Some(format!(
            "https://{}",
            host.trim_start_matches("https://")
                .trim_start_matches("http://")
        ))
    };
    let repo = if remote_type == RemoteType::Local {
        None
    } else {
        Some(prompts.input("Repo", default_repo.as_deref())?)
    };
    let board_default = config
        .boards
        .first()
        .map(|board| board.name.as_str())
        .unwrap_or("personal");
    let default_board = Some(prompts.input("Default board", Some(board_default))?);

    let path = if remote_type == RemoteType::Local {
        default_path
    } else {
        None
    };
    let remote = RemoteConfig {
        name,
        remote_type,
        host,
        repo,
        default_board,
        default_org: None,
        path,
    };
    validate_new_remote(config, &remote)?;
    config.remotes.push(remote.clone());
    Ok(remote)
}

fn is_inside_work_tree(cwd: &Utf8Path) -> Result<bool, RiptskError> {
    let status = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .status()
        .context("failed to inspect git work tree")
        .map_err(RiptskError::Other)?;
    Ok(status.success())
}

fn validate_new_remote(config: &Config, remote: &RemoteConfig) -> Result<(), RiptskError> {
    if config
        .remotes
        .iter()
        .any(|candidate| candidate.name == remote.name)
    {
        return Err(RiptskError::Config(format!(
            "remote name already exists: {}",
            remote.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{infer_type, normalize_url, split_host_repo};
    use crate::config::RemoteType;

    #[test]
    fn normalizes_ssh_and_https_urls() {
        assert_eq!(
            normalize_url("git@gitlab.example.com:chrono/project.git"),
            "gitlab.example.com:chrono/project"
        );
        assert_eq!(
            normalize_url("https://gitlab.example.com/chrono/project/"),
            "gitlab.example.com:chrono/project"
        );
    }

    #[test]
    fn infers_remote_type_from_host() {
        assert_eq!(infer_type("github.com"), RemoteType::Github);
        assert_eq!(infer_type("gitlab.example.com"), RemoteType::Gitlab);
        assert_eq!(infer_type("codeberg.org"), RemoteType::Local);
    }

    #[test]
    fn split_host_repo_handles_standard_urls() {
        assert_eq!(
            split_host_repo("github.com:user/repo"),
            Some(("github.com", "user/repo"))
        );
        assert_eq!(
            split_host_repo("gitlab.example.com:group/project"),
            Some(("gitlab.example.com", "group/project"))
        );
    }

    #[test]
    fn split_host_repo_handles_ported_urls() {
        // ssh://git@host:2222/group/repo normalizes to host:2222:group/repo
        assert_eq!(
            split_host_repo("gitlab.example.com:2222:group/repo"),
            Some(("gitlab.example.com:2222", "group/repo"))
        );
        // https://host:8443/group/repo normalizes to host:8443:group/repo
        assert_eq!(
            split_host_repo("gitlab.example.com:8443:group/repo"),
            Some(("gitlab.example.com:8443", "group/repo"))
        );
    }
}
