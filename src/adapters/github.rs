use crate::adapters::backend::{
    BackendIssueRecord, BackendIssueUpsert, BackendPrRecord, DeleteOutcome, IssueTracker,
    MergeMethod, PrCheckItem, PrChecksReport, PrChecksStatus, VersionControl,
};
use crate::error::RiptaskError;
use async_trait::async_trait;
use base64::Engine;
use chrono::SecondsFormat;
use octocrab::models;
use octocrab::models::workflows::WorkFlow;
use octocrab::params::LockReason;
use octocrab::params::repos::Commitish;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use tokio::try_join;

/// Format an octocrab error with full detail.
///
/// `octocrab::Error`'s `Display` for the `GitHub` variant just prints
/// "GitHub" (snafu default), losing the status code and message. This
/// helper extracts the inner `GitHubError` which has a proper `Display`.
fn format_octocrab_error(error: &octocrab::Error) -> String {
    match error {
        octocrab::Error::GitHub { source, .. } => {
            format!("{source} (HTTP {})", source.status_code.as_u16())
        }
        other => other.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct GithubProvider {
    pub client: octocrab::Octocrab,
}

impl GithubProvider {
    pub fn new(token: &str) -> Result<Self, RiptaskError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let client = octocrab::Octocrab::builder()
            .personal_token(token.to_owned())
            .build()
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(Self { client })
    }

    fn split_owner_repo<'a>(&self, repo: &'a str) -> Result<(&'a str, &'a str), RiptaskError> {
        repo.split_once('/')
            .ok_or_else(|| RiptaskError::Config(format!("invalid github repo: {repo}")))
    }

    async fn fetch_optional_json(
        &self,
        route: String,
        forbidden_warning: &str,
    ) -> Result<(Option<Value>, Option<String>), RiptaskError> {
        match self.client.get::<Value, _, _>(route, None::<&()>).await {
            Ok(value) => Ok((Some(value), None)),
            Err(octocrab::Error::GitHub { source, .. })
                if matches!(source.status_code.as_u16(), 403 | 404) =>
            {
                let warning =
                    (source.status_code.as_u16() == 403).then(|| forbidden_warning.to_owned());
                Ok((None, warning))
            }
            Err(error) => Err(RiptaskError::Unreachable(format_octocrab_error(&error))),
        }
    }

    async fn list_all_check_runs(
        &self,
        owner: &str,
        repo_name: &str,
        head_sha: &str,
    ) -> Result<Vec<models::checks::CheckRun>, RiptaskError> {
        let mut page = 1u32;
        let mut runs = Vec::new();
        loop {
            let response = self
                .client
                .checks(owner.to_string(), repo_name.to_string())
                .list_check_runs_for_git_ref(Commitish(head_sha.to_owned()))
                .per_page(100u8)
                .page(page)
                .send()
                .await
                .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
            let count = response.check_runs.len();
            runs.extend(response.check_runs);
            if count < 100 {
                break;
            }
            page += 1;
        }
        Ok(runs)
    }

    async fn list_all_check_suites(
        &self,
        owner: &str,
        repo_name: &str,
        head_sha: &str,
    ) -> Result<Vec<models::checks::CheckSuite>, RiptaskError> {
        let mut page = 1u32;
        let mut suites = Vec::new();
        loop {
            let response = self
                .client
                .checks(owner.to_string(), repo_name.to_string())
                .list_check_suites_for_git_ref(Commitish(head_sha.to_owned()))
                .per_page(100u8)
                .page(page)
                .send()
                .await
                .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
            let count = response.check_suites.len();
            suites.extend(response.check_suites);
            if count < 100 {
                break;
            }
            page += 1;
        }
        Ok(suites)
    }

    async fn list_all_workflow_runs(
        &self,
        owner: &str,
        repo_name: &str,
        head_sha: &str,
    ) -> Result<Vec<models::workflows::Run>, RiptaskError> {
        let first_page = self
            .client
            .workflows(owner, repo_name)
            .list_all_runs()
            .head_sha(head_sha.to_owned())
            .per_page(100u8)
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        self.client
            .all_pages(first_page)
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))
    }

    async fn list_all_workflows(
        &self,
        owner: &str,
        repo_name: &str,
    ) -> Result<Vec<WorkFlow>, RiptaskError> {
        let first_page = self
            .client
            .workflows(owner, repo_name)
            .list()
            .per_page(100u8)
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        self.client
            .all_pages(first_page)
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))
    }

    async fn load_head_workflow_specs(
        &self,
        owner: &str,
        repo_name: &str,
        head_sha: &str,
        contents: Option<Value>,
    ) -> Result<(Vec<WorkflowSpec>, Vec<String>), RiptaskError> {
        let entries = workflow_entries_from_contents(contents);
        let mut specs = Vec::new();
        let mut warnings = Vec::new();
        for entry in entries {
            let route = format!(
                "/repos/{owner}/{repo_name}/contents/{}?ref={head_sha}",
                entry.path
            );
            match self.client.get::<Value, _, _>(route, None::<&()>).await {
                Ok(value) => {
                    let yaml = decode_workflow_content(&value);
                    match yaml.and_then(parse_workflow_spec) {
                        Some(mut spec) => {
                            spec.path = entry.path.clone();
                            if spec.name.is_none() {
                                spec.name = Some(entry.path.clone());
                            }
                            specs.push(spec);
                        }
                        None => {
                            warnings.push(format!(
                                "could not parse workflow {}; including conservatively",
                                entry.path
                            ));
                            specs.push(WorkflowSpec::conservative_default(entry.path.clone()));
                        }
                    }
                }
                Err(_) => {
                    warnings.push(format!(
                        "could not read workflow {}; including conservatively",
                        entry.path
                    ));
                    specs.push(WorkflowSpec::conservative_default(entry.path.clone()));
                }
            }
        }
        Ok((specs, warnings))
    }
}

