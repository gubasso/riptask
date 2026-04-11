use crate::adapters::ai::AiBackend;
use crate::adapters::git::GitBackend;
use crate::adapters::prompts::PromptBackend;
use crate::config::{Config, load_config, save_config};
use crate::error::RiptskError;
use crate::models::{Backend, BackendConfig};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::issue_ids;
use anyhow::Context;
use camino::Utf8Path;
use std::io::IsTerminal;
use std::process::Command;

pub fn ensure_registered(
    paths: &AppPaths,
    git: &dyn GitBackend,
    prompts: &dyn PromptBackend,
) -> Result<(), RiptskError> {
    let config_path = paths.config_path();
    if !config_path.exists() {
        return Ok(());
    }
    let mut config = load_config(config_path.as_std_path())?;
    let cwd = camino::Utf8PathBuf::from(
        std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
    );
    // Best-effort: if git is unavailable or detection fails, skip silently
    // and let commands handle errors with their own messages.
    let detected = match detect_from_cwd(&cwd, &config) {
        Ok(result) => result,
        Err(_) => return Ok(()),
    };
    if detected.is_some() {
        return Ok(());
    }
    let ai_backend = if config.ai.enabled {
        crate::commands::ai::optional_backend(&config)
    } else {
        None
    };
    // Best-effort: if auto-registration fails (e.g. a derived-key collision in
    // a non-interactive environment), skip silently so unrelated commands still
    // run. Commands that require a registered project raise their own errors.
    let registered = match register_project_auto(
        &cwd,
        &mut config,
        Some(prompts),
        ai_backend.as_ref().map(|backend| backend as &dyn AiBackend),
    ) {
        Ok(result) => result,
        Err(_) => return Ok(()),
    };
    if registered.is_some() {
        save_config(config_path.as_std_path(), &config)?;
        maybe_auto_commit(
            &config,
            git,
            paths.riptsk_repo.as_std_path(),
            "riptsk: auto-register project",
            &[config_path.as_std_path()],
        )?;
    }
    Ok(())
}

