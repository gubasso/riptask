use crate::adapters::ai::AiBackend;
use crate::adapters::git::GitBackend;
use crate::adapters::prompts::PromptBackend;
use crate::config::{Config, load_config, save_config};
use crate::error::{ProjectKeyCollision, ProjectKeyProjectMeta, RiptaskError};
use crate::models::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};
use crate::paths::AppPaths;
use crate::services::auto_commit::maybe_auto_commit;
use crate::services::{issue_ids, repo_project_label};
use anyhow::Context;
use camino::Utf8Path;
use std::io::IsTerminal;
use std::process::Command;

pub fn ensure_registered(
    paths: &AppPaths,
    git: &dyn GitBackend,
    prompts: &dyn PromptBackend,
) -> Result<(), RiptaskError> {
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
            paths.riptask_repo.as_std_path(),
            "riptsk: auto-register project",
            &[config_path.as_std_path()],
        )?;
    }
    Ok(())
}

pub fn detect_from_cwd<'a>(
    cwd: &Utf8Path,
    config: &'a Config,
) -> Result<Option<&'a RepoProject>, RiptaskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()
        .context("failed to detect git remote")
        .map_err(RiptaskError::Other)?;

    if output.status.success() && !git_root_is_home(cwd)? {
        let url = String::from_utf8_lossy(&output.stdout);
        let normalized = normalize_url(url.trim());
        if let Some(repo_project) = config
            .projects
            .iter()
            .find(|repo_project| normalized == normalized_vc_backend(&repo_project.vc_backend))
        {
            return Ok(Some(repo_project));
        }
    }

    Ok(config.projects.iter().find(|repo_project| {
        repo_project
            .vc_backend
            .path
            .as_ref()
            .is_some_and(|path| path_is_under(cwd.as_str(), path))
            || repo_project
                .tasks_backend
                .path
                .as_ref()
                .is_some_and(|path| path_is_under(cwd.as_str(), path))
    }))
}

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

pub fn infer_type(host: &str) -> BackendKind {
    match host {
        "github.com" => BackendKind::Github,
        value if value.contains("gitlab") => BackendKind::Gitlab,
        _ => BackendKind::Local,
    }
}

