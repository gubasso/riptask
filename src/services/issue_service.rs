use crate::cli::{LsArgs, NewArgs};
use crate::config::{Config, parse_priority, parse_state};
use crate::domain::issue::{IssueDocument, IssueFrontmatter, IssueState, Priority};
use crate::error::RiptskError;
use crate::models::BackendKind;
use crate::paths::AppPaths;
use crate::services::issue_ids;
use crate::services::templates::TemplateService;
use crate::services::view_builder::ViewBuilder;
use crate::storage::{frontmatter, issue_store};
use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShiftDirection {
    Up,
    Down,
}

pub struct IssueService<'a> {
    paths: &'a AppPaths,
    config: &'a Config,
}

#[derive(Debug, Clone)]
pub struct IssueDraft {
    pub title: String,
    pub project: String,
    pub board: String,
    pub status: IssueState,
    pub priority: Priority,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub org: Option<String>,
    pub body: String,
    pub order: u32,
    pub tasks_backend_kind: Option<BackendKind>,
    pub tasks_repo: Option<String>,
}

pub struct ListMatchingResult {
    pub documents: Vec<IssueDocument>,
    pub conflict_count: usize,
}

impl<'a> IssueService<'a> {
    pub fn new(paths: &'a AppPaths, config: &'a Config) -> Self {
        Self { paths, config }
    }

    pub fn prepare_issue_draft(&self, args: NewArgs) -> Result<IssueDraft, RiptskError> {
        self.paths.ensure_repo_dirs().map_err(RiptskError::Other)?;
        let title = args
            .title
            .ok_or_else(|| RiptskError::General("missing title".into()))?;
        let project = if let Some(p) = args.project {
            p
        } else {
            let cwd = camino::Utf8PathBuf::from(
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            );
            crate::services::project_detection::detect_from_cwd(&cwd, self.config)?
                .map(|r| r.name.clone())
                .unwrap_or_else(|| "personal".into())
        };
        let template_name = args
            .template
            .or_else(|| self.config.defaults.template.clone())
            .unwrap_or_else(|| "task".into());
        let template_service = TemplateService::new(self.paths, self.config);
        let template = template_service
            .load(&template_name)
            .map_err(RiptskError::Other)?;
        let board = args
            .board
            .or_else(|| self.project_default_board(&project))
            .unwrap_or_else(|| self.config.defaults.board.clone());
        let status = args
            .status
            .map(|value| parse_state(&value))
            .transpose()
            .map_err(RiptskError::Other)?
            .or(template.default_status)
            .unwrap_or_else(|| self.config.defaults.status.clone());
        let priority = args
            .priority
            .map(|value| parse_priority(&value))
            .transpose()
            .map_err(RiptskError::Other)?
            .or(template.default_priority)
            .unwrap_or_else(|| self.config.defaults.priority.clone());
        self.validate_state_for_board(&board, &status)
            .map_err(RiptskError::Other)?;
        Ok(IssueDraft {
            title,
            project: project.clone(),
            board: board.clone(),
            status: status.clone(),
            priority,
            labels: template.default_labels,
            assignee: self.config.defaults.assignee.clone(),
            org: self.project_default_org(&project),
            body: template.body,
            order: self
                .next_order_for_lane(&board, status.as_str())
                .map_err(RiptskError::Other)?,
            tasks_backend_kind: self.project_tasks_backend_kind(&project),
            tasks_repo: self.project_tasks_repo(&project),
        })
    }

    pub fn persist_issue(&self, document: &IssueDocument) -> Result<(), RiptskError> {
        let path = self
            .paths
            .issues_dir()
            .join(format!("{}.md", document.frontmatter.id));
        frontmatter::save_issue(path.as_std_path(), document).map_err(RiptskError::Other)?;
        ViewBuilder::new(self.paths, self.config)
            .regenerate_all(None)
            .map_err(RiptskError::Other)
    }