pub fn detect_from_cwd(
    cwd: &Utf8Path,
    config: &Config,
) -> Result<Option<BackendConfig>, RiptskError> {
    // Git remote URL matching first — skip if git root is $HOME.
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()
        .context("failed to detect git remote")
        .map_err(RiptskError::Other)?;

    if output.status.success() && !git_root_is_home(cwd)? {
        let url = String::from_utf8_lossy(&output.stdout);
        let normalized = normalize_url(url.trim());
        if let Some(backend) = config
            .backends
            .iter()
            .find(|backend| normalized == normalized_backend(backend))
        {
            return Ok(Some(backend.clone()));
        }
    }

    // Path-based matching — works for all project types including non-git.
    if let Some(backend) = config
        .backends
        .iter()
        .find(|backend| {
            backend
                .path
                .as_ref()
                .is_some_and(|p| path_is_under(cwd.as_str(), p))
        })
        .cloned()
    {
        return Ok(Some(backend));
    }

    Ok(None)
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

pub fn infer_type(host: &str) -> Backend {
    match host {
        "github.com" => Backend::Github,
        value if value.contains("gitlab") => Backend::Gitlab,
        _ => Backend::Local,
    }
}

pub fn normalized_backend(backend: &BackendConfig) -> String {
    match (&backend.host, &backend.repo) {
        (Some(host), Some(repo)) => {
            let host = host
                .strip_prefix("https://")
                .or_else(|| host.strip_prefix("http://"))
                .unwrap_or(host);
            format!("{}:{}", host.trim_end_matches('/'), repo)
        }
        (None, Some(repo)) if backend.backend == Backend::Github => {
            format!("github.com:{repo}")
        }
        (None, Some(repo)) if backend.backend == Backend::Gitlab => {
            format!("gitlab.com:{repo}")
        }
        _ => String::new(),
    }
}

pub fn register_project_auto(
    cwd: &Utf8Path,
    config: &mut Config,
    prompts: Option<&dyn PromptBackend>,
    ai: Option<&dyn AiBackend>,
) -> Result<Option<BackendConfig>, RiptskError> {
    // Never auto-register $HOME itself as a project.
    if is_home_dir(cwd) {
        return Ok(None);
    }

    if let Some(existing) = detect_from_cwd(cwd, config)? {
        return Ok(Some(existing));
    }

    // Remote git repo — register by URL (skip if git root is $HOME).
    if !git_root_is_home(cwd)? {
        if let Some(url) = git_origin_url(cwd)? {
            let normalized = normalize_url(url.trim());
            let (host, repo) = split_host_repo(&normalized)
                .ok_or_else(|| RiptskError::General("failed to normalize git remote URL".into()))?;
            let Some(name) = deduplicate_name(repo.rsplit('/').next().unwrap_or(repo), cwd, config)
            else {
                return Ok(None);
            };
            let mut backend = BackendConfig {
                name,
                backend: infer_type(host),
                host: Some(format!("https://{}", host.trim_end_matches('/'))),
                repo: Some(repo.to_owned()),
                default_board: Some("personal".into()),
                default_org: None,
                path: None,
                vc: None,
                default_issue_type: None,
                key: None,
            };
            finalize_backend_registration(config, &mut backend, prompts, ai)?;
            config.backends.push(backend.clone());
            return Ok(Some(backend));
        }

        // Local git repo (no remote) — register by path.
        if is_inside_work_tree(cwd)? {
            let Some(name) = deduplicate_name(cwd.file_name().unwrap_or("project"), cwd, config)
            else {
                return Ok(None);
            };
            let path = std::fs::canonicalize(cwd.as_std_path())
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|_| cwd.as_str().to_owned());
            let mut backend = BackendConfig {
                name,
                backend: Backend::Local,
                host: None,
                repo: None,
                default_board: Some("personal".into()),
                default_org: None,
                path: Some(path),
                vc: None,
                default_issue_type: None,
                key: None,
            };
            finalize_backend_registration(config, &mut backend, prompts, ai)?;
            config.backends.push(backend.clone());
            return Ok(Some(backend));
        }
    }

    // Non-git directory — register as Backend::Local using cwd.
    let Some(name) = deduplicate_name(cwd.file_name().unwrap_or("project"), cwd, config) else {
        return Ok(None);
    };
    let path = std::fs::canonicalize(cwd.as_std_path())
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.as_str().to_owned());
    let mut backend = BackendConfig {
        name,
        backend: Backend::Local,
        host: None,
        repo: None,
        default_board: Some("personal".into()),
        default_org: None,
        path: Some(path),
        vc: None,
        default_issue_type: None,
        key: None,
    };
    finalize_backend_registration(config, &mut backend, prompts, ai)?;
    config.backends.push(backend.clone());
    Ok(Some(backend))
}

/// Ensure the backend name is unique within the config. If `base` already exists,
/// prepend the parent directory name. Returns `None` if uniqueness cannot be achieved.
fn deduplicate_name(base: &str, cwd: &Utf8Path, config: &Config) -> Option<String> {
    if !config.backends.iter().any(|b| b.name == base) {
        return Some(base.to_owned());
    }
    let parent = cwd.parent().and_then(|p| p.file_name()).unwrap_or("dup");
    let candidate = format!("{parent}-{base}");
    if !config.backends.iter().any(|b| b.name == candidate) {
        return Some(candidate);
    }
    None
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
    ai: Option<&dyn AiBackend>,
    cwd: &Utf8Path,
) -> Result<BackendConfig, RiptskError> {
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
                Backend::Local,
                None,
                None,
                Some(
                    std::fs::canonicalize(cwd.as_std_path())
                        .map(|path| path.to_string_lossy().to_string())
                        .unwrap_or_else(|_| cwd.as_str().to_owned()),
                ),
            )
        };

    let name = prompts.input("Backend name", Some(&default_name))?;
    let backend = match prompts
        .select(
            "Backend type",
            &["github".into(), "gitlab".into(), "local".into()],
            match default_type {
                Backend::Github => 0,
                Backend::Gitlab => 1,
                Backend::Jira => 0,
                Backend::Local => 2,
            },
        )?
        .as_str()
    {
        "github" => Backend::Github,
        "gitlab" => Backend::Gitlab,
        _ => Backend::Local,
    };
    let host = if backend == Backend::Local {
        None
    } else {
        let default_host = default_host.as_deref().unwrap_or(match backend {
            Backend::Github => "github.com",
            Backend::Gitlab => "gitlab.com",
            Backend::Jira => "",
            Backend::Local => "",
        });
        let host = prompts.input("Host", Some(default_host))?;
        Some(format!(
            "https://{}",
            host.trim_start_matches("https://")
                .trim_start_matches("http://")
        ))
    };
    let repo = if backend == Backend::Local {
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

    let path = if backend == Backend::Local {
        default_path
    } else {
        None
    };
    let mut backend_config = BackendConfig {
        name,
        backend,
        host,
        repo,
        default_board,
        default_org: None,
        path,
        vc: None,
        default_issue_type: None,
        key: None,
    };
    validate_new_backend(config, &backend_config)?;
    finalize_backend_registration(config, &mut backend_config, Some(prompts), ai)?;
    config.backends.push(backend_config.clone());
    Ok(backend_config)
}