pub fn normalized_vc_backend(vc_backend: &VCBackendSpec) -> String {
    match (&vc_backend.host, &vc_backend.repo) {
        (Some(host), Some(repo)) => {
            let host = host
                .strip_prefix("https://")
                .or_else(|| host.strip_prefix("http://"))
                .unwrap_or(host);
            format!("{}:{}", host.trim_end_matches('/'), repo)
        }
        (None, Some(repo)) if vc_backend.kind == BackendKind::Github => {
            format!("github.com:{repo}")
        }
        (None, Some(repo)) if vc_backend.kind == BackendKind::Gitlab => {
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
) -> Result<Option<RepoProject>, RiptaskError> {
    if is_home_dir(cwd) {
        return Ok(None);
    }

    if let Some(existing) = detect_from_cwd(cwd, config)? {
        return Ok(Some(existing.clone()));
    }

    if !git_root_is_home(cwd)? {
        if let Some(url) = git_origin_url(cwd)? {
            let normalized = normalize_url(url.trim());
            let (host, repo) = split_host_repo(&normalized).ok_or_else(|| {
                RiptaskError::General("failed to normalize git remote URL".into())
            })?;
            let repo_tail = repo.rsplit('/').next().unwrap_or(repo);
            let Some(name) = deduplicate_name(repo_tail, cwd, config) else {
                return Ok(None);
            };
            let host_url = format!("https://{}", host.trim_end_matches('/'));
            let mut repo_project = RepoProject {
                name,
                vc_backend: VCBackendSpec {
                    kind: infer_type(host),
                    host: Some(host_url.clone()),
                    repo: Some(repo.to_owned()),
                    path: None,
                },
                tasks_backend: TasksBackendSpec {
                    kind: infer_type(host),
                    host: Some(host_url),
                    repo: Some(repo.to_owned()),
                    jira_project: None,
                    default_issue_type: None,
                    path: None,
                },
                default_board: Some("personal".into()),
                default_org: None,
                key: None,
                // Only Jira TasksBackends use `repo_project_label`; auto-
                // registration in this branch always targets GitHub/GitLab via
                // `infer_type(host)`, so leave it unset. This also avoids the
                // edge case where `derive_default` can sanitize down to `""`
                // (e.g. for emoji-only or all-punctuation directory names),
                // which would otherwise fail validation for a project that
                // does not use the label at all.
                repo_project_label: None,
            };
            validate_new_repo_project(config, &repo_project)?;
            finalize_repo_project_registration(config, &mut repo_project, prompts, ai)?;
            config.projects.push(repo_project.clone());
            return Ok(Some(repo_project));
        }

        if is_inside_work_tree(cwd)? {
            let Some(name) = deduplicate_name(cwd.file_name().unwrap_or("project"), cwd, config)
            else {
                return Ok(None);
            };
            let path = std::fs::canonicalize(cwd.as_std_path())
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|_| cwd.as_str().to_owned());
            let mut repo_project = RepoProject {
                name,
                vc_backend: VCBackendSpec {
                    kind: BackendKind::Local,
                    host: None,
                    repo: None,
                    path: Some(path.clone()),
                },
                tasks_backend: TasksBackendSpec {
                    kind: BackendKind::Local,
                    host: None,
                    repo: None,
                    jira_project: None,
                    default_issue_type: None,
                    path: Some(path),
                },
                default_board: Some("personal".into()),
                default_org: None,
                key: None,
                // Local/Local RepoProjects do not use `repo_project_label`.
                repo_project_label: None,
            };
            validate_new_repo_project(config, &repo_project)?;
            finalize_repo_project_registration(config, &mut repo_project, prompts, ai)?;
            config.projects.push(repo_project.clone());
            return Ok(Some(repo_project));
        }
    }

    let Some(name) = deduplicate_name(cwd.file_name().unwrap_or("project"), cwd, config) else {
        return Ok(None);
    };
    let path = std::fs::canonicalize(cwd.as_std_path())
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|_| cwd.as_str().to_owned());
    let mut repo_project = RepoProject {
        name,
        vc_backend: VCBackendSpec {
            kind: BackendKind::Local,
            host: None,
            repo: None,
            path: Some(path.clone()),
        },
        tasks_backend: TasksBackendSpec {
            kind: BackendKind::Local,
            host: None,
            repo: None,
            jira_project: None,
            default_issue_type: None,
            path: Some(path),
        },
        default_board: Some("personal".into()),
        default_org: None,
        key: None,
        // Non-git, Local/Local RepoProject — `repo_project_label` is a Jira
        // partitioning concept and has no effect here, so leave it unset.
        repo_project_label: None,
    };
    validate_new_repo_project(config, &repo_project)?;
    finalize_repo_project_registration(config, &mut repo_project, prompts, ai)?;
    config.projects.push(repo_project.clone());
    Ok(Some(repo_project))
}

fn deduplicate_name(base: &str, cwd: &Utf8Path, config: &Config) -> Option<String> {
    if !config
        .projects
        .iter()
        .any(|repo_project| repo_project.name == base)
    {
        return Some(base.to_owned());
    }
    let parent = cwd.parent().and_then(|p| p.file_name()).unwrap_or("dup");
    let candidate = format!("{parent}-{base}");
    if !config
        .projects
        .iter()
        .any(|repo_project| repo_project.name == candidate)
    {
        return Some(candidate);
    }
    None
}

fn split_host_repo(normalized: &str) -> Option<(&str, &str)> {
    if let Some(pos) = normalized.rfind(':') {
        let candidate = &normalized[pos + 1..];
        if candidate.contains('/') {
            return Some((&normalized[..pos], candidate));
        }
    }
    normalized.split_once(':')
}