#[async_trait]
impl IssueTracker for GithubProvider {
    async fn list_issues(&self, repo: &str) -> Result<Vec<BackendIssueRecord>, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let page = self
            .client
            .issues(owner, repo_name)
            .list()
            .state(octocrab::params::State::All)
            .per_page(100)
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        let issues = self
            .client
            .all_pages::<models::issues::Issue>(page)
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(issues
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
            .map(map_issue)
            .collect())
    }

    async fn get_issue(
        &self,
        repo: &str,
        issue_id: u64,
    ) -> Result<BackendIssueRecord, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let issue = self
            .client
            .issues(owner, repo_name)
            .get(issue_id)
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(map_issue(issue))
    }

    async fn create_issue(
        &self,
        repo: &str,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let handler = self.client.issues(owner, repo_name);
        let mut builder = handler
            .create(&issue.title)
            .body(&issue.body)
            .labels(issue.labels.clone());
        if !issue.assignees.is_empty() {
            builder = builder.assignees(issue.assignees.clone());
        }
        if let Some(milestone_id) = issue.milestone_id {
            builder = builder.milestone(milestone_id);
        }
        let created = builder
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(map_issue(created))
    }

    async fn update_issue(
        &self,
        repo: &str,
        issue_id: u64,
        issue: &BackendIssueUpsert,
    ) -> Result<BackendIssueRecord, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let handler = self.client.issues(owner, repo_name);
        let mut builder = handler
            .update(issue_id)
            .title(&issue.title)
            .body(&issue.body)
            .labels(&issue.labels)
            .assignees(&issue.assignees);
        if let Some(milestone_id) = issue.milestone_id {
            builder = builder.milestone(milestone_id);
        }
        if let Some(state) = issue.state.as_deref() {
            builder = builder.state(parse_github_issue_state(state)?);
        }
        if let Some(state_reason) = issue.state_reason.as_deref() {
            builder = builder.state_reason(parse_github_state_reason(state_reason)?);
        }
        let updated = builder
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(map_issue(updated))
    }

    async fn close_issue(
        &self,
        repo: &str,
        issue_id: u64,
        state_reason: Option<&str>,
    ) -> Result<(), RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let issues = self.client.issues(owner, repo_name);
        let mut builder = issues.update(issue_id).state(models::IssueState::Closed);
        if let Some(reason) = state_reason {
            builder = builder.state_reason(parse_github_state_reason(reason)?);
        }
        builder
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(())
    }

    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .update(issue_id)
            .state(models::IssueState::Open)
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(())
    }

    async fn delete_issue(&self, repo: &str, issue_id: u64) -> Result<DeleteOutcome, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let issue = self
            .client
            .issues(owner, repo_name)
            .get(issue_id)
            .await
            .map_err(|e| RiptaskError::Unreachable(format_octocrab_error(&e)))?;
        let node_id = issue.node_id;

        let query = r#"mutation($issueId: ID!) {
            deleteIssue(input: {issueId: $issueId}) {
                clientMutationId
            }
        }"#;
        let payload = serde_json::json!({
            "query": query,
            "variables": { "issueId": node_id }
        });
        match self.client.graphql::<serde_json::Value>(&payload).await {
            Ok(response) => {
                if let Some(errors) = response.get("errors") {
                    let msg = errors.to_string();
                    let lower = msg.to_lowercase();
                    if lower.contains("forbidden")
                        || lower.contains("insufficient")
                        || lower.contains("not allowed")
                        || msg.contains("403")
                    {
                        self.close_issue(repo, issue_id, None).await?;
                        return Ok(DeleteOutcome::SoftClosed);
                    }
                    return Err(RiptaskError::Unreachable(format!(
                        "deleteIssue failed: {msg}"
                    )));
                }
                Ok(DeleteOutcome::HardDeleted)
            }
            Err(e) => {
                crate::ui::warn(&format!(
                    "GraphQL deleteIssue failed ({e}), falling back to close"
                ));
                self.close_issue(repo, issue_id, None).await?;
                Ok(DeleteOutcome::SoftClosed)
            }
        }
    }

    async fn lock_issue(
        &self,
        repo: &str,
        issue_id: u64,
        reason: Option<&str>,
    ) -> Result<(), RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let lock_reason = reason.and_then(parse_lock_reason);
        self.client
            .issues(owner, repo_name)
            .lock(issue_id, lock_reason)
            .await
            .map_err(|e| RiptaskError::Unreachable(format_octocrab_error(&e)))?;
        Ok(())
    }

    async fn unlock_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .unlock(issue_id)
            .await
            .map_err(|e| RiptaskError::Unreachable(format_octocrab_error(&e)))?;
        Ok(())
    }

    async fn sync_labels(
        &self,
        repo: &str,
        issue_id: u64,
        labels: &[String],
    ) -> Result<(), RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .issues(owner, repo_name)
            .replace_all_labels(issue_id, labels)
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(())
    }
}