pub(crate) fn suggest_key_with_ai(
    ai: &dyn AiBackend,
    repo_name: &str,
    backend_type: &str,
    existing_keys: &[String],
) -> Option<String> {
    let raw = ai
        .suggest_project_key(repo_name, backend_type, existing_keys)
        .ok()?;
    let sanitized = issue_ids::sanitize_key_candidate(&raw);
    if sanitized == issue_ids::FALLBACK_KEY {
        return None;
    }
    if existing_keys.iter().any(|existing| existing == &sanitized) {
        return None;
    }
    Some(sanitized)
}

pub(crate) fn resolve_key_conflict_interactive(
    prompts: &dyn PromptBackend,
    new_backend: &BackendConfig,
    conflicting: &BackendConfig,
    attempted: &str,
    ai_default: Option<&str>,
    existing_keys: &[String],
) -> Result<String, RiptskError> {
    crate::ui::error(&format!(
        "project key collision: attempted \"{attempted}\" is already used by '{}'",
        conflicting.name
    ));
    eprintln!("Conflicting project:");
    eprintln!("  name:  {}", conflicting.name);
    eprintln!("  type:  {}", conflicting.backend.as_str());
    if let Some(host) = &conflicting.host {
        eprintln!("  host:  {host}");
    }
    if let Some(repo) = &conflicting.repo {
        eprintln!("  repo:  {repo}");
    }
    if let Some(path) = &conflicting.path {
        eprintln!("  path:  {path}");
    }
    if let Some(key) = &conflicting.key {
        eprintln!("  key:   {key}");
    }
    eprintln!("New project:");
    eprintln!("  name:  {}", new_backend.name);
    eprintln!("  type:  {}", new_backend.backend.as_str());
    if let Some(host) = &new_backend.host {
        eprintln!("  host:  {host}");
    }
    if let Some(repo) = &new_backend.repo {
        eprintln!("  repo:  {repo}");
    }
    if let Some(path) = &new_backend.path {
        eprintln!("  path:  {path}");
    }

    let fallback_suggestion = numeric_suffix_suggestion(attempted, existing_keys);
    let default_value = ai_default.unwrap_or(&fallback_suggestion);

    loop {
        let input = prompts.input("Enter a unique project key", Some(default_value))?;
        match validate_user_key(&input, existing_keys) {
            Ok(key) => return Ok(key),
            Err(message) => crate::ui::error(&format!("invalid key: {message}")),
        }
    }
}