    pub fn build_local_issue_document(
        &self,
        draft: &IssueDraft,
    ) -> Result<IssueDocument, RiptskError> {
        let scope = self.project_effective_key(&draft.project);
        let sequence =
            issue_ids::next_local_sequence(self.paths, &scope).map_err(RiptskError::Other)?;
        let id = issue_ids::format_id(&scope, sequence);
        Ok(IssueDocument {
            frontmatter: IssueFrontmatter {
                id: id.clone(),
                title: draft.title.clone(),
                status: draft.status.clone(),
                board: draft.board.clone(),
                project: draft.project.clone(),
                org: draft.org.clone(),
                priority: Some(draft.priority.clone()),
                labels: draft.labels.clone(),
                assignees: draft.assignee.clone().into_iter().collect(),
                milestone: None,
                state_reason: None,
                cycle: None,
                order: Some(draft.order),
                gitlab: None,
                github: None,
                jira: None,
                local_updated_at: now_utc(),
                due: None,
                weight: None,
                confidential: None,
                discussion_locked: None,
                issue_type: None,
                locked: None,
                lock_reason: None,
                recurring: None,
                remote_deleted: false,
                id_slug: Some(generate_slug(&id, &draft.title)),
                branch: None,
                pr_url: None,
                pr_number: None,
            },
            body: draft.body.clone(),
            remote_section: None,
        })
    }

    pub fn edit_issue(&self, id: Option<String>) -> Result<(), RiptskError> {
        let id = id.ok_or_else(|| RiptskError::General("<ID> required".into()))?;
        let path = issue_store::find_issue(self.paths, &id)?;
        let before = fs::metadata(&path)?.modified()?;
        let had_conflict = matches!(
            frontmatter::try_load_issue(path.as_std_path()),
            frontmatter::IssueLoadResult::Conflict { .. }
        );
        let status = super::editor::open_in_editor(path.as_std_path())?;
        if !status.success() {
            return Ok(());
        }
        let after = fs::metadata(&path)?.modified()?;
        if after > before {
            let content = fs::read_to_string(&path)?;
            if had_conflict || frontmatter::has_conflict_markers(&content) {
                return Ok(());
            }
            let mut issue =
                frontmatter::load_issue(path.as_std_path()).map_err(RiptskError::Other)?;
            issue.frontmatter.local_updated_at = now_utc();
            frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
            ViewBuilder::new(self.paths, self.config)
                .regenerate_all(None)
                .map_err(RiptskError::Other)?;
        }
        Ok(())
    }

    pub fn move_issue(&self, id: &str, state: &str) -> Result<(), RiptskError> {
        let path = issue_store::find_issue(self.paths, id)?;
        let mut issue = match frontmatter::try_load_issue(path.as_std_path()) {
            frontmatter::IssueLoadResult::Ok(issue) => issue,
            frontmatter::IssueLoadResult::Conflict { .. } => {
                return Err(RiptskError::Conflict(format!(
                    "issue {id} has unresolved sync conflicts"
                )));
            }
            frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
        };
        let state = parse_state(state).map_err(RiptskError::Other)?;
        self.validate_state_for_board(&issue.frontmatter.board, &state)
            .map_err(RiptskError::Other)?;
        issue.frontmatter.status = state.clone();
        issue.frontmatter.order = Some(
            self.next_order_for_lane(&issue.frontmatter.board, state.as_str())
                .map_err(RiptskError::Other)?,
        );
        issue.frontmatter.local_updated_at = now_utc();
        frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
        ViewBuilder::new(self.paths, self.config)
            .regenerate_all(None)
            .map_err(RiptskError::Other)
    }

    pub fn remove_issue(&self, id: &str) -> Result<(), RiptskError> {
        issue_store::delete_issue_files(self.paths, id)?;
        ViewBuilder::new(self.paths, self.config)
            .regenerate_all(None)
            .map_err(RiptskError::Other)
    }

