use crate::config::Config;
use crate::paths::AppPaths;
use crate::scope::ProjectScope;
use crate::storage::{frontmatter, issue_store};
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use std::fs;

pub struct ViewBuilder<'a> {
    paths: &'a AppPaths,
    config: &'a Config,
}

impl<'a> ViewBuilder<'a> {
    pub fn new(paths: &'a AppPaths, config: &'a Config) -> Self {
        Self { paths, config }
    }

    pub fn regenerate_all(&self, scope: Option<&ProjectScope>) -> Result<()> {
        let root = self.paths.views_root();
        if root.exists() {
            fs::remove_dir_all(&root)
                .with_context(|| format!("failed to clear {}", root.as_str()))?;
        }
        fs::create_dir_all(&root).context("failed to create view root")?;
        self.build_kanban(scope)?;
        self.build_projects()?;
        self.build_orgs()?;
        self.build_cycles()?;
        Ok(())
    }

    pub fn board_path(&self, board: Option<&str>, all: bool) -> Utf8PathBuf {
        if all {
            return self.paths.views_root().join("kanban");
        }
        let board = board
            .map(ToOwned::to_owned)
            .or_else(|| self.config.boards.first().map(|board| board.name.clone()))
            .unwrap_or_else(|| "personal".into());
        self.paths.views_root().join("kanban").join(board)
    }

    fn build_kanban(&self, scope: Option<&ProjectScope>) -> Result<()> {
        for board in &self.config.boards {
            for state in &board.statuses {
                fs::create_dir_all(
                    self.paths
                        .views_root()
                        .join("kanban")
                        .join(&board.name)
                        .join(state.as_str()),
                )?;
            }
        }
        for path in issue_store::list_issues(self.paths)? {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { .. } => continue,
                frontmatter::IssueLoadResult::Err(error) => return Err(error),
            };
            if issue.frontmatter.remote_deleted {
                continue;
            }
            if scope.is_some_and(|project_scope| !project_scope.matches(&issue.frontmatter.project))
            {
                continue;
            }
            let order = issue.frontmatter.order.unwrap_or(1);
            let name = format!("{order:02}-{}.md", issue.frontmatter.id);
            let target = self
                .paths
                .views_root()
                .join("kanban")
                .join(&issue.frontmatter.board)
                .join(issue.frontmatter.status.as_str())
                .join(name);
            fs::create_dir_all(target.parent().context("missing target parent")?)?;
            fs::copy(&path, &target)?;
        }
        Ok(())
    }

    fn build_projects(&self) -> Result<()> {
        for path in issue_store::list_issues(self.paths)? {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { .. } => continue,
                frontmatter::IssueLoadResult::Err(error) => return Err(error),
            };
            let target = self
                .paths
                .views_root()
                .join("projects")
                .join(&issue.frontmatter.project)
                .join(format!("{}.md", issue.frontmatter.id));
            fs::create_dir_all(target.parent().context("missing project view parent")?)?;
            fs::copy(&path, &target)?;
        }
        Ok(())
    }

    fn build_orgs(&self) -> Result<()> {
        for path in issue_store::list_issues(self.paths)? {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { .. } => continue,
                frontmatter::IssueLoadResult::Err(error) => return Err(error),
            };
            let Some(org) = issue.frontmatter.org.as_ref() else {
                continue;
            };
            let target = self
                .paths
                .views_root()
                .join("orgs")
                .join(org)
                .join(format!("{}.md", issue.frontmatter.id));
            fs::create_dir_all(target.parent().context("missing org view parent")?)?;
            fs::copy(&path, &target)?;
        }
        Ok(())
    }

    fn build_cycles(&self) -> Result<()> {
        for path in issue_store::list_issues(self.paths)? {
            let issue = match frontmatter::try_load_issue(path.as_std_path()) {
                frontmatter::IssueLoadResult::Ok(issue) => issue,
                frontmatter::IssueLoadResult::Conflict { .. } => continue,
                frontmatter::IssueLoadResult::Err(error) => return Err(error),
            };
            let Some(cycle) = issue.frontmatter.cycle.as_ref() else {
                continue;
            };
            let order = issue.frontmatter.order.unwrap_or(1);
            let target = self
                .paths
                .views_root()
                .join("cycles")
                .join(cycle)
                .join(format!("{order:02}-{}.md", issue.frontmatter.id));
            fs::create_dir_all(target.parent().context("missing cycle view parent")?)?;
            fs::copy(&path, &target)?;
        }
        Ok(())
    }
}