fn finalize_backend_registration(
    config: &Config,
    new_backend: &mut BackendConfig,
    prompts: Option<&dyn PromptBackend>,
    ai: Option<&dyn AiBackend>,
) -> Result<(), RiptskError> {
    let new_group = issue_ids::logical_group_identity(new_backend);

    // If the new backend belongs to the same logical project as an existing
    // one, reuse that project's key so every entry resolves to the same
    // effective key at runtime.
    if let Some(sibling) = config
        .backends
        .iter()
        .find(|backend| issue_ids::logical_group_identity(backend) == new_group)
    {
        new_backend.key = Some(issue_ids::effective_key(sibling));
        return Ok(());
    }

    let derived = issue_ids::derive_default_key(new_backend);
    let mut existing_keys = Vec::new();
    let mut conflicting = None;
    for backend in &config.backends {
        if issue_ids::logical_group_identity(backend) == new_group {
            continue;
        }
        let key = issue_ids::effective_key(backend);
        if key == derived && conflicting.is_none() {
            conflicting = Some(backend);
        }
        existing_keys.push(key);
    }
    if conflicting.is_none() {
        new_backend.key = Some(derived);
        return Ok(());
    }

    let conflicting = conflicting.expect("checked");
    if let Some(prompts) = prompts
        && stdin_is_terminal()
        && stdout_is_terminal()
    {
        let ai_default = ai.and_then(|backend| {
            suggest_key_with_ai(
                backend,
                backend_key_source_name(new_backend),
                new_backend.backend.as_str(),
                &existing_keys,
            )
        });
        let resolved = resolve_key_conflict_interactive(
            prompts,
            new_backend,
            conflicting,
            &derived,
            ai_default.as_deref(),
            &existing_keys,
        )?;
        new_backend.key = Some(resolved);
        return Ok(());
    }

    Err(RiptskError::KeyCollision(Box::new(
        crate::error::ProjectKeyCollision {
            attempted_key: derived,
            new_project: crate::error::ProjectKeyProjectMeta {
                name: new_backend.name.clone(),
                backend: new_backend.backend.as_str().to_owned(),
                host: new_backend.host.clone(),
                repo: new_backend.repo.clone(),
                path: new_backend.path.clone(),
                existing_key: new_backend.key.clone(),
            },
            conflicting_project: crate::error::ProjectKeyProjectMeta {
                name: conflicting.name.clone(),
                backend: conflicting.backend.as_str().to_owned(),
                host: conflicting.host.clone(),
                repo: conflicting.repo.clone(),
                path: conflicting.path.clone(),
                existing_key: conflicting.key.clone(),
            },
        },
    )))
}

fn backend_key_source_name(backend: &BackendConfig) -> &str {
    match backend.backend {
        Backend::Github | Backend::Gitlab | Backend::Jira => backend
            .repo
            .as_deref()
            .and_then(|repo| repo.rsplit('/').next())
            .filter(|value| !value.is_empty())
            .unwrap_or(backend.name.as_str()),
        Backend::Local => backend.name.as_str(),
    }
}

fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

fn stdout_is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

fn numeric_suffix_suggestion(attempted: &str, existing: &[String]) -> String {
    // Reserve room for `-N` (up to three digits) so the suggested value still
    // fits inside `MAX_KEY_LEN` once the suffix is appended.
    let reserved = "-999".len();
    let max_prefix_len = issue_ids::MAX_KEY_LEN.saturating_sub(reserved);
    let mut prefix = attempted;
    if prefix.len() > max_prefix_len {
        prefix = &prefix[..max_prefix_len];
    }
    // Trim any trailing '-' that a naive slice may leave behind so the result
    // still passes `validate_user_key`.
    let prefix = prefix.trim_end_matches('-');
    for number in 2u32..1000 {
        let candidate = format!("{prefix}-{number}");
        if !existing.iter().any(|key| key == &candidate) {
            return candidate;
        }
    }
    format!("{prefix}-X")
}

fn validate_user_key(raw: &str, existing: &[String]) -> Result<String, String> {
    // Silently trim the user's input so enter-at-the-prompt typos don't count
    // as hard errors, then delegate syntactic validation to the shared helper
    // so that interactively accepted keys cannot be rejected later by
    // `save_config()`.
    let trimmed = raw.trim();
    issue_ids::validate_explicit_key_syntax(trimmed).map_err(|message| message.to_string())?;
    if existing.iter().any(|key| key == trimmed) {
        return Err(format!("key '{trimmed}' is already in use"));
    }
    Ok(trimmed.to_owned())
}