    pub fn list_matching(
        &self,
        args: &LsArgs,
        scope: &crate::scope::ProjectScope,
    ) -> Result<ListMatchingResult, RiptskError> {
        let mut documents = Vec::new();
        let mut conflict_count = 0usize;
        for path in issue_store::list_issues(self.paths)? {
            let document = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(document) => document,
                frontmatter::IssueLoadResult::Conflict { .. } => {
                    conflict_count += 1;
                    continue;
                }
                frontmatter::IssueLoadResult::Err(error) => {
                    return Err(RiptskError::Other(error));
                }
            };
            if !scope.matches(&document.frontmatter.project) {
                continue;
            }
            if !matches_issue(&document, args) {
                continue;
            }
            documents.push(*document);
        }
        Ok(ListMatchingResult {
            documents,
            conflict_count,
        })
    }

    pub fn next_local_sequence(&self, project: &str) -> Result<u64> {
        let scope = self.project_effective_key(project);
        issue_ids::next_local_sequence(self.paths, &scope)
    }

    pub fn next_order_for_lane(&self, board: &str, state: &str) -> Result<u32> {
        let mut max = 0u32;
        for path in issue_store::list_issues(self.paths)? {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { .. } => continue,
                frontmatter::IssueLoadResult::Err(error) => return Err(error),
            };
            if issue.frontmatter.board == board && issue.frontmatter.status.as_str() == state {
                max = max.max(issue.frontmatter.order.unwrap_or(0));
            }
        }
        Ok(max + 1)
    }

    pub fn shift_issue_order(
        &self,
        id: &str,
        direction: ShiftDirection,
    ) -> Result<(), RiptskError> {
        let target_path = issue_store::find_issue(self.paths, id)?;
        let target_issue = match frontmatter::try_load_issue(target_path.as_std_path()) {
            frontmatter::IssueLoadResult::Ok(issue) => *issue,
            frontmatter::IssueLoadResult::Conflict { .. } => {
                return Err(RiptskError::Conflict(format!(
                    "issue {id} has unresolved sync conflicts"
                )));
            }
            frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
        };
        let mut lane = self.load_lane(
            &target_issue.frontmatter.board,
            &target_issue.frontmatter.status,
        )?;
        let Some(index) = lane.iter().position(|(issue_id, _)| issue_id == id) else {
            return Err(RiptskError::NotFound(id.to_owned()));
        };
        let swap_index = match direction {
            ShiftDirection::Up if index > 0 => index - 1,
            ShiftDirection::Down if index + 1 < lane.len() => index + 1,
            _ => return Ok(()),
        };
        lane.swap(index, swap_index);
        self.persist_lane(&lane)?;
        Ok(())
    }

    pub fn reorder_lane(
        &self,
        board: &str,
        state: &IssueState,
        ordered_ids: &[String],
    ) -> Result<(), RiptskError> {
        let lane = self.load_lane(board, state)?;
        if lane.is_empty() || ordered_ids.is_empty() {
            return Ok(());
        }

        let known_ids = lane
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<std::collections::HashSet<_>>();
        for id in ordered_ids {
            if !known_ids.contains(id.as_str()) {
                return Err(RiptskError::NotFound(id.clone()));
            }
        }

        let mut reordered = Vec::with_capacity(lane.len());
        let mut remaining = lane;
        for id in ordered_ids {
            if let Some(index) = remaining.iter().position(|(candidate, _)| candidate == id) {
                reordered.push(remaining.remove(index));
            }
        }
        reordered.extend(remaining);
        self.persist_lane(&reordered)?;
        Ok(())
    }

    fn validate_state_for_board(&self, board: &str, state: &IssueState) -> Result<()> {
        let board = self
            .config
            .boards
            .iter()
            .find(|candidate| candidate.name == board)
            .context("board not found")?;
        if board.statuses.contains(state) {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "invalid status {} for board {}",
                state.as_str(),
                board.name
            ))
        }
    }

    fn project_tasks_backend_kind(&self, project: &str) -> Option<BackendKind> {
        self.config
            .projects
            .iter()
            .find(|repo_project| repo_project.name == project)
            .map(|repo_project| repo_project.tasks_backend.kind.clone())
    }

    fn project_tasks_repo(&self, project: &str) -> Option<String> {
        self.config
            .projects
            .iter()
            .find(|repo_project| repo_project.name == project)
            .and_then(|repo_project| {
                repo_project
                    .tasks_backend
                    .repo
                    .clone()
                    .or_else(|| repo_project.tasks_backend.jira_project.clone())
            })
    }

    fn project_default_board(&self, project: &str) -> Option<String> {
        self.config
            .projects
            .iter()
            .find(|repo_project| repo_project.name == project)
            .and_then(|repo_project| repo_project.default_board.clone())
    }

    fn project_default_org(&self, project: &str) -> Option<String> {
        self.config
            .projects
            .iter()
            .find(|repo_project| repo_project.name == project)
            .and_then(|repo_project| repo_project.default_org.clone())
    }

    fn project_effective_key(&self, project: &str) -> String {
        self.config
            .projects
            .iter()
            .find(|repo_project| repo_project.name == project)
            .map(issue_ids::effective_key)
            .unwrap_or_else(|| issue_ids::sanitize_key_candidate(project))
    }

    fn load_lane(
        &self,
        board: &str,
        state: &IssueState,
    ) -> Result<Vec<(String, camino::Utf8PathBuf)>, RiptskError> {
        let mut lane = Vec::new();
        for path in issue_store::list_issues(self.paths)? {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { .. } => continue,
                frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
            };
            if issue.frontmatter.board == board && issue.frontmatter.status == *state {
                lane.push((issue.frontmatter.id, path));
            }
        }
        lane.sort_by(|(_, left_path), (_, right_path)| {
            let left_issue = match frontmatter::try_load_issue(left_path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue.frontmatter.order.unwrap_or(0),
                _ => 0,
            };
            let right_issue = match frontmatter::try_load_issue(right_path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue.frontmatter.order.unwrap_or(0),
                _ => 0,
            };
            left_issue
                .cmp(&right_issue)
                .then_with(|| left_path.cmp(right_path))
        });
        Ok(lane)
    }

    fn persist_lane(&self, lane: &[(String, camino::Utf8PathBuf)]) -> Result<(), RiptskError> {
        for (index, (_, path)) in lane.iter().enumerate() {
            let mut issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { id, .. } => {
                    return Err(RiptskError::Conflict(format!(
                        "issue {id} has unresolved sync conflicts"
                    )));
                }
                frontmatter::IssueLoadResult::Err(error) => return Err(RiptskError::Other(error)),
            };
            issue.frontmatter.order = Some((index + 1) as u32);
            issue.frontmatter.local_updated_at = now_utc();
            frontmatter::save_issue(path.as_std_path(), &issue).map_err(RiptskError::Other)?;
        }
        ViewBuilder::new(self.paths, self.config)
            .regenerate_all(None)
            .map_err(RiptskError::Other)?;
        Ok(())
    }
}

