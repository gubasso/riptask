//! Derivation, sanitization, and validation for `RepoProject.repo_project_label`.
//!
//! Labels carry a fixed `proj::` prefix so a Jira label ring clearly identifies
//! which tags reference a RepoProject (vs. a status, component, or freeform
//! tag). Derived labels always have the prefix; user input is normalized to
//! the prefixed form.

use crate::error::RiptaskError;
use crate::models::{BackendKind, RepoProject};

pub const MAX_LABEL_LEN: usize = 255;

/// Fixed prefix for every `repo_project_label`. Identifies a Jira label as a
/// RepoProject reference.
pub const LABEL_PREFIX: &str = "proj::";

/// Derive a default label from the git origin URL's last path segment (minus
/// `.git`), falling back to `cwd_basename` when no remote is available. The
/// return value carries the `proj::` prefix. Returns `None` if the sanitized
/// suffix is empty (e.g. emoji-only or punctuation-only source).
pub fn derive_default(origin_last_segment: Option<&str>, cwd_basename: &str) -> Option<String> {
    let raw = origin_last_segment.unwrap_or(cwd_basename);
    let suffix = sanitize(raw);
    if suffix.is_empty() {
        None
    } else {
        Some(format!("{LABEL_PREFIX}{suffix}"))
    }
}

/// Normalize a user-supplied label value to the prefixed form. Accepts input
/// that already starts with `proj::` (kept verbatim after validation) or a
/// bare suffix (sanitized and prefixed). Returns `None` when normalization
/// yields an empty suffix.
pub fn normalize_user_input(raw: &str) -> Option<String> {
    if let Some(suffix) = raw.strip_prefix(LABEL_PREFIX) {
        if suffix.is_empty() {
            None
        } else {
            Some(format!("{LABEL_PREFIX}{suffix}"))
        }
    } else {
        let suffix = sanitize(raw);
        if suffix.is_empty() {
            None
        } else {
            Some(format!("{LABEL_PREFIX}{suffix}"))
        }
    }
}

/// Lowercase, replace separators with `-`, strip disallowed chars, collapse
/// repeated `-`, and trim trailing `-`.
pub fn sanitize(raw: &str) -> String {
    let lowered = raw.trim_end_matches(".git").to_lowercase();
    let mut buffer = String::with_capacity(lowered.len());
    let mut last_was_dash = false;
    for ch in lowered.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '_' {
            Some(ch)
        } else if matches!(ch, ' ' | '\t' | '.' | '/' | '\\' | '-') {
            Some('-')
        } else {
            None
        };

        match mapped {
            Some('-') if !last_was_dash && !buffer.is_empty() => {
                buffer.push('-');
                last_was_dash = true;
            }
            Some('-') => {}
            Some(c) => {
                buffer.push(c);
                last_was_dash = false;
            }
            None => {}
        }
    }

    while buffer.ends_with('-') {
        buffer.pop();
    }

    buffer
}

/// Validate a user-or-derived label. Enforces the `proj::` prefix, a
/// non-empty suffix, the length bound, and the whitespace/quote ban.
pub fn validate(label: &str) -> Result<(), RiptaskError> {
    if label.is_empty() {
        return Err(RiptaskError::Config(
            "repo_project_label cannot be empty".into(),
        ));
    }
    if label.len() > MAX_LABEL_LEN {
        return Err(RiptaskError::Config(format!(
            "repo_project_label '{label}' exceeds {MAX_LABEL_LEN} chars"
        )));
    }
    if label.chars().any(|c| c.is_whitespace() || c == '"') {
        return Err(RiptaskError::Config(format!(
            "repo_project_label '{label}' must not contain whitespace or double quotes"
        )));
    }
    let Some(suffix) = label.strip_prefix(LABEL_PREFIX) else {
        return Err(RiptaskError::Config(format!(
            "repo_project_label '{label}' must start with '{LABEL_PREFIX}'"
        )));
    };
    if suffix.is_empty() {
        return Err(RiptaskError::Config(format!(
            "repo_project_label '{label}' is missing a suffix after '{LABEL_PREFIX}'"
        )));
    }
    Ok(())
}