/// Returns true if `cwd` is exactly `$HOME` (canonicalized comparison).
/// Directories *under* `$HOME` are allowed; only `$HOME` itself is excluded.
fn is_home_dir(cwd: &Utf8Path) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let home_str = home.to_string_lossy();
    path_is_same(cwd.as_str(), &home_str)
}

fn path_is_same(a: &str, b: &str) -> bool {
    let a_canon = std::fs::canonicalize(a)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| a.to_owned());
    let b_canon = std::fs::canonicalize(b)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| b.to_owned());
    a_canon == b_canon
}

/// Returns the git top-level directory for `cwd`, or `None` if not in a git repo.
fn git_toplevel(cwd: &Utf8Path) -> Result<Option<String>, RiptskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .context("failed to detect git top-level")
        .map_err(RiptskError::Other)?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

fn git_root_is_home(cwd: &Utf8Path) -> Result<bool, RiptskError> {
    if let Some(toplevel) = git_toplevel(cwd)? {
        Ok(is_home_dir(Utf8Path::new(&toplevel)))
    } else {
        Ok(false)
    }
}

fn is_inside_work_tree(cwd: &Utf8Path) -> Result<bool, RiptskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .context("failed to inspect git work tree")
        .map_err(RiptskError::Other)?;
    Ok(output.status.success())
}