#[async_trait]
impl VersionControl for GithubProvider {
    async fn create_pr(
        &self,
        repo: &str,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .create(title, head, base)
            .body(body)
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        map_pull_request(pull, repo)
    }

    async fn get_pr(&self, repo: &str, number: u64) -> Result<BackendPrRecord, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .get(number)
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        map_pull_request(pull, repo)
    }

    async fn update_pr(
        &self,
        repo: &str,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<BackendPrRecord, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .update(number)
            .title(title)
            .body(body)
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        map_pull_request(pull, repo)
    }

    async fn find_pr_by_branch(
        &self,
        repo: &str,
        head: &str,
        base: &str,
    ) -> Result<Option<BackendPrRecord>, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let page = self
            .client
            .pulls(owner, repo_name)
            .list()
            .head(format!("{owner}:{head}"))
            .base(base)
            .send()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        let Some(pull) = page.items.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(map_pull_request(pull, repo)?))
    }

    async fn merge_pr(
        &self,
        repo: &str,
        number: u64,
        method: MergeMethod,
        commit_title: Option<&str>,
        commit_message: Option<&str>,
    ) -> Result<(), RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pulls = self.client.pulls(owner, repo_name);
        let mut builder = pulls.merge(number).method(match method {
            MergeMethod::Merge => octocrab::params::pulls::MergeMethod::Merge,
            MergeMethod::Squash => octocrab::params::pulls::MergeMethod::Squash,
            MergeMethod::Rebase => octocrab::params::pulls::MergeMethod::Rebase,
        });
        if let Some(title) = commit_title {
            builder = builder.title(title);
        }
        if let Some(message) = commit_message {
            builder = builder.message(message);
        }
        builder.send().await.map_err(|error| {
            RiptaskError::General(format!("failed to merge PR #{number}: {error}"))
        })?;
        Ok(())
    }

    async fn get_pr_checks_report(
        &self,
        repo: &str,
        number: u64,
    ) -> Result<PrChecksReport, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let pull = self
            .client
            .pulls(owner, repo_name)
            .get(number)
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        let head_sha = pull.head.sha.clone();
        let base_ref = pull.base.ref_field.clone();
        let head_ref = pull.head.ref_field.clone();
        let base_full_name = format!("{owner}/{repo_name}");
        let head_repo = pull.head.repo.as_ref();
        let head_repo_full = head_repo.and_then(|repo| repo.full_name.clone());
        let is_fork = head_repo_full
            .as_deref()
            .map(|name| name != base_full_name)
            .unwrap_or_else(|| head_repo.and_then(|repo| repo.fork).unwrap_or(false));

        let (
            check_suites,
            check_runs,
            workflow_runs,
            combined_status,
            rulesets,
            classic_protection,
            workflow_contents,
            workflows_api,
        ) = try_join!(
            self.list_all_check_suites(owner, repo_name, &head_sha),
            self.list_all_check_runs(owner, repo_name, &head_sha),
            self.list_all_workflow_runs(owner, repo_name, &head_sha),
            async {
                self.client
                    .get::<octocrab::models::CombinedStatus, _, _>(
                        format!("/repos/{owner}/{repo_name}/commits/{head_sha}/status"),
                        None::<&()>,
                    )
                    .await
                    .map(Some)
                    .or_else(|error| match error {
                        octocrab::Error::GitHub { source, .. }
                            if source.status_code.as_u16() == 404 =>
                        {
                            Ok(None)
                        }
                        _ => Err(RiptaskError::Unreachable(format_octocrab_error(&error))),
                    })
            },
            self.fetch_optional_json(
                format!("/repos/{owner}/{repo_name}/rules/branches/{base_ref}"),
                "could not read GitHub rulesets for this branch; continuing without ruleset-required contexts",
            ),
            self.fetch_optional_json(
                format!(
                    "/repos/{owner}/{repo_name}/branches/{base_ref}/protection/required_status_checks"
                ),
                "could not read GitHub branch protection required checks; continuing without classic required contexts",
            ),
            self.fetch_optional_json(
                format!("/repos/{owner}/{repo_name}/contents/.github/workflows?ref={head_sha}"),
                "could not read workflow files at the PR head; continuing without workflow-derived expectations",
            ),
            async {
                self.list_all_workflows(owner, repo_name)
                    .await
                    .map(Some)
                    .or_else(|error| match error {
                        RiptaskError::Unreachable(message)
                            if message.contains("HTTP 403") || message.contains("HTTP 404") =>
                        {
                            Ok(None)
                        }
                        other => Err(other),
                    })
            }
        )?;

        let mut warnings = Vec::new();
        warnings.extend(rulesets.1);
        warnings.extend(classic_protection.1);
        warnings.extend(workflow_contents.1);
        if is_fork {
            warnings.push(
                "fork PR: workflow-derived expectations may not run for first-time contributors"
                    .into(),
            );
        }

        let disabled_workflows = workflows_api
            .as_ref()
            .map(|workflows| disabled_workflow_paths(workflows))
            .unwrap_or_default();
        let (workflow_specs, workflow_warnings) = self
            .load_head_workflow_specs(owner, repo_name, &head_sha, workflow_contents.0)
            .await?;
        warnings.extend(workflow_warnings);
        let ruleset_expected = rulesets
            .0
            .as_ref()
            .map(extract_ruleset_required_contexts)
            .unwrap_or_default();
        let classic_expected = classic_protection
            .0
            .as_ref()
            .map(extract_classic_required_contexts)
            .unwrap_or_default();
        let workflow_expected = workflow_specs
            .iter()
            .filter(|spec| !disabled_workflows.contains(&spec.path))
            .filter(|_| !is_fork)
            .filter(|spec| workflow_matches_pr(spec, &base_ref, &head_ref))
            .map(WorkflowSpec::resolved_name)
            .collect::<Vec<_>>();

        let expected =
            merge_expected_names([ruleset_expected, classic_expected, workflow_expected]);
        let combined_statuses = combined_status
            .map(|status| status.statuses)
            .unwrap_or_default();
        let registered = collect_registered_names(&check_runs, &workflow_runs, &combined_statuses);
        let suite_activity = check_suites.iter().any(|suite| {
            suite
                .status
                .as_deref()
                .is_some_and(|status| is_pending_state(status) || is_pass_state(status))
        });
        let items = collect_github_check_items(&check_runs, &workflow_runs, &combined_statuses);
        let status = rollup_report_status(&expected, &registered, &items, suite_activity);

        Ok(PrChecksReport {
            head_sha,
            expected,
            registered,
            items,
            status,
            warnings,
        })
    }

    async fn create_branch(
        &self,
        repo: &str,
        branch_name: &str,
        base_ref: &str,
        issue_id: Option<u64>,
    ) -> Result<(), RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;

        // Resolve base branch to SHA
        let base = self
            .client
            .repos(owner, repo_name)
            .get_ref(&octocrab::params::repos::Reference::Branch(
                base_ref.to_owned(),
            ))
            .await
            .map_err(|e| RiptaskError::Unreachable(format_octocrab_error(&e)))?;
        let sha = match base.object {
            octocrab::models::repos::Object::Commit { sha, .. }
            | octocrab::models::repos::Object::Tag { sha, .. } => sha,
            _ => {
                return Err(RiptaskError::Unreachable(format!(
                    "unsupported git ref object for base branch {base_ref}"
                )));
            }
        };

        if let Some(id) = issue_id {
            // Get issue node_id for GraphQL linking
            let issue = self
                .client
                .issues(owner, repo_name)
                .get(id)
                .await
                .map_err(|e| RiptaskError::Unreachable(format_octocrab_error(&e)))?;
            let issue_node_id = issue.node_id;

            // Use createLinkedBranch GraphQL mutation to create branch linked to issue
            let query = r#"mutation($issueId: ID!, $oid: GitObjectID!, $name: String) {
                createLinkedBranch(input: {issueId: $issueId, oid: $oid, name: $name}) {
                    linkedBranch { ref { name } }
                }
            }"#;
            let payload = serde_json::json!({
                "query": query,
                "variables": {
                    "issueId": issue_node_id,
                    "oid": sha,
                    "name": branch_name,
                }
            });
            let response: serde_json::Value = self
                .client
                .graphql(&payload)
                .await
                .map_err(|e| RiptaskError::Unreachable(format_octocrab_error(&e)))?;

            if let Some(errors) = response.get("errors") {
                return Err(RiptaskError::Unreachable(format!(
                    "GitHub createLinkedBranch failed: {errors}"
                )));
            }
        } else {
            // Create a plain branch without linking to an issue
            let query = r#"mutation($repoId: ID!, $name: String!, $oid: GitObjectID!) {
                createRef(input: {repositoryId: $repoId, name: $name, oid: $oid}) {
                    ref { name }
                }
            }"#;
            let repo_info = self
                .client
                .repos(owner, repo_name)
                .get()
                .await
                .map_err(|e| RiptaskError::Unreachable(format_octocrab_error(&e)))?;
            let repo_node_id = repo_info.node_id.ok_or_else(|| {
                RiptaskError::Unreachable(format!("missing node_id for repo {repo}"))
            })?;
            let payload = serde_json::json!({
                "query": query,
                "variables": {
                    "repoId": repo_node_id,
                    "name": format!("refs/heads/{branch_name}"),
                    "oid": sha,
                }
            });
            let response: serde_json::Value = self
                .client
                .graphql(&payload)
                .await
                .map_err(|e| RiptaskError::Unreachable(format_octocrab_error(&e)))?;

            if let Some(errors) = response.get("errors") {
                return Err(RiptaskError::Unreachable(format!(
                    "GitHub createRef failed: {errors}"
                )));
            }
        }

        Ok(())
    }

    async fn default_branch(&self, repo: &str) -> Result<String, RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        let repository = self
            .client
            .repos(owner, repo_name)
            .get()
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        repository
            .default_branch
            .ok_or_else(|| RiptaskError::Unreachable(format!("missing default branch for {repo}")))
    }

    async fn delete_branch(&self, repo: &str, branch_name: &str) -> Result<(), RiptaskError> {
        let (owner, repo_name) = self.split_owner_repo(repo)?;
        self.client
            .repos(owner, repo_name)
            .delete_ref(&octocrab::params::repos::Reference::Branch(
                branch_name.to_owned(),
            ))
            .await
            .map_err(|error| RiptaskError::Unreachable(format_octocrab_error(&error)))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct WorkflowSpec {
    name: Option<String>,
    path: String,
    on: OnTriggers,
}

impl WorkflowSpec {
    fn conservative_default(path: String) -> Self {
        Self {
            name: Some(path.clone()),
            path,
            on: OnTriggers::Map(WorkflowTriggerMap::pull_request_any()),
        }
    }

    fn resolved_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.path.clone())
    }
}