fn git_origin_url(cwd: &Utf8Path) -> Result<Option<String>, RiptaskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .output()
        .context("failed to detect git remote")
        .map_err(RiptaskError::Other)?;
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
    repo_project_label_override: Option<String>,
) -> Result<RepoProject, RiptaskError> {
    if let Some(existing) = detect_from_cwd(cwd, config)? {
        return Ok(existing.clone());
    }

    let repo_url = git_origin_url(cwd)?;
    let (default_name, default_vc_kind, default_host, default_repo, default_path, origin_tail) =
        if let Some(url) = repo_url {
            let normalized = normalize_url(&url);
            let (host, repo) = split_host_repo(&normalized).ok_or_else(|| {
                RiptaskError::General("failed to normalize git remote URL".into())
            })?;
            (
                repo.rsplit('/').next().unwrap_or(repo).to_owned(),
                infer_type(host),
                Some(host.to_owned()),
                Some(repo.to_owned()),
                None,
                Some(repo.rsplit('/').next().unwrap_or(repo).to_owned()),
            )
        } else {
            (
                cwd.file_name().unwrap_or("project").to_owned(),
                BackendKind::Local,
                None,
                None,
                Some(
                    std::fs::canonicalize(cwd.as_std_path())
                        .map(|path| path.to_string_lossy().to_string())
                        .unwrap_or_else(|_| cwd.as_str().to_owned()),
                ),
                None,
            )
        };

    let name = prompts.input("RepoProject name", Some(&default_name))?;
    let vc_kind = select_backend_kind(
        prompts,
        "VCBackend type",
        &["github", "gitlab", "local"],
        default_vc_kind.clone(),
    )?;
    let vc_host = if vc_kind == BackendKind::Local {
        None
    } else {
        let default_host = default_host.as_deref().unwrap_or(match vc_kind {
            BackendKind::Github => "github.com",
            BackendKind::Gitlab => "gitlab.com",
            BackendKind::Jira | BackendKind::Local => "",
        });
        let host = prompts.input("VCBackend host", Some(default_host))?;
        Some(format!(
            "https://{}",
            host.trim_start_matches("https://")
                .trim_start_matches("http://")
        ))
    };
    let vc_repo = if vc_kind == BackendKind::Local {
        None
    } else {
        Some(prompts.input("VCBackend repo", default_repo.as_deref())?)
    };

    let default_tasks_kind = if vc_kind == BackendKind::Local {
        BackendKind::Local
    } else {
        vc_kind.clone()
    };
    let tasks_kind = select_backend_kind(
        prompts,
        "TasksBackend type",
        &["github", "gitlab", "jira", "local"],
        default_tasks_kind,
    )?;
    let tasks_host = if tasks_kind == BackendKind::Local {
        None
    } else {
        let default_tasks_host = default_host.as_deref().unwrap_or(match tasks_kind {
            BackendKind::Github => "github.com",
            BackendKind::Gitlab => "gitlab.com",
            BackendKind::Jira => "",
            BackendKind::Local => "",
        });
        let host = prompts.input("TasksBackend host", Some(default_tasks_host))?;
        Some(format!(
            "https://{}",
            host.trim_start_matches("https://")
                .trim_start_matches("http://")
        ))
    };
    let (tasks_repo, jira_project, default_issue_type, tasks_path) = match tasks_kind {
        BackendKind::Github | BackendKind::Gitlab => (
            Some(prompts.input("TasksBackend repo", default_repo.as_deref())?),
            None,
            None,
            None,
        ),
        BackendKind::Jira => (
            None,
            Some(prompts.input("JiraProject", Some("org/PROJ"))?),
            match prompts.input("Default issue type", Some("Task"))? {
                value if value.trim().is_empty() => None,
                value => Some(value),
            },
            None,
        ),
        BackendKind::Local => (None, None, None, default_path.clone()),
    };

    let board_default = config
        .boards
        .first()
        .map(|board| board.name.as_str())
        .unwrap_or("personal");
    let default_board = Some(prompts.input("Default board", Some(board_default))?);
    let derived_label = repo_project_label::derive_default(
        origin_tail.as_deref(),
        cwd.file_name().unwrap_or("project"),
    );
    let mut repo_project = RepoProject {
        name,
        vc_backend: VCBackendSpec {
            kind: vc_kind,
            host: vc_host,
            repo: vc_repo,
            path: default_path.clone(),
        },
        tasks_backend: TasksBackendSpec {
            kind: tasks_kind.clone(),
            host: tasks_host.clone(),
            repo: tasks_repo,
            jira_project: jira_project.clone(),
            default_issue_type,
            path: tasks_path,
        },
        default_board,
        default_org: None,
        key: None,
        repo_project_label: None,
    };

    let shared_jira = tasks_kind == BackendKind::Jira
        && config.projects.iter().any(|existing| {
            existing.tasks_backend.kind == BackendKind::Jira
                && existing.tasks_backend.host == tasks_host
                && existing.tasks_backend.jira_project == jira_project
        });
    // `repo_project_label` is only meaningful for *shared* Jira projects —
    // it partitions one `jira_project` across multiple RepoProjects. Setting
    // it on a solo Jira RepoProject would silently narrow `list_issues` JQL
    // to `AND labels = "<label>"`, hiding every pre-existing Jira issue that
    // lacks the synthetic label. So we only populate the label when:
    //   - the user explicitly passed `--repo-project-label`, OR
    //   - another RepoProject already registered for the same
    //     `(tasks_host, jira_project)` (shared mode, prompt the user).
    // For non-Jira TasksBackends the override flag is rejected and the field
    // stays `None`.
    if tasks_kind == BackendKind::Jira {
        let final_label: Option<String> = if let Some(raw) = repo_project_label_override {
            Some(
                repo_project_label::normalize_user_input(&raw).ok_or_else(|| {
                    RiptaskError::Config(format!(
                        "--repo-project-label '{raw}' sanitizes to an empty suffix after '{}'",
                        repo_project_label::LABEL_PREFIX
                    ))
                })?,
            )
        } else if shared_jira {
            let answer = prompts.input("Repo-project label", derived_label.as_deref())?;
            repo_project_label::normalize_user_input(&answer)
        } else {
            None
        };
        if let Some(label) = final_label {
            repo_project.repo_project_label = Some(deduplicate_default_label(
                config,
                cwd,
                label,
                tasks_host.as_deref(),
                jira_project.as_deref(),
            ));
        }
    } else if repo_project_label_override.is_some() {
        return Err(RiptaskError::Config(
            "--repo-project-label is only valid when the TasksBackend is Jira".into(),
        ));
    }

    validate_new_repo_project(config, &repo_project)?;
    finalize_repo_project_registration(config, &mut repo_project, Some(prompts), ai)?;
    config.projects.push(repo_project.clone());
    Ok(repo_project)
}