/// Deduplicate a derived label against other RepoProjects sharing the same Jira project.
///
/// The bare `derived` label is tried first; on collision we prefix with the
/// current directory's parent name and re-check. If the prefixed candidate is
/// also taken we fall through to numeric suffixes (`-2`, `-3`, ...) scoped to
/// the same Jira project, so a third sibling registration cannot silently
/// reuse a sibling's label.
pub fn deduplicate_within_jira_project<'a>(
    derived: &str,
    new: &RepoProject,
    existing: impl IntoIterator<Item = &'a RepoProject>,
    cwd_parent_name: Option<&str>,
) -> String {
    let same_jira = |rp: &RepoProject| -> bool {
        rp.tasks_backend.kind == BackendKind::Jira
            && new.tasks_backend.kind == BackendKind::Jira
            && rp.tasks_backend.host == new.tasks_backend.host
            && rp.tasks_backend.jira_project == new.tasks_backend.jira_project
    };
    let siblings: Vec<&'a RepoProject> = existing.into_iter().filter(|rp| same_jira(rp)).collect();
    let taken = |candidate: &str| -> bool {
        siblings
            .iter()
            .any(|rp| rp.repo_project_label.as_deref() == Some(candidate))
    };

    if !taken(derived) {
        return derived.to_owned();
    }
    let derived_suffix = derived.strip_prefix(LABEL_PREFIX).unwrap_or(derived);
    let prefixed = |suffix: &str| format!("{LABEL_PREFIX}{suffix}");
    if let Some(parent) = cwd_parent_name {
        let suffix = sanitize(&format!("{parent}-{derived_suffix}"));
        if !suffix.is_empty() {
            let candidate = prefixed(&suffix);
            if !taken(&candidate) {
                return candidate;
            }
        }
    }
    // Final fallback: numeric suffix. Guaranteed to terminate because `siblings`
    // is finite, so some `{derived}-N` for N in 2..=len+2 is free.
    let mut n: usize = 2;
    loop {
        let suffix = sanitize(&format!("{derived_suffix}-{n}"));
        let candidate = prefixed(&suffix);
        if !suffix.is_empty() && !taken(&candidate) {
            return candidate;
        }
        n += 1;
        if n > siblings.len() + 2 {
            // Defensive: should be unreachable under the invariant above.
            return candidate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LABEL_PREFIX, MAX_LABEL_LEN, deduplicate_within_jira_project, derive_default,
        normalize_user_input, sanitize, validate,
    };
    use crate::models::{BackendKind, RepoProject, TasksBackendSpec, VCBackendSpec};

    fn repo_project(
        name: &str,
        label: Option<&str>,
        host: Option<&str>,
        jira_project: Option<&str>,
    ) -> RepoProject {
        RepoProject {
            name: name.into(),
            vc_backend: VCBackendSpec {
                kind: BackendKind::Local,
                host: None,
                repo: None,
                path: Some(format!("/tmp/{name}")),
            },
            tasks_backend: TasksBackendSpec {
                kind: if jira_project.is_some() {
                    BackendKind::Jira
                } else {
                    BackendKind::Local
                },
                host: host.map(str::to_owned),
                repo: None,
                jira_project: jira_project.map(str::to_owned),
                default_issue_type: None,
                path: None,
            },
            default_board: None,
            default_org: None,
            key: None,
            repo_project_label: label.map(str::to_owned),
        }
    }

    #[test]
    fn sanitize_normalizes_common_cases() {
        assert_eq!(sanitize("My Repo.git"), "my-repo");
        assert_eq!(sanitize("owner/repo_name"), "owner-repo_name");
        assert_eq!(sanitize("already---split"), "already-split");
        assert_eq!(sanitize("UPPER.CASE"), "upper-case");
        assert_eq!(sanitize(" weird / value "), "weird-value");
    }

    #[test]
    fn derive_default_prefers_origin_segment_and_prepends_prefix() {
        assert_eq!(
            derive_default(Some("my-api.git"), "fallback").as_deref(),
            Some("proj::my-api")
        );
        assert_eq!(
            derive_default(None, "my-api").as_deref(),
            Some("proj::my-api")
        );
    }

    #[test]
    fn derive_default_returns_none_when_suffix_sanitizes_to_empty() {
        assert_eq!(derive_default(Some("!!!"), "???"), None);
    }

    #[test]
    fn normalize_accepts_bare_suffix_and_prefixed_forms() {
        assert_eq!(
            normalize_user_input("my-api").as_deref(),
            Some("proj::my-api")
        );
        assert_eq!(
            normalize_user_input("proj::already").as_deref(),
            Some("proj::already")
        );
        assert_eq!(normalize_user_input("proj::").as_deref(), None);
        assert_eq!(normalize_user_input("!!!"), None);
    }

    #[test]
    fn validate_rejects_bad_inputs() {
        assert!(validate("").is_err());
        assert!(validate("has space").is_err());
        assert!(validate("has\"quote").is_err());
        assert!(validate(&format!("{LABEL_PREFIX}{}", "a".repeat(MAX_LABEL_LEN))).is_err());
        // No prefix → reject.
        assert!(validate("good-label").is_err());
        // Prefix only → reject.
        assert!(validate("proj::").is_err());
        // Prefixed, non-empty suffix, no forbidden chars → accept.
        assert!(validate("proj::good-label").is_ok());
    }

    #[test]
    fn deduplicate_scopes_to_matching_jira_project() {
        let new = repo_project("new", None, Some("https://jira.example"), Some("org/PROJ"));
        let existing = [repo_project(
            "existing",
            Some("proj::repo-a"),
            Some("https://jira.example"),
            Some("org/PROJ"),
        )];
        assert_eq!(
            deduplicate_within_jira_project("proj::repo-a", &new, existing.iter(), Some("team")),
            "proj::team-repo-a"
        );
    }

    #[test]
    fn deduplicate_falls_back_when_prefixed_candidate_also_taken() {
        let new = repo_project("new", None, Some("https://jira.example"), Some("org/PROJ"));
        let existing = [
            repo_project(
                "first",
                Some("proj::repo-a"),
                Some("https://jira.example"),
                Some("org/PROJ"),
            ),
            repo_project(
                "second",
                Some("proj::team-repo-a"),
                Some("https://jira.example"),
                Some("org/PROJ"),
            ),
        ];
        let resolved =
            deduplicate_within_jira_project("proj::repo-a", &new, existing.iter(), Some("team"));
        assert_ne!(resolved, "proj::repo-a");
        assert_ne!(resolved, "proj::team-repo-a");
        // Numeric fallback kicks in; accept any `proj::repo-a-N` form.
        assert!(
            resolved.starts_with("proj::repo-a-"),
            "expected numeric fallback, got {resolved}"
        );
    }
}