pub fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn generate_slug(id: &str, title: &str) -> String {
    let slug = title
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let slug = slug
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let suffix = if slug.is_empty() {
        String::new()
    } else {
        format!("-{slug}")
    };
    format!("{id}{suffix}")
}

pub fn generate_branch_slug(issue_number: u64, title: &str) -> String {
    let slug = title
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        format!("{issue_number}")
    } else {
        format!("{issue_number}-{slug}")
    }
}

fn matches_issue(issue: &IssueDocument, args: &LsArgs) -> bool {
    if !args.include_done && issue.frontmatter.status == IssueState::Done {
        return false;
    }
    if args.conflicts {
        return false;
    }
    if let Some(status) = &args.status
        && issue.frontmatter.status.as_str() != status
    {
        return false;
    }
    if let Some(priority) = &args.priority
        && issue
            .frontmatter
            .priority
            .as_ref()
            .map(|value| value.as_str())
            != Some(priority.as_str())
    {
        return false;
    }
    if let Some(board) = &args.board
        && &issue.frontmatter.board != board
    {
        return false;
    }
    if let Some(cycle) = &args.cycle
        && issue.frontmatter.cycle.as_deref() != Some(cycle.as_str())
    {
        return false;
    }
    if let Some(assignee) = &args.assignee
        && !issue
            .frontmatter
            .assignees
            .iter()
            .any(|candidate| candidate == assignee)
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{IssueService, generate_branch_slug, generate_slug};
    use crate::config::load_config;
    use crate::paths::AppPaths;
    use tempfile::tempdir;

    #[test]
    fn generates_slug_from_title() {
        assert_eq!(
            generate_slug("WHL-042", "Fix wormhole stabilizer"),
            "WHL-042-fix-wormhole-stabilizer"
        );
    }

    #[test]
    fn branch_slug_basic() {
        assert_eq!(
            generate_branch_slug(42, "Fix wormhole stabilizer"),
            "42-fix-wormhole-stabilizer"
        );
    }

    #[test]
    fn branch_slug_empty_title() {
        assert_eq!(generate_branch_slug(7, ""), "7");
    }

    #[test]
    fn branch_slug_special_chars() {
        assert_eq!(generate_branch_slug(99, "foo/bar: baz!"), "99-foo-bar-baz");
    }

    #[test]
    fn prepares_local_sequence_for_remote_projects() {
        let temp = tempdir().expect("temp dir");
        let repo = temp.path().join("repo");
        let cache = temp.path().join("cache");
        std::fs::create_dir_all(&repo).expect("repo dir");
        std::fs::create_dir_all(repo.join("issues")).expect("issues dir");
        std::fs::create_dir_all(repo.join("templates")).expect("templates dir");
        std::fs::write(
            repo.join("riptsk.yaml"),
            include_str!("../../tests/fixtures/riptsk.yaml"),
        )
        .expect("config");
        let paths = AppPaths {
            riptsk_repo: repo.to_string_lossy().as_ref().into(),
            cache_root: cache.to_string_lossy().as_ref().into(),
            state_root: temp.path().join("state").to_string_lossy().as_ref().into(),
        };
        let config = load_config(paths.config_path().as_std_path()).expect("config");
        let service = IssueService::new(&paths, &config);
        assert_eq!(
            service
                .next_local_sequence("wormhole-router")
                .expect("sequence"),
            1
        );
    }
}