fn select_backend_kind(
    prompts: &dyn PromptBackend,
    prompt: &str,
    options: &[&str],
    default: BackendKind,
) -> Result<BackendKind, RiptaskError> {
    let items = options
        .iter()
        .map(|item| item.to_string())
        .collect::<Vec<_>>();
    let default_index = options
        .iter()
        .position(|item| *item == default.as_str())
        .unwrap_or(0);
    let selected = prompts.select(prompt, &items, default_index)?;
    Ok(match selected.as_str() {
        "github" => BackendKind::Github,
        "gitlab" => BackendKind::Gitlab,
        "jira" => BackendKind::Jira,
        _ => BackendKind::Local,
    })
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
    new_repo_project: &RepoProject,
    conflicting: &RepoProject,
    attempted: &str,
    ai_default: Option<&str>,
    existing_keys: &[String],
) -> Result<String, RiptaskError> {
    crate::ui::error(&format!(
        "project key collision: attempted \"{attempted}\" is already used by '{}'",
        conflicting.name
    ));
    eprintln!("Conflicting RepoProject:");
    eprintln!("  name:  {}", conflicting.name);
    eprintln!("  tasks: {}", conflicting.tasks_backend.kind.as_str());
    if let Some(host) = &conflicting.tasks_backend.host {
        eprintln!("  host:  {host}");
    }
    if let Some(repo) = &conflicting.tasks_backend.repo {
        eprintln!("  repo:  {repo}");
    }
    if let Some(jira_project) = &conflicting.tasks_backend.jira_project {
        eprintln!("  jira_project:  {jira_project}");
    }
    eprintln!("New RepoProject:");
    eprintln!("  name:  {}", new_repo_project.name);
    eprintln!("  tasks: {}", new_repo_project.tasks_backend.kind.as_str());

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

fn finalize_repo_project_registration(
    config: &Config,
    new_repo_project: &mut RepoProject,
    prompts: Option<&dyn PromptBackend>,
    ai: Option<&dyn AiBackend>,
) -> Result<(), RiptaskError> {
    let new_group = issue_ids::logical_group_identity(new_repo_project);

    if let Some(sibling) = config
        .projects
        .iter()
        .find(|repo_project| issue_ids::logical_group_identity(repo_project) == new_group)
    {
        new_repo_project.key = Some(issue_ids::effective_key(sibling));
        return Ok(());
    }

    let derived = issue_ids::derive_default_key(new_repo_project);
    let mut existing_keys = Vec::new();
    let mut conflicting = None;
    for repo_project in &config.projects {
        if issue_ids::logical_group_identity(repo_project) == new_group {
            continue;
        }
        let key = issue_ids::effective_key(repo_project);
        if key == derived && conflicting.is_none() {
            conflicting = Some(repo_project);
        }
        existing_keys.push(key);
    }
    if conflicting.is_none() {
        new_repo_project.key = Some(derived);
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
                backend_key_source_name(new_repo_project),
                new_repo_project.tasks_backend.kind.as_str(),
                &existing_keys,
            )
        });
        let resolved = resolve_key_conflict_interactive(
            prompts,
            new_repo_project,
            conflicting,
            &derived,
            ai_default.as_deref(),
            &existing_keys,
        )?;
        new_repo_project.key = Some(resolved);
        return Ok(());
    }

    Err(RiptaskError::KeyCollision(Box::new(ProjectKeyCollision {
        attempted_key: derived,
        new_project: project_meta(new_repo_project),
        conflicting_project: project_meta(conflicting),
    })))
}