#[derive(Debug, Clone, Default)]
enum OnTriggers {
    #[default]
    None,
    Single(String),
    List(Vec<String>),
    Map(WorkflowTriggerMap),
}

#[derive(Debug, Clone, Default)]
struct WorkflowTriggerMap {
    pull_request: Option<BranchFilter>,
    pull_request_target: Option<BranchFilter>,
    push: Option<BranchFilter>,
    workflow_dispatch: bool,
    schedule: bool,
    repository_dispatch: bool,
}

impl WorkflowTriggerMap {
    fn pull_request_any() -> Self {
        Self {
            pull_request: Some(BranchFilter::default()),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
struct BranchFilter {
    branches: Vec<String>,
    branches_ignore: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WorkflowEntry {
    path: String,
    #[serde(rename = "type")]
    item_type: String,
}

fn workflow_entries_from_contents(contents: Option<Value>) -> Vec<WorkflowEntry> {
    contents
        .and_then(|value| serde_json::from_value::<Vec<WorkflowEntry>>(value).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            entry.item_type == "file"
                && (entry.path.ends_with(".yml") || entry.path.ends_with(".yaml"))
        })
        .collect()
}

fn decode_workflow_content(value: &Value) -> Option<String> {
    let content = value.get("content")?.as_str()?;
    let encoding = value.get("encoding").and_then(Value::as_str).unwrap_or("");
    if encoding != "base64" {
        return None;
    }
    let compact = content.lines().collect::<String>();
    base64::engine::general_purpose::STANDARD
        .decode(compact)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_workflow_spec(yaml: String) -> Option<WorkflowSpec> {
    let value = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&yaml).ok()?;
    let mapping = value.as_mapping()?;
    let name = mapping
        .get(serde_yaml_ng::Value::String("name".into()))
        .and_then(serde_yaml_ng::Value::as_str)
        .map(ToOwned::to_owned);
    let on = mapping
        .get(serde_yaml_ng::Value::String("on".into()))
        .map(parse_on_triggers)
        .unwrap_or_default();
    Some(WorkflowSpec {
        name,
        path: String::new(),
        on,
    })
}

fn parse_on_triggers(value: &serde_yaml_ng::Value) -> OnTriggers {
    match value {
        serde_yaml_ng::Value::String(single) => OnTriggers::Single(single.clone()),
        serde_yaml_ng::Value::Sequence(sequence) => OnTriggers::List(
            sequence
                .iter()
                .filter_map(serde_yaml_ng::Value::as_str)
                .map(ToOwned::to_owned)
                .collect(),
        ),
        serde_yaml_ng::Value::Mapping(mapping) => OnTriggers::Map(parse_trigger_map(mapping)),
        _ => OnTriggers::None,
    }
}

fn parse_trigger_map(mapping: &serde_yaml_ng::Mapping) -> WorkflowTriggerMap {
    let mut triggers = WorkflowTriggerMap::default();
    for (key, value) in mapping {
        let Some(key) = key.as_str() else {
            continue;
        };
        match key {
            "pull_request" => triggers.pull_request = Some(parse_branch_filter(value)),
            "pull_request_target" => {
                triggers.pull_request_target = Some(parse_branch_filter(value));
            }
            "push" => triggers.push = Some(parse_branch_filter(value)),
            "workflow_dispatch" => triggers.workflow_dispatch = true,
            "schedule" => triggers.schedule = true,
            "repository_dispatch" => triggers.repository_dispatch = true,
            _ => {}
        }
    }
    triggers
}

fn parse_branch_filter(value: &serde_yaml_ng::Value) -> BranchFilter {
    let Some(mapping) = value.as_mapping() else {
        return BranchFilter::default();
    };
    BranchFilter {
        branches: yaml_string_list(mapping.get(serde_yaml_ng::Value::String("branches".into()))),
        branches_ignore: yaml_string_list(
            mapping.get(serde_yaml_ng::Value::String("branches-ignore".into())),
        ),
    }
}

fn yaml_string_list(value: Option<&serde_yaml_ng::Value>) -> Vec<String> {
    match value {
        Some(serde_yaml_ng::Value::String(value)) => vec![value.clone()],
        Some(serde_yaml_ng::Value::Sequence(values)) => values
            .iter()
            .filter_map(serde_yaml_ng::Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn workflow_matches_pr(spec: &WorkflowSpec, base_ref: &str, head_ref: &str) -> bool {
    match &spec.on {
        OnTriggers::Single(trigger) => is_supported_single_trigger(trigger),
        OnTriggers::List(triggers) => triggers
            .iter()
            .any(|trigger| is_supported_single_trigger(trigger)),
        OnTriggers::Map(triggers) => {
            let pull_matches = triggers
                .pull_request
                .as_ref()
                .is_some_and(|filter| filter.matches(base_ref));
            let target_matches = triggers
                .pull_request_target
                .as_ref()
                .is_some_and(|filter| filter.matches(base_ref));
            let push_matches = triggers
                .push
                .as_ref()
                .is_some_and(|filter| filter.matches(head_ref));
            pull_matches || target_matches || push_matches
        }
        OnTriggers::None => false,
    }
}

fn is_supported_single_trigger(trigger: &str) -> bool {
    matches!(trigger, "pull_request" | "pull_request_target" | "push")
}

impl BranchFilter {
    fn matches(&self, branch: &str) -> bool {
        let branches_match = if self.branches.is_empty() {
            true
        } else {
            self.branches
                .iter()
                .any(|pattern| github_pattern_matches(pattern, branch))
        };
        let ignored = self
            .branches_ignore
            .iter()
            .any(|pattern| github_pattern_matches(pattern, branch));
        branches_match && !ignored
    }
}

fn github_pattern_matches(pattern: &str, value: &str) -> bool {
    let mut regex = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    regex.push_str(".*");
                } else {
                    regex.push_str("[^/]*");
                }
            }
            '?' => regex.push('.'),
            _ => regex.push_str(&regex::escape(&ch.to_string())),
        }
    }
    regex.push('$');
    Regex::new(&regex)
        .map(|compiled| compiled.is_match(value))
        .unwrap_or(false)
}

fn disabled_workflow_paths(workflows: &[WorkFlow]) -> HashSet<String> {
    workflows
        .iter()
        .filter(|workflow| {
            matches!(
                workflow.state.as_str(),
                "disabled_manually" | "disabled_inactivity"
            )
        })
        .map(|workflow| workflow.path.clone())
        .collect()
}

fn extract_ruleset_required_contexts(value: &Value) -> Vec<String> {
    let rules = value
        .as_array()
        .cloned()
        .or_else(|| value.get("rules").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    rules
        .into_iter()
        .filter(|rule| rule.get("type").and_then(Value::as_str) == Some("required_status_checks"))
        .flat_map(|rule| {
            rule.get("parameters")
                .and_then(|parameters| parameters.get("required_status_checks"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|check| {
            check
                .get("context")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn extract_classic_required_contexts(value: &Value) -> Vec<String> {
    value
        .get("contexts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| entry.as_str().map(ToOwned::to_owned))
        .collect()
}

fn merge_expected_names(groups: [Vec<String>; 3]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for group in groups {
        names.extend(group);
    }
    names.into_iter().collect()
}

fn collect_registered_names(
    check_runs: &[models::checks::CheckRun],
    workflow_runs: &[models::workflows::Run],
    statuses: &[models::Status],
) -> Vec<String> {
    let mut names = BTreeSet::new();
    for run in check_runs {
        names.insert(run.name.clone());
    }
    for run in workflow_runs {
        names.insert(run.name.clone());
    }
    for status in statuses {
        if let Some(context) = &status.context {
            names.insert(context.clone());
        }
    }
    names.into_iter().collect()
}

fn collect_github_check_items(
    check_runs: &[models::checks::CheckRun],
    workflow_runs: &[models::workflows::Run],
    statuses: &[models::Status],
) -> Vec<PrCheckItem> {
    let mut items = HashMap::<String, PrCheckItem>::new();
    for run in check_runs {
        merge_pr_check_item(
            &mut items,
            run.name.clone(),
            check_run_state(run),
            run.details_url.clone().or(run.html_url.clone()),
        );
    }
    for run in workflow_runs {
        merge_pr_check_item(
            &mut items,
            run.name.clone(),
            workflow_run_state(run),
            Some(run.html_url.to_string()),
        );
    }
    for status in statuses {
        if let Some(context) = &status.context {
            merge_pr_check_item(
                &mut items,
                context.clone(),
                commit_status_state(status),
                status.target_url.clone(),
            );
        }
    }
    let mut values = items.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| left.name.cmp(&right.name));
    values
}

fn merge_pr_check_item(
    items: &mut HashMap<String, PrCheckItem>,
    name: String,
    state: PrChecksStatus,
    url: Option<String>,
) {
    items
        .entry(name.clone())
        .and_modify(|item| {
            item.state = merge_pr_check_states(item.state, state);
            if item.url.is_none() {
                item.url = url.clone();
            }
        })
        .or_insert(PrCheckItem { name, state, url });
}

fn check_run_state(run: &models::checks::CheckRun) -> PrChecksStatus {
    match run.conclusion.as_deref() {
        Some(conclusion) if is_fail_state(conclusion) => PrChecksStatus::Failed,
        Some(conclusion) if is_pass_state(conclusion) => PrChecksStatus::Passed,
        Some(_) => PrChecksStatus::Pending,
        None => PrChecksStatus::Pending,
    }
}

fn workflow_run_state(run: &models::workflows::Run) -> PrChecksStatus {
    if run.status != "completed" {
        return PrChecksStatus::Pending;
    }
    match run.conclusion.as_deref() {
        Some(conclusion) if is_fail_state(conclusion) => PrChecksStatus::Failed,
        Some(conclusion) if is_pass_state(conclusion) => PrChecksStatus::Passed,
        Some(_) => PrChecksStatus::Pending,
        None => PrChecksStatus::Pending,
    }
}

fn commit_status_state(status: &models::Status) -> PrChecksStatus {
    match status.state {
        models::StatusState::Success => PrChecksStatus::Passed,
        models::StatusState::Failure | models::StatusState::Error => PrChecksStatus::Failed,
        models::StatusState::Pending => PrChecksStatus::Pending,
        _ => PrChecksStatus::None,
    }
}

fn merge_pr_check_states(current: PrChecksStatus, next: PrChecksStatus) -> PrChecksStatus {
    use PrChecksStatus::{Failed, None, Passed, Pending};
    match (current, next) {
        (Failed, _) | (_, Failed) => Failed,
        (Pending, _) | (_, Pending) => Pending,
        (Passed, _) | (_, Passed) => Passed,
        _ => None,
    }
}

fn rollup_report_status(
    expected: &[String],
    registered: &[String],
    items: &[PrCheckItem],
    suite_activity: bool,
) -> PrChecksStatus {
    if expected.is_empty() && registered.is_empty() && !suite_activity {
        return PrChecksStatus::None;
    }

    let expected_set = expected.iter().cloned().collect::<HashSet<_>>();
    let registered_set = registered.iter().cloned().collect::<HashSet<_>>();
    let mut any_failed = false;
    let mut all_expected_passed = !expected.is_empty();

    for item in items {
        if !(expected_set.contains(&item.name) || registered_set.contains(&item.name)) {
            continue;
        }
        match item.state {
            PrChecksStatus::Failed => any_failed = true,
            PrChecksStatus::Passed => {}
            _ => {
                if expected_set.contains(&item.name) {
                    all_expected_passed = false;
                }
            }
        }
    }

    if any_failed {
        return PrChecksStatus::Failed;
    }
    if !expected.is_empty() && expected_set.is_subset(&registered_set) && all_expected_passed {
        return PrChecksStatus::Passed;
    }
    PrChecksStatus::Pending
}

fn is_pass_state(state: &str) -> bool {
    matches!(state, "success" | "skipped" | "neutral")
}

fn is_fail_state(state: &str) -> bool {
    matches!(
        state,
        "failure"
            | "error"
            | "cancelled"
            | "timed_out"
            | "action_required"
            | "startup_failure"
            | "stale"
    )
}

fn is_pending_state(state: &str) -> bool {
    matches!(
        state,
        "pending" | "in_progress" | "queued" | "requested" | "waiting"
    )
}

fn map_issue(issue: models::issues::Issue) -> BackendIssueRecord {
    let milestone = issue
        .milestone
        .as_ref()
        .map(|milestone| milestone.title.clone());
    let milestone_id = issue
        .milestone
        .as_ref()
        .and_then(|milestone| u64::try_from(milestone.number).ok());
    BackendIssueRecord {
        issue_id: issue.number,
        node_id: Some(issue.node_id),
        title: issue.title,
        state: match issue.state {
            models::IssueState::Open => "open".into(),
            models::IssueState::Closed => "closed".into(),
            _ => "open".into(),
        },
        state_reason: issue.state_reason.map(github_state_reason_to_string),
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        assignees: issue
            .assignees
            .into_iter()
            .map(|assignee| assignee.login)
            .collect(),
        milestone,
        milestone_id,
        body: issue.body,
        url: issue.html_url.to_string(),
        updated_at: issue.updated_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        due_date: None,
        weight: None,
        confidential: None,
        discussion_locked: None,
        issue_type: None,
        locked: Some(issue.locked),
        lock_reason: issue.active_lock_reason,
        comments: Vec::new(),
        linked_mrs: Vec::new(),
        assignee_account_id: None,
        assignee_name: None,
    }
}

fn map_pull_request(
    pull: models::pulls::PullRequest,
    repo: &str,
) -> Result<BackendPrRecord, RiptaskError> {
    let url = pull
        .html_url
        .map(|url| url.to_string())
        .ok_or_else(|| RiptaskError::Unreachable(format!("missing PR url for {repo}")))?;
    let state = if pull.merged_at.is_some() {
        "merged".into()
    } else {
        match pull.state {
            Some(models::IssueState::Open) => "open".into(),
            Some(models::IssueState::Closed) => "closed".into(),
            _ => "open".into(),
        }
    };
    Ok(BackendPrRecord {
        number: pull.number,
        title: pull.title.unwrap_or_default(),
        body: pull.body.unwrap_or_default(),
        url,
        state,
        head: pull.head.ref_field,
        head_sha: Some(pull.head.sha),
        base: pull.base.ref_field,
        node_id: pull.node_id,
        merged: pull.merged.unwrap_or(false) || pull.merged_at.is_some(),
        updated_at: pull
            .updated_at
            .map(|updated_at| updated_at.to_rfc3339_opts(SecondsFormat::Secs, true))
            .unwrap_or_default(),
    })
}

fn github_state_reason_to_string(reason: models::issues::IssueStateReason) -> String {
    match reason {
        models::issues::IssueStateReason::Completed => "completed".into(),
        models::issues::IssueStateReason::NotPlanned => "not_planned".into(),
        models::issues::IssueStateReason::Reopened => "reopened".into(),
        models::issues::IssueStateReason::Duplicate => "duplicate".into(),
        _ => "completed".into(),
    }
}

fn parse_github_issue_state(state: &str) -> Result<models::IssueState, RiptaskError> {
    match state {
        "open" => Ok(models::IssueState::Open),
        "closed" => Ok(models::IssueState::Closed),
        other => Err(RiptaskError::Config(format!(
            "unsupported github issue state: {other}"
        ))),
    }
}

fn parse_github_state_reason(
    state_reason: &str,
) -> Result<models::issues::IssueStateReason, RiptaskError> {
    match state_reason {
        "completed" => Ok(models::issues::IssueStateReason::Completed),
        "not_planned" => Ok(models::issues::IssueStateReason::NotPlanned),
        "reopened" => Ok(models::issues::IssueStateReason::Reopened),
        "duplicate" => Ok(models::issues::IssueStateReason::Duplicate),
        other => Err(RiptaskError::Config(format!(
            "unsupported github issue state reason: {other}"
        ))),
    }
}

fn parse_lock_reason(reason: &str) -> Option<LockReason> {
    match reason {
        "off-topic" => Some(LockReason::OffTopic),
        "too heated" => Some(LockReason::TooHeated),
        "resolved" => Some(LockReason::Resolved),
        "spam" => Some(LockReason::Spam),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BranchFilter, OnTriggers, PrCheckItem, PrChecksStatus, WorkflowSpec, WorkflowTriggerMap,
        extract_classic_required_contexts, extract_ruleset_required_contexts,
        merge_pr_check_states, rollup_report_status, workflow_matches_pr,
    };
    use serde_json::json;

    #[test]
    fn workflow_dispatch_only_is_not_expected() {
        let spec = WorkflowSpec {
            name: Some("dispatch".into()),
            path: ".github/workflows/dispatch.yml".into(),
            on: OnTriggers::Map(WorkflowTriggerMap {
                workflow_dispatch: true,
                ..WorkflowTriggerMap::default()
            }),
        };
        assert!(!workflow_matches_pr(&spec, "main", "feature"));
    }

    #[test]
    fn pull_request_trigger_matches_base_branch() {
        let spec = WorkflowSpec {
            name: Some("ci".into()),
            path: ".github/workflows/ci.yml".into(),
            on: OnTriggers::Map(WorkflowTriggerMap {
                pull_request: Some(BranchFilter {
                    branches: vec!["main".into()],
                    branches_ignore: Vec::new(),
                }),
                ..WorkflowTriggerMap::default()
            }),
        };
        assert!(workflow_matches_pr(&spec, "main", "feature"));
        assert!(!workflow_matches_pr(&spec, "release", "feature"));
    }

    #[test]
    fn extracts_required_contexts_from_rulesets() {
        let contexts = extract_ruleset_required_contexts(&json!([
            {
                "type": "required_status_checks",
                "parameters": {
                    "required_status_checks": [
                        {"context": "ci"},
                        {"context": "lint"}
                    ]
                }
            }
        ]));
        assert_eq!(contexts, vec!["ci".to_string(), "lint".to_string()]);
    }

    #[test]
    fn extracts_classic_required_contexts() {
        let contexts = extract_classic_required_contexts(&json!({
            "contexts": ["build", "test"]
        }));
        assert_eq!(contexts, vec!["build".to_string(), "test".to_string()]);
    }

    #[test]
    fn rollup_prefers_failed_over_passed() {
        let items = vec![
            PrCheckItem {
                name: "ci".into(),
                state: PrChecksStatus::Passed,
                url: None,
            },
            PrCheckItem {
                name: "lint".into(),
                state: PrChecksStatus::Failed,
                url: None,
            },
        ];
        let expected = vec!["ci".to_string(), "lint".to_string()];
        let registered = expected.clone();
        assert_eq!(
            rollup_report_status(&expected, &registered, &items, false),
            PrChecksStatus::Failed
        );
        assert_eq!(
            merge_pr_check_states(PrChecksStatus::Passed, PrChecksStatus::Failed),
            PrChecksStatus::Failed
        );
    }
}