fn validate_new_backend(config: &Config, backend: &BackendConfig) -> Result<(), RiptskError> {
    if config
        .backends
        .iter()
        .any(|candidate| candidate.name == backend.name)
    {
        return Err(RiptskError::Config(format!(
            "backend name already exists: {}",
            backend.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        deduplicate_name, detect_from_cwd, finalize_backend_registration, infer_type, is_home_dir,
        normalize_url, numeric_suffix_suggestion, path_is_same, register_project_auto,
        resolve_key_conflict_interactive, split_host_repo, suggest_key_with_ai,
    };
    use crate::adapters::ai::{AiBackend, GeneratedIssueContent, TriageSuggestion};
    use crate::adapters::prompts::PromptBackend;
    use crate::config::default_config;
    use crate::error::RiptskError;
    use crate::models::{Backend, BackendConfig};
    use camino::Utf8PathBuf;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use tempfile::tempdir;

    struct FakePrompts {
        inputs: RefCell<VecDeque<String>>,
    }

    impl FakePrompts {
        fn new(inputs: Vec<&str>) -> Self {
            Self {
                inputs: RefCell::new(inputs.into_iter().map(str::to_owned).collect()),
            }
        }
    }

    impl PromptBackend for FakePrompts {
        fn input(&self, _prompt: &str, _default: Option<&str>) -> Result<String, RiptskError> {
            self.inputs
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| RiptskError::General("missing prompt input".into()))
        }

        fn confirm(&self, _prompt: &str, _default: bool) -> Result<bool, RiptskError> {
            unimplemented!()
        }

        fn select(
            &self,
            _prompt: &str,
            _items: &[String],
            _default: usize,
        ) -> Result<String, RiptskError> {
            unimplemented!()
        }
    }

    struct FakeAiBackend {
        suggestion: Option<String>,
        should_error: bool,
    }

    impl AiBackend for FakeAiBackend {
        fn generate_issue_content(
            &self,
            _context: &str,
        ) -> Result<GeneratedIssueContent, RiptskError> {
            unimplemented!()
        }

        fn generate_body(&self, _context: &str) -> Result<String, RiptskError> {
            unimplemented!()
        }

        fn suggest_project_key(
            &self,
            _repo_name: &str,
            _backend_type: &str,
            _existing_keys: &[String],
        ) -> Result<String, RiptskError> {
            if self.should_error {
                Err(RiptskError::General("nope".into()))
            } else {
                Ok(self.suggestion.clone().unwrap_or_default())
            }
        }

        fn generate_pr_description(&self, _context: &str) -> Result<String, RiptskError> {
            unimplemented!()
        }

        fn triage(&self, _issue_context: &str) -> Result<TriageSuggestion, RiptskError> {
            unimplemented!()
        }

        fn summarize(&self, _issues: &str) -> Result<String, RiptskError> {
            unimplemented!()
        }

        fn ask(&self, _question: &str, _context: &str) -> Result<String, RiptskError> {
            unimplemented!()
        }

        fn update_pr_description(&self, _context: &str) -> Result<String, RiptskError> {
            unimplemented!()
        }

        fn generate_commit_message(&self, _diff: &str) -> Result<String, RiptskError> {
            unimplemented!()
        }
    }

    fn backend(kind: Backend, name: &str, repo: Option<&str>, key: Option<&str>) -> BackendConfig {
        BackendConfig {
            name: name.into(),
            backend: kind,
            host: None,
            repo: repo.map(str::to_owned),
            default_board: Some("personal".into()),
            default_org: None,
            path: None,
            vc: None,
            default_issue_type: None,
            key: key.map(str::to_owned),
        }
    }

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
        assert_eq!(infer_type("github.com"), Backend::Github);
        assert_eq!(infer_type("gitlab.example.com"), Backend::Gitlab);
        assert_eq!(infer_type("codeberg.org"), Backend::Local);
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

    #[test]
    fn is_home_dir_matches_actual_home() {
        if let Ok(home) = std::env::var("HOME") {
            let path = Utf8PathBuf::from(&home);
            assert!(is_home_dir(&path));
        }
    }

    #[test]
    fn is_home_dir_rejects_subdir() {
        if let Ok(home) = std::env::var("HOME") {
            let subdir = Utf8PathBuf::from(format!("{home}/some-subdir"));
            assert!(!is_home_dir(&subdir));
        }
    }

    #[test]
    fn path_is_same_handles_identical_paths() {
        let temp = tempdir().expect("temp dir");
        let path = temp.path().to_string_lossy().to_string();
        assert!(path_is_same(&path, &path));
    }

    #[test]
    fn register_auto_non_git_creates_local_backend() {
        let temp = tempdir().expect("temp dir");
        let plain_dir = temp.path().join("my-proj");
        std::fs::create_dir_all(&plain_dir).expect("create dir");
        let cwd = Utf8PathBuf::from_path_buf(plain_dir.clone()).expect("utf8");
        let mut config = default_config();

        let result = register_project_auto(&cwd, &mut config, None, None).expect("register");

        assert!(result.is_some());
        let backend = result.unwrap();
        assert_eq!(backend.backend, Backend::Local);
        assert_eq!(backend.name, "my-proj");
        assert!(backend.path.is_some());
        assert!(backend.host.is_none());
        assert!(backend.repo.is_none());
        assert_eq!(backend.key.as_deref(), Some("MYPROJ"));
    }

    #[test]
    fn register_auto_skips_home_dir() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).expect("utf8");
        let mut config = default_config();

        // Pretend this temp dir is $HOME
        let old_home = std::env::var("HOME").ok();
        // SAFETY: test is single-threaded; restoring HOME immediately after.
        unsafe { std::env::set_var("HOME", temp.path()) };
        let result = register_project_auto(&cwd, &mut config, None, None).expect("register");
        match old_home {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(result.is_none(), "should not register $HOME as a project");
    }

    #[test]
    fn detect_from_cwd_matches_non_git_by_path() {
        let temp = tempdir().expect("temp dir");
        let plain_dir = temp.path().join("my-proj");
        std::fs::create_dir_all(&plain_dir).expect("create dir");
        let cwd = Utf8PathBuf::from_path_buf(plain_dir.clone()).expect("utf8");
        let canon = std::fs::canonicalize(&plain_dir)
            .expect("canonicalize")
            .to_string_lossy()
            .to_string();

        let mut config = default_config();
        config.backends.push(BackendConfig {
            name: "my-proj".into(),
            backend: Backend::Local,
            host: None,
            repo: None,
            default_board: Some("personal".into()),
            default_org: None,
            path: Some(canon),
            vc: None,
            default_issue_type: None,
            key: None,
        });

        let detected = detect_from_cwd(&cwd, &config).expect("detect");
        assert!(detected.is_some());
        assert_eq!(detected.unwrap().name, "my-proj");
    }

    #[test]
    fn detect_from_cwd_url_beats_ancestor_path() {
        // Regression: a registered remote backend must win over a parent local backend
        // when the child directory has a git remote matching the remote backend.
        let temp = tempdir().expect("temp dir");
        let parent_dir = temp.path().join("projects");
        let child_dir = parent_dir.join("my-repo");
        std::fs::create_dir_all(&child_dir).expect("create dirs");

        // Set up a real git repo with a remote URL in child_dir
        let status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&child_dir)
            .status()
            .expect("git init");
        assert!(status.success());
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&child_dir)
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/user/my-repo.git",
            ])
            .status()
            .expect("git remote add");
        assert!(status.success());

        let parent_canon = std::fs::canonicalize(&parent_dir)
            .expect("canonicalize")
            .to_string_lossy()
            .to_string();

        let mut config = default_config();
        // Parent local backend registered at ancestor path
        config.backends.push(BackendConfig {
            name: "projects".into(),
            backend: Backend::Local,
            host: None,
            repo: None,
            default_board: Some("personal".into()),
            default_org: None,
            path: Some(parent_canon),
            vc: None,
            default_issue_type: None,
            key: None,
        });
        // Remote backend registered by URL — should win over parent path
        config.backends.push(BackendConfig {
            name: "my-repo".into(),
            backend: Backend::Github,
            host: Some("https://github.com".into()),
            repo: Some("user/my-repo".into()),
            default_board: Some("personal".into()),
            default_org: None,
            path: None,
            vc: None,
            default_issue_type: None,
            key: None,
        });

        let cwd = Utf8PathBuf::from_path_buf(child_dir).expect("utf8");
        let detected = detect_from_cwd(&cwd, &config).expect("detect");
        assert!(detected.is_some());
        // URL match must win over ancestor path match
        assert_eq!(detected.unwrap().name, "my-repo");
    }

    #[test]
    fn deduplicate_name_appends_parent_on_collision() {
        let temp = tempdir().expect("temp dir");
        let child = temp.path().join("parent").join("myapp");
        std::fs::create_dir_all(&child).expect("create dirs");
        let cwd = Utf8PathBuf::from_path_buf(child).expect("utf8");

        let mut config = default_config();
        config.backends.push(BackendConfig {
            name: "myapp".into(),
            backend: Backend::Local,
            host: None,
            repo: None,
            default_board: None,
            default_org: None,
            path: Some("/some/other/myapp".into()),
            vc: None,
            default_issue_type: None,
            key: None,
        });

        let name = deduplicate_name("myapp", &cwd, &config);
        assert_eq!(name, Some("parent-myapp".into()));
    }

    #[test]
    fn deduplicate_name_returns_none_when_exhausted() {
        let temp = tempdir().expect("temp dir");
        let child = temp.path().join("parent").join("myapp");
        std::fs::create_dir_all(&child).expect("create dirs");
        let cwd = Utf8PathBuf::from_path_buf(child).expect("utf8");

        let mut config = default_config();
        config.backends.push(BackendConfig {
            name: "myapp".into(),
            backend: Backend::Local,
            host: None,
            repo: None,
            default_board: None,
            default_org: None,
            path: Some("/some/other/myapp".into()),
            vc: None,
            default_issue_type: None,
            key: None,
        });
        config.backends.push(BackendConfig {
            name: "parent-myapp".into(),
            backend: Backend::Local,
            host: None,
            repo: None,
            default_board: None,
            default_org: None,
            path: Some("/some/other/parent-myapp".into()),
            vc: None,
            default_issue_type: None,
            key: None,
        });

        let name = deduplicate_name("myapp", &cwd, &config);
        assert_eq!(name, None);
    }

    #[test]
    fn suggest_key_with_ai_accepts_clean_value() {
        let ai = FakeAiBackend {
            suggestion: Some("RIPTSK".into()),
            should_error: false,
        };
        let existing = vec!["OTHER".into()];
        assert_eq!(
            suggest_key_with_ai(&ai, "riptsk", "github", &existing),
            Some("RIPTSK".into())
        );
    }

    #[test]
    fn suggest_key_with_ai_sanitizes_output() {
        let ai = FakeAiBackend {
            suggestion: Some("riptsk!!!".into()),
            should_error: false,
        };
        assert_eq!(
            suggest_key_with_ai(&ai, "riptsk", "github", &[]),
            Some("RIPTSK".into())
        );
    }

    #[test]
    fn suggest_key_with_ai_rejects_collision_and_invalid() {
        let taken = vec!["ALREADYTAKEN".into()];
        let collision = FakeAiBackend {
            suggestion: Some("ALREADY_TAKEN".into()),
            should_error: false,
        };
        let invalid = FakeAiBackend {
            suggestion: Some("!!!".into()),
            should_error: false,
        };
        let erroring = FakeAiBackend {
            suggestion: None,
            should_error: true,
        };
        assert_eq!(
            suggest_key_with_ai(&collision, "repo", "github", &taken),
            None
        );
        assert_eq!(suggest_key_with_ai(&invalid, "repo", "github", &[]), None);
        assert_eq!(suggest_key_with_ai(&erroring, "repo", "github", &[]), None);
    }

    #[test]
    fn resolve_key_conflict_interactive_accepts_ai_default_value() {
        let prompts = FakePrompts::new(vec!["RIPTSK"]);
        let new_backend = backend(Backend::Github, "new", Some("owner/riptsk"), None);
        let conflicting = backend(
            Backend::Github,
            "existing",
            Some("other/riptsk"),
            Some("RIPTSK"),
        );
        let resolved = resolve_key_conflict_interactive(
            &prompts,
            &new_backend,
            &conflicting,
            "RIPTSK",
            Some("RIPTSK"),
            &["OTHER".into()],
        )
        .expect("resolve");
        assert_eq!(resolved, "RIPTSK");
    }

    #[test]
    fn resolve_key_conflict_interactive_loops_on_invalid_inputs() {
        let prompts = FakePrompts::new(vec!["lower-case", "UPPERCASE"]);
        let new_backend = backend(Backend::Github, "new", Some("owner/new"), None);
        let conflicting = backend(Backend::Github, "existing", Some("other/new"), Some("NEW"));
        let resolved = resolve_key_conflict_interactive(
            &prompts,
            &new_backend,
            &conflicting,
            "NEW",
            None,
            &["NEW".into()],
        )
        .expect("resolve");
        assert_eq!(resolved, "UPPERCASE");
    }

    #[test]
    fn resolve_key_conflict_interactive_retries_on_duplicate() {
        let prompts = FakePrompts::new(vec!["USED", "UNUSED"]);
        let new_backend = backend(Backend::Github, "new", Some("owner/new"), None);
        let conflicting = backend(Backend::Github, "existing", Some("other/new"), Some("NEW"));
        let resolved = resolve_key_conflict_interactive(
            &prompts,
            &new_backend,
            &conflicting,
            "NEW",
            None,
            &["USED".into()],
        )
        .expect("resolve");
        assert_eq!(resolved, "UNUSED");
    }

    #[test]
    fn numeric_suffix_suggestion_uses_next_free_number() {
        let existing = vec!["RIPTSK".into(), "RIPTSK-2".into()];
        assert_eq!(numeric_suffix_suggestion("RIPTSK", &existing), "RIPTSK-3");
    }

    #[test]
    fn finalize_backend_registration_sets_derived_key_when_free() {
        let config = default_config();
        let mut new_backend = backend(Backend::Github, "riptsk", Some("owner/riptsk"), None);
        finalize_backend_registration(&config, &mut new_backend, None, None).expect("finalize");
        assert_eq!(new_backend.key.as_deref(), Some("RIPTSK"));
    }

    #[test]
    fn finalize_backend_registration_returns_collision_without_prompts() {
        let mut config = default_config();
        config.backends.push(backend(
            Backend::Github,
            "existing",
            Some("owner/riptsk"),
            None,
        ));
        let before = config.backends.len();
        let mut new_backend = backend(Backend::Github, "new", Some("other/riptsk"), None);
        let error = finalize_backend_registration(&config, &mut new_backend, None, None)
            .expect_err("collision");
        assert!(matches!(error, RiptskError::KeyCollision(_)));
        assert!(new_backend.key.is_none());
        assert_eq!(config.backends.len(), before);
    }
}