fn backend_key_source_name(repo_project: &RepoProject) -> &str {
    repo_project
        .tasks_backend
        .jira_project
        .as_deref()
        .and_then(|jira_project| jira_project.rsplit('/').next())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            repo_project
                .vc_backend
                .repo
                .as_deref()
                .and_then(|repo| repo.rsplit('/').next())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(repo_project.name.as_str())
}

fn deduplicate_default_label(
    config: &Config,
    cwd: &Utf8Path,
    label: String,
    jira_host: Option<&str>,
    jira_project: Option<&str>,
) -> String {
    let temp_repo_project = RepoProject {
        name: "__new__".into(),
        vc_backend: VCBackendSpec {
            kind: BackendKind::Local,
            host: None,
            repo: None,
            path: None,
        },
        tasks_backend: TasksBackendSpec {
            kind: if jira_project.is_some() {
                BackendKind::Jira
            } else {
                BackendKind::Local
            },
            host: jira_host.map(str::to_owned),
            repo: None,
            jira_project: jira_project.map(str::to_owned),
            default_issue_type: None,
            path: None,
        },
        default_board: None,
        default_org: None,
        key: None,
        repo_project_label: Some(label.clone()),
    };
    repo_project_label::deduplicate_within_jira_project(
        &label,
        &temp_repo_project,
        config.projects.iter(),
        cwd.parent().and_then(|parent| parent.file_name()),
    )
}

fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

fn stdout_is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

fn numeric_suffix_suggestion(attempted: &str, existing: &[String]) -> String {
    let reserved = "-999".len();
    let max_prefix_len = issue_ids::MAX_KEY_LEN.saturating_sub(reserved);
    let mut prefix = attempted;
    if prefix.len() > max_prefix_len {
        prefix = &prefix[..max_prefix_len];
    }
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
    let trimmed = raw.trim();
    issue_ids::validate_explicit_key_syntax(trimmed)?;
    if existing.iter().any(|key| key == trimmed) {
        return Err(format!("key '{trimmed}' is already in use"));
    }
    Ok(trimmed.to_owned())
}

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

fn git_toplevel(cwd: &Utf8Path) -> Result<Option<String>, RiptaskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .context("failed to detect git top-level")
        .map_err(RiptaskError::Other)?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

fn git_root_is_home(cwd: &Utf8Path) -> Result<bool, RiptaskError> {
    if let Some(toplevel) = git_toplevel(cwd)? {
        Ok(is_home_dir(Utf8Path::new(&toplevel)))
    } else {
        Ok(false)
    }
}

fn is_inside_work_tree(cwd: &Utf8Path) -> Result<bool, RiptaskError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .context("failed to inspect git work tree")
        .map_err(RiptaskError::Other)?;
    Ok(output.status.success())
}

fn validate_new_repo_project(
    config: &Config,
    repo_project: &RepoProject,
) -> Result<(), RiptaskError> {
    if config
        .projects
        .iter()
        .any(|candidate| candidate.name == repo_project.name)
    {
        return Err(RiptaskError::Config(format!(
            "RepoProject name already exists: {}",
            repo_project.name
        )));
    }
    if repo_project.vc_backend.kind == BackendKind::Jira {
        return Err(RiptaskError::Config(format!(
            "RepoProject '{}' cannot use Jira as a VCBackend",
            repo_project.name
        )));
    }
    if repo_project.tasks_backend.kind == BackendKind::Jira {
        let host = repo_project
            .tasks_backend
            .host
            .as_deref()
            .unwrap_or_default();
        if !host.starts_with("https://") {
            return Err(RiptaskError::Config(format!(
                "Jira TasksBackend for RepoProject '{}' requires https:// host",
                repo_project.name
            )));
        }
        let jira_project = repo_project
            .tasks_backend
            .jira_project
            .as_deref()
            .unwrap_or_default();
        if jira_project.is_empty() || !jira_project.contains('/') {
            return Err(RiptaskError::Config(format!(
                "Jira TasksBackend for RepoProject '{}' requires jira_project in org/PROJECT_KEY format",
                repo_project.name
            )));
        }
    }
    if let Some(label) = repo_project.repo_project_label.as_deref() {
        repo_project_label::validate(label)?;
    }
    Ok(())
}

