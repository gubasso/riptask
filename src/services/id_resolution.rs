use crate::adapters::picker::{FzfPicker, IssueDisplayMode, Picker, format_issue_plain};
use crate::cli::{LsArgs, ScopeArgs};
use crate::config::Config;
use crate::error::{RiptskError, StoreError};
use crate::paths::AppPaths;
use crate::services::{issue_ids, issue_service::IssueService, project_detection};
use camino::Utf8Path;

pub fn resolve_id(
    paths: &AppPaths,
    config: &Config,
    cwd: &Utf8Path,
    input: &str,
) -> Result<String, RiptskError> {
    if input.contains("--") {
        return Ok(input.to_owned());
    }

    if !input.chars().all(|character| character.is_ascii_digit()) {
        return Ok(input.to_owned());
    }

    let number = input
        .parse::<u64>()
        .map_err(|_| RiptskError::General(format!("invalid numeric issue ID: {input}")))?;
    let mut detected_candidate = None;

    if let Some(backend) = project_detection::detect_from_cwd(cwd, config)? {
        let scope = issue_ids::derive_scope_from_backend(&backend);
        let full_id = issue_ids::format_id(&scope, number);
        if crate::storage::issue_store::find_issue(paths, &full_id).is_ok() {
            return Ok(full_id);
        }
        detected_candidate = Some(full_id);
    }

    let suffix = format!("--{number}");
    let mut matches = crate::storage::issue_store::list_issues(paths)?
        .into_iter()
        .filter_map(|path| {
            path.file_stem()
                .map(str::to_owned)
                .filter(|id| id.ends_with(&suffix))
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();

    match matches.len() {
        1 => Ok(matches.pop().unwrap_or_default()),
        0 => Err(
            StoreError::FileNotFound(detected_candidate.unwrap_or_else(|| input.to_owned())).into(),
        ),
        _ => Err(RiptskError::General(format!(
            "ambiguous numeric ID {number}: matches {}",
            matches.join(", ")
        ))),
    }
}

pub fn require_id(
    paths: &AppPaths,
    config: &Config,
    cwd: &Utf8Path,
    id: Option<String>,
    scope_args: &ScopeArgs,
) -> Result<Option<String>, RiptskError> {
    if let Some(input) = id {
        return resolve_id(paths, config, cwd, &input).map(Some);
    }

    let scope =
        crate::scope::resolve_scope(&scope_args.projects, scope_args.all_projects, cwd, config)?;
    let issues = IssueService::new(paths, config).list_matching(&LsArgs::default(), &scope)?;
    let mode = if scope_args.all_projects {
        IssueDisplayMode::AllProjects
    } else {
        IssueDisplayMode::PerProject
    };
    let issues_dir = paths.issues_dir();
    let picker = FzfPicker {
        fzf_opts: config.ui.fzf_opts.clone(),
    };
    match picker.pick_issue(&issues, "issue> ", Some(issues_dir.as_str()), &mode) {
        Ok(Some(id)) => Ok(Some(id)),
        Ok(None) => Ok(None),
        Err(e) => {
            // fzf not installed — print available issues to stderr as fallback
            if !issues.is_empty() {
                eprintln!("Available issues:");
                for issue in &issues {
                    eprintln!("  {}", format_issue_plain(issue));
                }
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_id;
    use crate::config::{Config, default_config};
    use crate::models::{Backend, BackendConfig};
    use crate::paths::AppPaths;
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    #[test]
    fn resolve_id_full_id_passes_through() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).expect("utf8 cwd");
        let paths = app_paths(temp.path());
        let config = default_config();

        let resolved =
            resolve_id(&paths, &config, &cwd, "GH-GUB-DEV--61").expect("resolve full id");

        assert_eq!(resolved, "GH-GUB-DEV--61");
    }

    #[test]
    fn resolve_id_numeric_uses_detected_project() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("worktree")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("worktree dir");
        let paths = app_paths(temp.path());
        touch_issue(&paths, "GH-GUB-DEV--61");
        touch_issue(&paths, "GL-FOO-BAR--61");
        let config = config_with_backends(vec![BackendConfig {
            name: "dev-tools".into(),
            backend: Backend::Github,
            host: None,
            repo: Some("GubCorp/dev-tools".into()),
            default_board: Some("personal".into()),
            default_org: None,
            path: Some(cwd.to_string()),
        }]);

        let resolved = resolve_id(&paths, &config, &cwd, "61").expect("resolve numeric id");

        assert_eq!(resolved, "GH-GUB-DEV--61");
    }

    #[test]
    fn resolve_id_numeric_without_detected_project_uses_unique_suffix_match() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("outside")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("outside dir");
        let paths = app_paths(temp.path());
        touch_issue(&paths, "GH-GUB-DEV--61");
        touch_issue(&paths, "GL-FOO-BAR--7");
        let config = default_config();

        let resolved = resolve_id(&paths, &config, &cwd, "61").expect("resolve numeric id");

        assert_eq!(resolved, "GH-GUB-DEV--61");
    }

    #[test]
    fn resolve_id_numeric_ambiguous_match_errors() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("outside")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("outside dir");
        let paths = app_paths(temp.path());
        touch_issue(&paths, "GH-GUB-DEV--61");
        touch_issue(&paths, "GL-FOO-BAR--61");
        let config = default_config();

        let error = resolve_id(&paths, &config, &cwd, "61").expect_err("ambiguous id");

        assert_eq!(
            error.to_string(),
            "ambiguous numeric ID 61: matches GH-GUB-DEV--61, GL-FOO-BAR--61"
        );
    }

    #[test]
    fn resolve_id_numeric_without_matches_errors() {
        let temp = tempdir().expect("temp dir");
        let cwd = Utf8PathBuf::from_path_buf(temp.path().join("outside")).expect("utf8 cwd");
        std::fs::create_dir_all(cwd.as_std_path()).expect("outside dir");
        let paths = app_paths(temp.path());
        let config = default_config();

        let error = resolve_id(&paths, &config, &cwd, "61").expect_err("missing id");

        assert_eq!(error.to_string(), "issue file not found: 61");
    }

    fn app_paths(root: &std::path::Path) -> AppPaths {
        let repo = root.join("repo");
        let cache = root.join("cache");
        std::fs::create_dir_all(repo.join("issues")).expect("issues dir");
        std::fs::create_dir_all(repo.join("templates")).expect("templates dir");
        AppPaths {
            riptsk_repo: repo.to_string_lossy().as_ref().into(),
            cache_root: cache.to_string_lossy().as_ref().into(),
        }
    }

    fn touch_issue(paths: &AppPaths, id: &str) {
        std::fs::write(paths.issues_dir().join(format!("{id}.md")), "").expect("write issue");
    }

    fn config_with_backends(backends: Vec<BackendConfig>) -> Config {
        let mut config = default_config();
        config.backends = backends;
        config
    }
}