fn project_meta(repo_project: &RepoProject) -> ProjectKeyProjectMeta {
    ProjectKeyProjectMeta {
        name: repo_project.name.clone(),
        backend: repo_project.tasks_backend.kind.as_str().to_owned(),
        host: repo_project
            .tasks_backend
            .host
            .clone()
            .or_else(|| repo_project.vc_backend.host.clone()),
        repo: repo_project
            .tasks_backend
            .repo
            .clone()
            .or_else(|| repo_project.tasks_backend.jira_project.clone())
            .or_else(|| repo_project.vc_backend.repo.clone()),
        path: repo_project
            .vc_backend
            .path
            .clone()
            .or_else(|| repo_project.tasks_backend.path.clone()),
        existing_key: repo_project.key.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        deduplicate_name, detect_from_cwd, infer_type, normalize_url, register_project_auto,
        split_host_repo,
    };
    use crate::config::default_config;
    use crate::models::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};
    use crate::services::repo_project_label::validate as validate_label;
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    fn local_repo_project(name: &str, path: &str) -> RepoProject {
        RepoProject {
            name: name.into(),
            vc_backend: VCBackendSpec {
                kind: BackendKind::Local,
                host: None,
                repo: None,
                path: Some(path.into()),
            },
            tasks_backend: TasksBackendSpec {
                kind: BackendKind::Local,
                host: None,
                repo: None,
                jira_project: None,
                default_issue_type: None,
                path: Some(path.into()),
            },
            default_board: Some("personal".into()),
            default_org: None,
            key: Some("LOCAL".into()),
            repo_project_label: None,
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
        assert_eq!(infer_type("github.com"), BackendKind::Github);
        assert_eq!(infer_type("gitlab.example.com"), BackendKind::Gitlab);
        assert_eq!(infer_type("codeberg.org"), BackendKind::Local);
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
    fn detect_from_cwd_matches_non_git_by_path() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("worktree")).expect("utf8 path");
        std::fs::create_dir_all(cwd.as_std_path()).expect("cwd");
        let mut config = default_config();
        config
            .projects
            .push(local_repo_project("local", cwd.as_str()));

        let detected = detect_from_cwd(&cwd, &config).expect("detect");
        assert_eq!(detected.expect("detected").name, "local");
    }

    #[test]
    fn deduplicate_name_appends_parent_on_collision() {
        let temp = tempdir().expect("temp dir");
        let cwd =
            Utf8PathBuf::from_path_buf(temp.path().join("parent").join("myapp")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("cwd");
        let mut config = default_config();
        config
            .projects
            .push(local_repo_project("myapp", "/tmp/myapp"));

        let name = deduplicate_name("myapp", &cwd, &config);
        assert_eq!(name.as_deref(), Some("parent-myapp"));
    }

    #[test]
    fn register_auto_non_git_creates_local_project() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("repo")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("cwd");
        let mut config = default_config();

        let result = register_project_auto(&cwd, &mut config, None, None).expect("register");
        let registered = result.expect("project");
        assert_eq!(registered.vc_backend.kind, BackendKind::Local);
        // Local/Local RepoProjects do not carry `repo_project_label` — that
        // field is exclusively a Jira-shared-project partition concept.
        assert_eq!(registered.repo_project_label, None);
        assert_eq!(config.projects.len(), 1);
    }

    #[test]
    fn validate_repo_project_rejects_space_in_label() {
        assert!(validate_label("has space").is_err());
    }

    #[test]
    fn validate_repo_project_rejects_empty_label() {
        assert!(validate_label("").is_err());
    }

    #[test]
    fn validate_repo_project_rejects_quote_in_label() {
        assert!(validate_label("has\"quote").is_err());
    }

    #[test]
    fn validate_repo_project_rejects_over_255_chars() {
        let long = format!(
            "{}{}",
            crate::services::repo_project_label::LABEL_PREFIX,
            "a".repeat(crate::services::repo_project_label::MAX_LABEL_LEN)
        );
        assert!(validate_label(&long).is_err());
    }

    #[test]
    fn validate_repo_project_rejects_missing_prefix() {
        assert!(validate_label("good-label").is_err());
    }

    #[test]
    fn validate_repo_project_accepts_prefixed_label() {
        assert!(validate_label("proj::good-label").is_ok());
    }
}
