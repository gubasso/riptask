# Jira Backend Integration — Implementation Spec

## Overview

Add Jira as an issue tracker backend, and in the process **decouple the architecture into two independent layers**:

- **Issue Tracking layer** (`IssueTracker` trait): GitHub / GitLab / Jira / Local
- **Version Control layer** (`VersionControl` trait): GitHub / GitLab / no-remote

Today these are conflated into a single `BackendProvider` trait because GitHub and GitLab happen to provide both. Jira breaks that assumption — it's an issue tracker with no PRs or branches. Rather than stubbing out PR methods with errors, we split the domain properly so each layer has a clean contract.

**Scope prefix**: `JR` (parallel to `GH` for GitHub, `GL` for GitLab)
**Config key**: `jira` in the `Backend` enum
**Repo field meaning**: Jira project key (e.g., `"PROJ"`)

---

## Phase 0: Architectural Split — IssueTracker + VersionControl

**This phase is the foundation. It must be completed before adding Jira.**

### 0.1 — The problem

The current `BackendProvider` trait (`src/adapters/backend.rs:90-166`) has 20 methods mixing two concerns:

| Concern | Methods | Count |
|---|---|---|
| Issue tracking | `list_issues`, `get_issue`, `create_issue`, `update_issue`, `close_issue`, `reopen_issue`, `delete_issue`, `lock_issue`, `unlock_issue`, `sync_labels` | 10 |
| Version control | `create_pr`, `get_pr`, `update_pr`, `find_pr_by_branch`, `merge_pr`, `get_pr_checks_status`, `get_ci_presence`, `create_branch`, `default_branch`, `delete_branch` | 10 |

This works when GitHub/GitLab serve both roles, but forces Jira to implement 10 stub methods it can never support. Worse, it prevents a natural configuration like "issues in Jira, PRs on GitHub".

### 0.2 — The design

Split into two traits. `BackendProvider` becomes a convenience alias for implementations that do both:

```rust
#[async_trait]
pub trait IssueTracker: Send + Sync {
    async fn list_issues(&self, repo: &str) -> Result<Vec<BackendIssueRecord>, RiptskError>;
    async fn get_issue(&self, repo: &str, issue_id: u64) -> Result<BackendIssueRecord, RiptskError>;
    async fn create_issue(&self, repo: &str, issue: &BackendIssueUpsert) -> Result<BackendIssueRecord, RiptskError>;
    async fn update_issue(&self, repo: &str, issue_id: u64, issue: &BackendIssueUpsert) -> Result<BackendIssueRecord, RiptskError>;
    async fn close_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError>;
    async fn reopen_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError>;
    async fn delete_issue(&self, repo: &str, issue_id: u64) -> Result<DeleteOutcome, RiptskError>;
    async fn lock_issue(&self, repo: &str, issue_id: u64, reason: Option<&str>) -> Result<(), RiptskError>;
    async fn unlock_issue(&self, repo: &str, issue_id: u64) -> Result<(), RiptskError>;
    async fn sync_labels(&self, repo: &str, issue_id: u64, labels: &[String]) -> Result<(), RiptskError>;
}

#[async_trait]
pub trait VersionControl: Send + Sync {
    async fn create_pr(&self, repo: &str, head: &str, base: &str, title: &str, body: &str) -> Result<BackendPrRecord, RiptskError>;
    async fn get_pr(&self, repo: &str, number: u64) -> Result<BackendPrRecord, RiptskError>;
    async fn update_pr(&self, repo: &str, number: u64, title: &str, body: &str) -> Result<BackendPrRecord, RiptskError>;
    async fn find_pr_by_branch(&self, repo: &str, head: &str, base: &str) -> Result<Option<BackendPrRecord>, RiptskError>;
    async fn merge_pr(&self, repo: &str, number: u64, method: MergeMethod, commit_title: Option<&str>, commit_message: Option<&str>) -> Result<(), RiptskError>;
    async fn get_pr_checks_status(&self, repo: &str, number: u64) -> Result<PrChecksStatus, RiptskError>;
    async fn get_ci_presence(&self, repo: &str) -> Result<CiPresence, RiptskError>;
    async fn create_branch(&self, repo: &str, branch_name: &str, base_ref: &str, issue_id: u64) -> Result<(), RiptskError>;
    async fn default_branch(&self, repo: &str) -> Result<String, RiptskError>;
    async fn delete_branch(&self, repo: &str, branch_name: &str) -> Result<(), RiptskError>;
}

/// Convenience: backends that provide both (GitHub, GitLab).
pub trait BackendProvider: IssueTracker + VersionControl {}
impl<T: IssueTracker + VersionControl> BackendProvider for T {}
```

### 0.3 — Config model change

Current `BackendConfig` assumes one backend = one provider for everything. We need to allow **independent issue tracker and VC per project**.

**New config model** — add an optional `vc` (version control) field alongside the existing backends:

```yaml
backends:
  # GitHub does both — works exactly as before
  - name: github-myrepo
    backend: github
    repo: owner/repo

  # Jira for issues, GitHub for PRs/branches
  - name: jira-myteam
    backend: jira
    host: https://myteam.atlassian.net
    repo: PROJ
    default_board: sprint
    vc: github-myrepo          # ← links to the VC backend for PRs/branches

  # GitLab standalone — also does both
  - name: gitlab-project
    backend: gitlab
    host: https://gitlab.com
    repo: group/project
```

**Rules**:
- `vc` is **optional**. If omitted and backend supports VC (GitHub/GitLab), it provides its own.
- If `vc` is omitted and backend is **issue-only** (Jira), PR/branch commands simply aren't available for that project.
- `vc` must reference another backend by `name`. That backend must support `VersionControl` (GitHub or GitLab).
- A backend can be referenced as `vc` by multiple issue backends (e.g., one GitHub repo serving PRs for multiple Jira projects).

**`BackendConfig` struct change**:
```rust
pub struct BackendConfig {
    pub name: String,
    pub backend: Backend,
    pub host: Option<String>,
    pub repo: Option<String>,
    pub default_board: Option<String>,
    pub default_org: Option<String>,
    pub path: Option<String>,
    pub vc: Option<String>,        // ← NEW: name of VC backend
}
```

### 0.4 — Provider builder changes

**File**: `src/services/backend_mapping.rs`

Split `build_provider_for_backend` into two builders:

```rust
/// Build an issue tracker for a backend config.
/// Works for Github, Gitlab, Jira. Errors for Local.
pub fn build_issue_tracker(
    backend: &BackendConfig,
) -> Result<Box<dyn IssueTracker>, RiptskError>

/// Build a version control provider for a backend config.
/// Works for Github, Gitlab. Errors for Jira, Local.
pub fn build_version_control(
    backend: &BackendConfig,
) -> Result<Box<dyn VersionControl>, RiptskError>

/// Resolve the VC provider for a given issue backend.
/// If backend has `vc` field, look up that backend and build its VC provider.
/// If backend itself supports VC (Github/Gitlab), use itself.
/// If neither, return None (issue-only project).
pub fn resolve_vc_for_backend(
    backend: &BackendConfig,
    config: &Config,
) -> Result<Option<(Box<dyn VersionControl>, &BackendConfig)>, RiptskError>
```

The old `build_provider_for_backend` can remain as a convenience that returns `Box<dyn BackendProvider>` — it just calls both builders internally. This keeps existing call sites working during migration.

### 0.5 — Command call site migration

Each command receives the provider(s) it actually needs:

| Command | Before | After |
|---|---|---|
| `sync pull/push` | `&dyn BackendProvider` | `&dyn IssueTracker` |
| `tsk new` | `&dyn BackendProvider` | `&dyn IssueTracker` |
| `tsk ls/show` | local only | no change |
| `pr show/edit/create` | `&dyn BackendProvider` | `&dyn VersionControl` |
| `branch create/delete` | `&dyn BackendProvider` | `&dyn VersionControl` + issue context |
| `tsk start` | `&dyn BackendProvider` | `&dyn IssueTracker` + `Option<&dyn VersionControl>` |
| `tsk done` | `&dyn BackendProvider` | `&dyn IssueTracker` + `Option<&dyn VersionControl>` |
| `pr merge` | `&dyn BackendProvider` | `&dyn IssueTracker` + `&dyn VersionControl` |

**Key behavior changes**:
- `tsk start` with Jira (no VC): creates issue → done. No branch, no PR. Clean.
- `tsk start` with Jira + GitHub VC: creates Jira issue → GitHub branch → GitHub PR → links PR URL on local issue. Full workflow.
- `tsk done` with Jira (no VC): closes Jira issue → done. No PR merge.
- `tsk done` with Jira + GitHub VC: merges GitHub PR → deletes GitHub branch → closes Jira issue.

### 0.6 — `create_branch` issue_id parameter

The `create_branch(repo, branch_name, base_ref, issue_id)` signature couples VC to issues. This is intentional — GitHub uses it for linked branches (GraphQL `createLinkedBranch`), GitLab may associate branches with issues.

**Keep this parameter.** The VC layer receives the issue ID from the calling command, which has access to both layers. The VC provider can use or ignore the issue_id as it sees fit. This is data flow, not trait coupling.

### 0.7 — Implementation steps

- [ ] **0.7.1** — Define `IssueTracker` and `VersionControl` traits in `src/adapters/backend.rs`. Add blanket `BackendProvider` impl.
- [ ] **0.7.2** — Split `GithubProvider` to implement both `IssueTracker` and `VersionControl` separately (two `impl` blocks, same struct).
- [ ] **0.7.3** — Split `GitlabProvider` to implement both `IssueTracker` and `VersionControl` separately.
- [ ] **0.7.4** — Add `vc: Option<String>` field to `BackendConfig` in `src/models/backend.rs`.
- [ ] **0.7.5** — Add config validation: if `vc` is set, it must reference an existing backend that supports VC (Github or Gitlab).
- [ ] **0.7.6** — Implement `build_issue_tracker()`, `build_version_control()`, and `resolve_vc_for_backend()` in `src/services/backend_mapping.rs`.
- [ ] **0.7.7** — Migrate `SyncEngine` to accept `&dyn IssueTracker` instead of `&dyn BackendProvider`. This is the cleanest migration — sync uses zero VC methods.
- [ ] **0.7.8** — Migrate `src/commands/issues.rs` (`tsk new`) to accept `&dyn IssueTracker`.
- [ ] **0.7.9** — Migrate `src/commands/pr.rs` to accept `&dyn VersionControl`.
- [ ] **0.7.10** — Migrate `src/commands/branch.rs` to accept `&dyn VersionControl`.
- [ ] **0.7.11** — Migrate `src/commands/start.rs` to accept `&dyn IssueTracker` + `Option<&dyn VersionControl>`. Skip PR/branch steps when VC is `None`.
- [ ] **0.7.12** — Migrate `src/commands/done.rs` to accept `&dyn IssueTracker` + `Option<&dyn VersionControl>`. Skip PR merge/branch delete when VC is `None`.
- [ ] **0.7.13** — Run `make check` — all existing tests must pass with no behavior change for GitHub/GitLab configurations.

---

## Phase 1: Research & Decisions

Before writing Jira-specific code, resolve these open questions.

### 1.1 — Jira Rust library selection

**Decision: Use `gouqi` (async) as the primary Jira client, with direct `reqwest` calls for corner cases.**

#### Research summary

| Crate | Version | Downloads | Async | Cloud+Server | Maintained | Verdict |
|---|---|---|---|---|---|---|
| **`gouqi`** | 0.20.0 (2025-10) | 54k | Yes (feature flag) | Both | Active | **Selected** |
| `jira_v3_openapi` | 1.6.1 (2026-02) | 49k | Yes | Cloud only | Active | Cloud-only, auto-generated, verbose |
| `jira_query` | 1.7.2 (2026-03) | 31k | Yes | Both | Active (Red Hat) | Read-only, no CRUD |
| `jira-issue-api` | 0.6.1 (2025-10) | 22k | Yes | Both | Low activity | No issue creation |
| `goji` | 0.2.4 (2018-07) | 17k | No | — | Dead | Superseded by `gouqi` |
| `jira-api-v2` | 1.0.1 (2025-02) | 1.6k | Yes | Both | Low | Auto-generated, barely used |

#### Why `gouqi`

- **Async/tokio**: Enabled via `async` feature flag — matches our runtime.
- **reqwest 0.12**: Same version octocrab brings in — no dependency conflicts.
- **Cloud + Server/DC**: Supports Basic auth (Cloud), PAT (Server), Bearer, Cookie.
- **API v2 and v3**: Handles both versions, including ADF support for v3 rich text.
- **Endpoint coverage**: Issues CRUD, JQL search, transitions, boards, sprints, labels, users, components, versions, attachments, worklogs, comments.
- **Actively maintained**: Regular releases, conventional commits, responsive maintainer.
- **Follows our pattern**: Just like `octocrab` for GitHub and `gitlab` crate for GitLab — a typed Rust client that covers the common operations.

#### Corner cases: direct `reqwest` calls

For endpoints that `gouqi` doesn't cover or where we need finer control, use direct `reqwest` calls against the Jira REST API — same pattern already established in the project:

- **GitHub**: `octocrab` for most operations, raw `.get::<serde_json::Value>()` and `.graphql()` for CI checks, linked branches, and status queries (`src/adapters/github.rs:367,448,575`).
- **GitLab**: `gitlab` crate for most operations, with the crate's query builders giving typed access.
- **Jira**: `gouqi` for issues, transitions, search, labels. Direct `reqwest` for anything missing (e.g., specific custom field queries, bulk operations, dev panel integration data).

Since `gouqi` is built on `reqwest`, we can reuse its configured HTTP client (with auth headers already set) for direct API calls, avoiding duplicate auth setup.

#### API version strategy

- **Use REST API v2** (`/rest/api/2/`) as primary target — it uses plain text for descriptions and comments, which aligns with our markdown-based issue documents and avoids the ADF conversion complexity entirely.
- **Server/Data Center only supports v2** (v3 does not exist on-prem), so v2 gives us automatic compatibility with both Cloud and Server.
- `gouqi` supports both v2 and v3, so we can start with v2 and upgrade specific calls to v3 later if needed (e.g., for rich text features).

#### Implementation steps

- [ ] **1.1.1** — Add `gouqi` dependency to `Cargo.toml`:
  ```toml
  gouqi = { version = "0.20", features = ["async"] }
  ```
- [ ] **1.1.2** — Create `src/adapters/jira.rs` with a `JiraProvider` struct wrapping `gouqi::Jira` (async client). Expose a constructor `JiraProvider::new(host, auth)` that builds the client.
- [ ] **1.1.3** — Verify `gouqi` connectivity: write a minimal integration smoke test that authenticates and calls `list_issues` (JQL search) against a test Jira project. This validates the crate works before building out the full provider.
- [ ] **1.1.4** — Identify `gouqi` coverage gaps: map every `IssueTracker` trait method to its `gouqi` equivalent. For methods with no `gouqi` support, document the raw REST API endpoint to call via `reqwest`.
- [ ] **1.1.5** — Set up the direct-API escape hatch: extract `gouqi`'s inner `reqwest::Client` (or build a sibling client with the same auth headers) so we can make raw calls for corner cases without duplicating credentials.

### 1.2 — Jira authentication model

**Decision: Piggyback on `jira-cli-go` for persistent auth (same pattern as `gh`/`glab`), with env var override.**

#### How riptsk already handles auth (the pattern to follow)

riptsk never stores credentials itself — it piggybacks on external CLI tools:

| Backend | Env vars | CLI fallback | "Login once" command |
|---|---|---|---|
| GitHub | `GITHUB_TOKEN` → `GH_TOKEN` | `gh auth token --hostname github.com` | `gh auth login` |
| GitLab | `GITLAB_TOKEN` | `glab auth status --show-token --hostname {host}` | `glab auth login` |
| **Jira** | `JIRA_API_TOKEN` + `JIRA_EMAIL` | `jira-cli-go` config + system keychain | `jira init` |

#### Credential resolution chain

```
resolve_jira_credentials(backend_name, host)
  1. JIRA_API_TOKEN env var set?
     ├─ Yes + JIRA_EMAIL set? → Basic auth (Cloud)
     └─ Yes + no JIRA_EMAIL?  → PAT auth (Server/DC)
  2. jira-cli-go installed?
     ├─ Parse ~/.config/.jira/.config.yml → get server + login (email)
     └─ Read API token from system keychain (service: "jira-cli", user: login email)
  3. All failed → RiptskError::Auth with helpful message:
       "Jira authentication failed for backend '{name}' (host: {host})
        Tried: JIRA_API_TOKEN, jira-cli-go keychain
        To fix, do one of:
          • export JIRA_API_TOKEN=<token> JIRA_EMAIL=<email>
          • brew install ankitpokhrel/jira-cli/jira-cli && jira init"
```

#### Why `jira-cli-go`

- ~3.8k GitHub stars, actively maintained, the de facto standard Jira CLI
- `jira init` is interactive: prompts for server URL, email, API token — one-time setup
- Stores config at `~/.config/.jira/.config.yml` (YAML: server, login, project)
- Stores the **API token in the system keychain** (macOS Keychain, GNOME Keyring, Windows Credential Manager)
- No official Atlassian CLI exists with persistent auth — this is the community standard
- Same UX as `gh auth login` / `glab auth login`: run once, forget it

#### Auth types by platform

| Platform | Auth method | Credentials needed |
|---|---|---|
| Jira Cloud | Basic auth (RFC 7617) | `email` + `API token` (base64 encoded as `email:token`) |
| Jira Server/DC | PAT (Bearer token) | `PAT` only (no email needed) |

`gouqi` supports both via its `Credentials` enum — we just pass the right variant based on what we resolved.

#### New dependency: `keyring` crate

Reading from the system keychain requires the [`keyring`](https://crates.io/crates/keyring) crate:
- Cross-platform: macOS Keychain, GNOME Keyring / `secret-service`, Windows Credential Manager
- Widely used (~1.8M downloads), actively maintained
- We read with service `"jira-cli"` and the user's login email as the username

#### Implementation steps

- [ ] **1.2.1** — Add `keyring` dependency to `Cargo.toml`:
  ```toml
  keyring = "3"
  ```
- [ ] **1.2.2** — Add `serde_yaml_ng` parsing for `jira-cli-go` config (already a project dependency — no new dep needed). Create a small struct to deserialize `~/.config/.jira/.config.yml`:
  ```rust
  struct JiraCliGoConfig {
      server: String,
      login: String,  // email
  }
  ```
- [ ] **1.2.3** — Implement `run_jira_cli_go_auth(host: &str) -> Result<(String, String), String>` that:
  1. Reads and parses the config YAML
  2. Validates the `server` field matches the expected `host`
  3. Reads the API token from the system keychain (`keyring::Entry::new("jira-cli", &login)`)
  4. Returns `(email, token)` pair
- [ ] **1.2.4** — Implement `resolve_jira_credentials(backend_name: &str, host: &str) -> Result<(JiraAuth, CredentialSource), RiptskError>` following the chain above (env vars → `jira-cli-go` → error with guidance).
- [ ] **1.2.5** — Define `JiraAuth` enum in `src/adapters/jira.rs`:
  ```rust
  pub enum JiraAuth {
      Basic { email: String, token: String },  // Jira Cloud
      Pat(String),                              // Jira Server/DC
  }
  ```
- [ ] **1.2.6** — Wire into `build_issue_tracker()`: add `Backend::Jira` arm that calls `resolve_jira_credentials()` and constructs `JiraProvider`.

### 1.3 — Jira issue state mapping

**Decision: Map via Jira's `statusCategory.key` (a fixed 4-value set), with `status::` labels for fine-grained riptsk states. Use transitions API for all state changes.**

#### Jira's status model (background)

Jira statuses are fully customizable per project/workflow, but every status **must** belong to one of exactly **4 hardcoded status categories**:

| Category ID | `statusCategory.key` | `statusCategory.name` | Color | Notes |
|---|---|---|---|---|
| 1 | `undefined` | `No Category` | medium-gray | System artifact — not assignable via UI, appears after migrations |
| 2 | `new` | `To Do` | blue-gray | Issues not yet started |
| 3 | `done` | `Done` | green | Issues considered complete |
| 4 | `indeterminate` | `In Progress` | yellow | Issues actively being worked |

These categories are **identical across Cloud and Server/Data Center** and are language-agnostic when using the `key` field. The `key` is the reliable mapping anchor.

#### Reading state: `statusCategory.key` → `IssueState`

When pulling issues from Jira, map using the category key from `fields.status.statusCategory.key`:

| `statusCategory.key` | `BackendIssueRecord.state` | `IssueState` (via `state_from_backend`) |
|---|---|---|
| `new` | `"open"` | Determined by `status::` label (default: `Todo`) |
| `indeterminate` | `"open"` | Determined by `status::` label (default: `InProgress`) |
| `done` | `"closed"` | `Done` |
| `undefined` | `"open"` | Determined by `status::` label (default: `Backlog`) |

This plugs cleanly into the existing `state_from_backend()` logic (`src/services/backend_mapping.rs:411-428`), which already:
1. Checks if `record.state` is `"closed"` → returns `Done`
2. Falls back to `status::` label prefix for fine-grained states
3. Defaults to `Backlog` if no label matches

**Fine-grained state via labels**: Since Jira's 3 usable categories don't distinguish `Backlog` vs `Todo` or `InProgress` vs `Review`, we use the existing `status::` label mechanism (same as GitHub/GitLab). When pulling from Jira:
- Set `status::backlog` if `key == "new"` and Jira status name contains "backlog" (case-insensitive)
- Set `status::todo` if `key == "new"` otherwise
- Set `status::in-progress` if `key == "indeterminate"` and status name doesn't suggest review
- Set `status::review` if `key == "indeterminate"` and Jira status name contains "review" (case-insensitive)
- `Done` category doesn't need a label — `state_from_backend()` catches `"closed"` directly

This heuristic covers common Jira statuses:

| Jira status name | Category key | `status::` label assigned | `IssueState` |
|---|---|---|---|
| `Backlog` | `new` | `status::backlog` | `Backlog` |
| `To Do`, `Open`, `Reopened` | `new` | `status::todo` | `Todo` |
| `In Progress`, `In Development` | `indeterminate` | `status::in-progress` | `InProgress` |
| `In Review`, `Code Review`, `In QA` | `indeterminate` | `status::review` | `Review` |
| `Done`, `Closed`, `Resolved`, `Cancelled` | `done` | (none needed) | `Done` |

#### Writing state: transitions API (mandatory)

**You cannot directly set `fields.status` on a Jira issue.** The only way to change status is via the transitions endpoint:

1. `GET /rest/api/2/issue/{id}/transitions?expand=transitions.fields` — discover available transitions
2. Find the transition whose `to.statusCategory.key` matches the target category
3. `POST /rest/api/2/issue/{id}/transitions` with the transition ID (and resolution if closing)

**Transition selection algorithm:**

```
fn find_transition(transitions, target_state: IssueState) -> Option<Transition>:
    target_category = match target_state {
        Backlog | Todo     => "new"
        InProgress | Review => "indeterminate"
        Done                => "done"
    }

    candidates = transitions
        .filter(|t| t.to.statusCategory.key == target_category)
        .filter(|t| t.isAvailable)

    // Prefer transitions matching the exact status name
    if let Some(exact) = candidates.find(|t| name_matches_state(t.to.name, target_state)):
        return Some(exact)

    // Fall back to any transition reaching the target category
    return candidates.first()
```

Where `name_matches_state` does case-insensitive matching:
- `Backlog` → status name contains "backlog"
- `Todo` → status name contains "to do" or "open"
- `InProgress` → status name contains "progress"
- `Review` → status name contains "review"
- `Done` → any `done` category status

**If no valid transition exists** (workflow doesn't allow it from current status), return a clear error: `"Cannot transition PROJ-123 from '{current}' to '{target}' — no available transition in the workflow"`.

#### Resolution handling

Jira resolutions map to riptsk's `state_reason` field:

| riptsk `state_reason` | Jira `resolution.name` | Direction |
|---|---|---|
| `"completed"` | `"Done"` | Both ways |
| `"not_planned"` | `"Won't Do"` | Both ways |
| `"duplicate"` | `"Duplicate"` | Both ways |
| `None` / other | `"Done"` (default) | Push only |

**When transitioning to `done` category** (closing):
- Check if the transition has `hasScreen: true` with a required `resolution` field
- Always include resolution in the POST body to be safe:
  ```json
  {
    "transition": { "id": "31" },
    "fields": {
      "resolution": { "name": "Done" }
    }
  }
  ```
- Use the mapped resolution name from `state_reason`, defaulting to `"Done"`

**When transitioning away from `done` category** (reopening):
- Simplified workflows clear resolution automatically
- Classic workflows rely on a "Clear Field Value" post function — if absent, resolution may persist
- After reopening, explicitly clear resolution via `PUT /rest/api/2/issue/{id}` with `{"fields": {"resolution": null}}` as a safety measure

**When pulling from Jira**:
- `fields.resolution == null` → `state_reason: None`
- `fields.resolution.name == "Done"` → `state_reason: Some("completed")`
- `fields.resolution.name == "Won't Do"` → `state_reason: Some("not_planned")`
- `fields.resolution.name == "Duplicate"` → `state_reason: Some("duplicate")`
- Other resolutions → `state_reason: Some("completed")` (safe default)

#### Edge cases

| Scenario | Handling |
|---|---|
| Multiple `done`-category statuses (`Resolved`, `Closed`, `Cancelled`) | All map to `IssueState::Done` — category key is the anchor, not status name |
| Multiple transitions to same target status | Pick `isAvailable == true`, prefer exact name match, fall back to first |
| Transition requires screen fields (`hasScreen: true`) | Always send resolution when closing. For other required fields, log a warning and attempt anyway — API doesn't enforce screens |
| Sub-tasks with different workflows | Treated as independent issues — same transition logic applies |
| 409 Conflict on simultaneous transitions | Retry once after a short delay (Jira Cloud-specific) |
| `statusCategory.key == "undefined"` | Treat as `"new"` (Backlog), log a warning |

#### `sanitize_state_reason` for Jira

Update `sanitize_state_reason()` in `src/services/backend_mapping.rs` to handle Jira's resolution model. Since the function currently only validates GitHub-style reasons, add Jira-aware validation when the backend is Jira (or make it backend-agnostic by accepting any known reason).

#### Implementation steps

- [ ] **1.3.1** — Implement `jira_status_to_backend_state(status_category_key: &str) -> &str` that maps category keys to `"open"` / `"closed"` for `BackendIssueRecord.state`.
- [ ] **1.3.2** — Implement `jira_status_to_label(status_category_key: &str, status_name: &str) -> String` that produces the appropriate `status::` label using the heuristic table above.
- [ ] **1.3.3** — Implement `jira_resolution_to_state_reason(resolution: Option<&str>) -> Option<String>` for pull direction (Jira resolution → riptsk state_reason).
- [ ] **1.3.4** — Implement `state_reason_to_jira_resolution(reason: Option<&str>) -> &str` for push direction (riptsk state_reason → Jira resolution name, default `"Done"`).
- [ ] **1.3.5** — Implement `find_transition(transitions: &[JiraTransition], target: &IssueState) -> Result<&JiraTransition, RiptskError>` with the selection algorithm above (category match → name match → first available → error).
- [ ] **1.3.6** — Implement the close flow in `JiraProvider::close_issue()`: get transitions → find done-category transition → POST with resolution.
- [ ] **1.3.7** — Implement the reopen flow in `JiraProvider::reopen_issue()`: get transitions → find new-category transition → POST → clear resolution via PUT.
- [ ] **1.3.8** — Add unit tests for the mapping functions covering: all 4 category keys, common Jira status names (Backlog, To Do, In Progress, In Review, Done, Closed, Resolved, Cancelled, Reopened), all resolution mappings, and the `undefined` category edge case.

### 1.4 — Jira field mapping to BackendIssueRecord

**Decision: Use REST API v2 (plain text / wiki markup strings — no ADF). Map `fixVersions[0]` as milestone. Skip custom fields (story points, sprint) in v1.**

#### Description format — no ADF conversion needed

Since we're using API v2 (decided in 1.1), the `description` and `comment.body` fields are **plain text strings with optional Jira wiki markup** — NOT ADF. This eliminates the biggest complexity risk.

- GitHub body: markdown string → `BackendIssueRecord.body`
- GitLab description: markdown string → `BackendIssueRecord.body`
- Jira description: wiki markup string → `BackendIssueRecord.body`

Wiki markup is close enough to markdown for round-tripping in a CLI context. Both are human-readable plain text. We store the raw wiki markup string as-is, same as GitHub stores raw markdown. No conversion layer needed for v1.

**Future enhancement**: If precision matters, add a lightweight wiki-markup ↔ markdown converter later. The syntaxes overlap significantly (`*bold*`, lists, code blocks differ slightly).

#### Complete field mapping

**Pull direction** — Jira API v2 response → `BackendIssueRecord`:

| `BackendIssueRecord` field | Jira API v2 path | Mapping logic |
|---|---|---|
| `issue_id: u64` | `id` (string → parse) | Jira returns `id` as string (`"10042"`), parse to u64. **Not** the key (`PROJ-123`). |
| `node_id` | — | `None` (GitHub-specific) |
| `title` | `fields.summary` | Direct copy |
| `state` | `fields.status.statusCategory.key` | `"done"` → `"closed"`, all others → `"open"` (per §1.3) |
| `state_reason` | `fields.resolution.name` | `"Done"` → `"completed"`, `"Won't Do"` → `"not_planned"`, `"Duplicate"` → `"duplicate"`, `null` → `None` (per §1.3) |
| `labels` | `fields.labels` | Direct copy (`string[]`). Append `status::` label from status heuristic (per §1.3). |
| `assignees` | `fields.assignee.displayName` | Wrap single assignee in `vec![]`. `null` → empty vec. Jira supports **one assignee only**. |
| `milestone` | `fields.fixVersions[0].name` | First fix version name, or `None` if empty. |
| `milestone_id` | `fields.fixVersions[0].id` | First fix version ID (string → parse to u64). |
| `body` | `fields.description` | Direct copy (wiki markup string). `null` → `None`. |
| `url` | constructed | `"{host}/browse/{key}"` (e.g., `https://myteam.atlassian.net/browse/PROJ-123`). NOT `self` (that's the API URL). |
| `updated_at` | `fields.updated` | Jira format: `2024-03-28T09:15:42.123+0000`. Convert to RFC 3339 (add colon in tz offset: `+00:00`). |
| `due_date` | `fields.duedate` | Direct copy (`"YYYY-MM-DD"` string or `null`). |
| `weight` | — | `None` in v1. Story points is a custom field (`customfield_XXXXX`) — skip for now. |
| `confidential` | `fields.security` | `Some(true)` if security level is set (non-null), `None` otherwise. |
| `discussion_locked` | — | `None` (not a Jira concept). |
| `issue_type` | `fields.issuetype.name` | Direct copy (`"Task"`, `"Bug"`, `"Story"`, `"Epic"`, etc.). |
| `locked` | — | `None` (not a Jira concept). |
| `lock_reason` | — | `None`. |
| `comments` | `fields.comment.comments[].body` | Collect all comment body strings into `Vec<String>`. Wiki markup, stored as-is. |
| `linked_mrs` | — | Empty vec. Jira has no native MR/PR concept. |

**Push direction** — `BackendIssueUpsert` → Jira API v2 create/update payload:

| `BackendIssueUpsert` field | Jira API v2 field | Mapping logic |
|---|---|---|
| `title` | `fields.summary` | Direct |
| `body` | `fields.description` | Direct (wiki markup string) |
| `state` | — | **Not settable directly.** State changes go through transitions API (§1.3). |
| `state_reason` | — | Set as `fields.resolution` during transition POST (§1.3). |
| `labels` | `fields.labels` | Direct (`string[]`). Include `status::` labels. |
| `assignees` | `fields.assignee` | Take first assignee, resolve to `accountId` (Cloud) or `name` (Server). See below. |
| `milestone_id` | `fields.fixVersions` | `[{"id": "{milestone_id}"}]` if set. |
| `due_date` | `fields.duedate` | Direct (`"YYYY-MM-DD"` or `null`). |
| `weight` | — | Skipped in v1 (custom field). |
| `confidential` | — | Skipped in v1 (issue security levels are instance-specific). |
| `discussion_locked` | — | N/A. |

#### Assignee resolution challenge

Jira `assignee` is a single-user object, not a list. And the identifier differs by platform:

| Platform | Set assignee via | Example |
|---|---|---|
| Jira Cloud | `accountId` | `{"assignee": {"accountId": "5b10a284..."}}` |
| Jira Server/DC | `name` (username) | `{"assignee": {"name": "jdoe"}}` |

**Problem**: `BackendIssueUpsert.assignees` contains display names (strings). To push assignee changes to Jira Cloud, we need to resolve `displayName` → `accountId`.

**Strategy**:
1. On **pull**: Store `displayName` in `assignees` (for display) and `accountId` in `JiraIssueMeta` (for push).
2. On **push**: If `JiraIssueMeta.assignee_account_id` is available, use it directly. Otherwise, call `GET /rest/api/2/user/assignable/search?project={key}&query={displayName}` to resolve.
3. On **Server/DC**: Store `name` (username) in `JiraIssueMeta.assignee_name` and use directly.

This means `JiraIssueMeta` needs assignee tracking fields (see §3.1 update).

#### Jira issue key tracking

Jira issues have both a numeric `id` (`"10042"`) and a human-readable `key` (`"PROJ-123"`). We use `id` for `BackendIssueRecord.issue_id` (consistent with GitHub/GitLab), but the `key` is essential for:
- Constructing browse URLs (`{host}/browse/{key}`)
- Transition API calls (`/rest/api/2/issue/{key}/transitions`)
- Human-readable references

Store the `key` in `JiraIssueMeta.issue_key` (see §3.1 update).

#### Timestamp format normalization

Jira's timestamp format (`2024-03-28T09:15:42.123+0000`) differs from RFC 3339 (`2024-03-28T09:15:42.123+00:00`). The project uses RFC 3339 strings throughout (`updated_at` fields).

**Conversion**: Parse with `chrono::DateTime::parse_from_str` using format `%Y-%m-%dT%H:%M:%S%.3f%z` (chrono handles both `+0000` and `+00:00`), then output with `.to_rfc3339()`.

#### Skipped fields (v1 scope)

These Jira fields are intentionally skipped in the initial implementation:

| Field | Why skipped | Future path |
|---|---|---|
| `story_points` | Custom field — ID varies per instance, needs discovery via `GET /rest/api/2/field` | Add config option `story_points_field: "customfield_10028"` |
| `sprint` | Custom field — same discovery issue, also board-scoped | Add optional sprint mapping |
| `components` | No `BackendIssueRecord` equivalent | Could map to labels with `component::` prefix |
| `reporter` | Not tracked in riptsk's model | N/A |
| `watchers` | Not tracked | N/A |
| `attachments` | Not tracked | N/A |
| `subtasks` / `parent` | Not tracked — each issue is independent | Could add hierarchy support later |
| `worklog` | Not tracked | N/A |

#### Jira-specific deserialization structs

Define these in `src/adapters/jira.rs` for parsing API v2 responses:

```rust
#[derive(Deserialize)]
struct JiraIssue {
    id: String,                          // numeric ID as string
    key: String,                         // "PROJ-123"
    fields: JiraFields,
}

#[derive(Deserialize)]
struct JiraFields {
    summary: String,
    description: Option<String>,         // wiki markup
    status: JiraStatus,
    resolution: Option<JiraResolution>,
    labels: Vec<String>,
    assignee: Option<JiraUser>,
    #[serde(rename = "fixVersions")]
    fix_versions: Vec<JiraVersion>,
    #[serde(rename = "issuetype")]
    issue_type: JiraIssueType,
    priority: Option<JiraPriority>,
    duedate: Option<String>,             // "YYYY-MM-DD"
    security: Option<JiraSecurityLevel>,
    comment: Option<JiraCommentPage>,
    updated: String,                     // "2024-03-28T09:15:42.123+0000"
    created: String,
}

#[derive(Deserialize)]
struct JiraStatus {
    name: String,
    #[serde(rename = "statusCategory")]
    status_category: JiraStatusCategory,
}

#[derive(Deserialize)]
struct JiraStatusCategory {
    key: String,        // "new", "indeterminate", "done", "undefined"
    name: String,
}

#[derive(Deserialize)]
struct JiraResolution {
    name: String,       // "Done", "Won't Do", "Duplicate", etc.
}

#[derive(Deserialize)]
struct JiraUser {
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "accountId")]
    account_id: Option<String>,          // Cloud only
    name: Option<String>,                // Server/DC only
}

#[derive(Deserialize)]
struct JiraVersion {
    id: String,                          // numeric as string
    name: String,
}

#[derive(Deserialize)]
struct JiraIssueType {
    name: String,                        // "Task", "Bug", "Story", etc.
    subtask: bool,
}

#[derive(Deserialize)]
struct JiraPriority {
    name: String,                        // "Highest", "High", "Medium", "Low", "Lowest"
}

#[derive(Deserialize)]
struct JiraSecurityLevel {
    name: String,
}

#[derive(Deserialize)]
struct JiraCommentPage {
    comments: Vec<JiraComment>,
}

#[derive(Deserialize)]
struct JiraComment {
    body: String,                        // wiki markup
}

#[derive(Deserialize)]
struct JiraTransition {
    id: String,
    name: String,
    to: JiraTransitionTarget,
    #[serde(rename = "hasScreen")]
    has_screen: bool,
    #[serde(rename = "isAvailable")]
    is_available: bool,
}

#[derive(Deserialize)]
struct JiraTransitionTarget {
    name: String,
    #[serde(rename = "statusCategory")]
    status_category: JiraStatusCategory,
}

#[derive(Deserialize)]
struct JiraSearchResult {
    issues: Vec<JiraIssue>,
    total: u64,
}
```

#### Implementation steps

- [ ] **1.4.1** — Define all Jira deserialization structs above in `src/adapters/jira.rs`.
- [ ] **1.4.2** — Implement `jira_issue_to_record(issue: &JiraIssue, host: &str) -> BackendIssueRecord` that maps every field per the table above, including timestamp normalization and URL construction.
- [ ] **1.4.3** — Implement `upsert_to_jira_create(upsert: &BackendIssueUpsert, project_key: &str, issue_type: &str) -> serde_json::Value` that builds the `POST /rest/api/2/issue` JSON payload.
- [ ] **1.4.4** — Implement `upsert_to_jira_update(upsert: &BackendIssueUpsert) -> serde_json::Value` that builds the `PUT /rest/api/2/issue/{id}` JSON payload (excludes state — handled via transitions).
- [ ] **1.4.5** — Implement `normalize_jira_timestamp(jira_ts: &str) -> String` to convert Jira's `+0000` format to RFC 3339 `+00:00` using chrono.
- [ ] **1.4.6** — Update `JiraIssueMeta` (§3.1) to include `issue_key`, `assignee_account_id`, and `assignee_name` fields for round-trip fidelity.
- [ ] **1.4.7** — Add unit tests for `jira_issue_to_record` with a realistic JSON fixture (use the example response from the research), verifying every field maps correctly.
- [ ] **1.4.8** — Add unit tests for `upsert_to_jira_create` and `upsert_to_jira_update`, verifying the generated JSON payloads have correct structure.

---

## Phase 2: Backend Enum & Config

**Depends on**: Phase 0 (trait split), Phase 1 decisions

### 2.1 — Add `Jira` variant to `Backend` enum

**File**: `src/models/backend.rs`

```rust
pub enum Backend {
    Github,
    Gitlab,
    Jira,   // ← add
    Local,
}
```

This will trigger compiler errors everywhere `Backend` is matched exhaustively — that's the point. Follow the compiler to find every location that needs a Jira arm.

### 2.2 — Add `Jira` scope prefix in issue_ids

**File**: `src/services/issue_ids.rs`

In `derive_scope()`, add `Backend::Jira => "JR"` to the server match.

In the `(parent, project)` match, Jira follows the same `owner/repo` pattern but `repo` is the project key (e.g., `"myteam/PROJ"` or just `"PROJ"`). Decide:
- If `repo` = `"PROJ"` (no slash), use backend `name` as parent
- If `repo` = `"org/PROJ"`, split normally

Result: IDs like `JR-myteam-PROJ--42`.

### 2.3 — Config validation for Jira backends

**File**: `src/config.rs`

Add validation in `validate_config`:
- `host` is **required** for Jira backends (unlike GitHub which defaults to github.com)
- `repo` is **required** and should be a valid Jira project key (uppercase alphanumeric)
- Validate host URL format (must start with `https://`)
- If `vc` is set, validate the referenced backend exists and is GitHub or GitLab

### 2.4 — Update `hosted_backends` filter

**File**: `src/commands/sync_cmd.rs`

Rename to `syncable_backends` for clarity — these are backends with an `IssueTracker`:

```rust
fn syncable_backends(config: &Config) -> Vec<&BackendConfig> {
    config.backends.iter()
        .filter(|b| matches!(b.backend, Backend::Github | Backend::Gitlab | Backend::Jira))
        .collect()
}
```

### 2.5 — Update `resolve_git_auth`

**File**: `src/services/backend_mapping.rs`

Add `Backend::Jira => None` — Jira doesn't participate in git HTTP auth. Git auth comes from the VC layer, resolved separately.

---

## Phase 3: Domain Model Extensions

**Depends on**: Phase 1.4 (field mapping)

### 3.1 — Create `JiraIssueMeta` struct

**File**: `src/domain/issue.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JiraIssueMeta {
    pub project_key: String,              // e.g., "PROJ"
    pub issue_key: Option<String>,        // e.g., "PROJ-123"
    pub issue_id: Option<u64>,            // numeric ID from API
    pub url: Option<String>,
    pub updated_at: String,
    pub last_pushed_state: Option<IssueState>,
    pub issue_type: Option<String>,       // Bug, Task, Story, Epic
    pub assignee_account_id: Option<String>,  // Cloud: for push round-trip
    pub assignee_name: Option<String>,        // Server/DC: for push round-trip
}
```

### 3.2 — Add `jira` field to `IssueFrontmatter`

**File**: `src/domain/issue.rs`

Add `pub jira: Option<JiraIssueMeta>` to `IssueFrontmatter`, alongside `github` and `gitlab`.

### 3.3 — Update serde skip conditions

Ensure `jira` field is skipped when `None` in YAML serialization (follow the same `#[serde(skip_serializing_if = "Option::is_none")]` pattern).

---

## Phase 4: Jira API Client

**Depends on**: Phase 1.1 (library choice), Phase 1.2 (auth model)

### 4.1 — Create `JiraProvider` struct

**File**: `src/adapters/jira.rs` (new file)

```rust
pub struct JiraProvider {
    client: gouqi::Jira,  // or wrapper struct
    host: String,
}

impl JiraProvider {
    pub fn new(host: &str, auth: JiraAuth) -> Result<Self, RiptskError> { ... }
}
```

### 4.2 — Implement core API methods on `JiraProvider`

Implement thin wrappers around Jira REST API v2 (decided in §1.1):

- `search_issues(&self, jql: &str)` → `GET /rest/api/2/search`
- `get_issue(&self, issue_id_or_key: &str)` → `GET /rest/api/2/issue/{id}`
- `create_issue(&self, project_key: &str, ...)` → `POST /rest/api/2/issue`
- `update_issue(&self, issue_id_or_key: &str, ...)` → `PUT /rest/api/2/issue/{id}`
- `transition_issue(&self, issue_id_or_key: &str, transition_id: &str)` → `POST /rest/api/2/issue/{id}/transitions`
- `get_transitions(&self, issue_id_or_key: &str)` → `GET /rest/api/2/issue/{id}/transitions`
- `delete_issue(&self, issue_id_or_key: &str)` → `DELETE /rest/api/2/issue/{id}`

### 4.3 — Register module

**File**: `src/adapters/mod.rs`

Add `pub mod jira;`.

---

## Phase 5: IssueTracker Implementation for Jira

**Depends on**: Phase 0 (trait split), Phase 3, Phase 4

### 5.1 — Implement `IssueTracker` for `JiraProvider`

`JiraProvider` implements **only** `IssueTracker` — it does NOT implement `VersionControl`. No stubs, no errors, no dead code.

```rust
#[async_trait]
impl IssueTracker for JiraProvider {
    // All 10 issue methods — real implementations
}
```

### 5.2 — Issue method implementations

- **`list_issues`**: Use JQL `project = {project_key} ORDER BY updated DESC`. Map results to `Vec<BackendIssueRecord>`.
- **`get_issue`**: Fetch by numeric ID or key. Map to `BackendIssueRecord`.
- **`create_issue`**: POST new issue. Map `BackendIssueUpsert` → Jira create payload.
- **`update_issue`**: PUT update. Map `BackendIssueUpsert` → Jira update payload.
- **`close_issue`**: Find transition to `Done` category, execute it (per §1.3).
- **`reopen_issue`**: Find transition back to `new`/`indeterminate` category, execute it (per §1.3).
- **`delete_issue`**: Attempt `DELETE`. If forbidden (403), return `DeleteOutcome::SoftClosed`.
- **`lock_issue`**: Return `Ok(())` (no-op — Jira has no lock concept).
- **`unlock_issue`**: Return `Ok(())` (no-op).
- **`sync_labels`**: Use `PUT /rest/api/2/issue/{id}` with labels field.

---

## Phase 6: Backend Mapping & Credential Resolution

**Depends on**: Phase 0, Phase 1.2, Phase 5

### 6.1 — Implement credential resolution

**File**: `src/services/backend_mapping.rs`

```rust
fn resolve_jira_credentials(
    backend_name: &str,
    host: &str,
) -> Result<(JiraAuth, CredentialSource), RiptskError> {
    // 1. Check JIRA_API_TOKEN + JIRA_EMAIL (Cloud basic auth)
    // 2. Check JIRA_API_TOKEN alone (Server/DC PAT)
    // 3. Fallback: jira-cli-go config + keychain
    // 4. Error with helpful message
}
```

### 6.2 — Wire Jira into `build_issue_tracker`

**File**: `src/services/backend_mapping.rs`

Add `Backend::Jira` arm:

```rust
Backend::Jira => {
    let host = backend.host.as_deref()
        .ok_or_else(|| RiptskError::Config("Jira backend requires 'host'".into()))?;
    let (auth, source) = resolve_jira_credentials(&backend.name, host)?;
    crate::ui::info(&format!("auth: jira backend '{}' using {}", backend.name, source));
    Ok(Box::new(JiraProvider::new(host, auth)?))
}
```

`build_version_control()` for `Backend::Jira` returns an error: `"Jira is an issue tracker — use the 'vc' config field to link a GitHub/GitLab backend for PRs and branches"`.

### 6.3 — Update conversion functions

**File**: `src/services/backend_mapping.rs`

Update `backend_to_local()`:
- Add `Backend::Jira` arm that populates `frontmatter.jira = Some(JiraIssueMeta { ... })`

Update `update_issue_from_backend()`:
- Add `Backend::Jira` arm that updates the `jira` meta

Update `issue_to_upsert()`:
- Handle `jira` meta for `milestone_id` extraction (alongside github/gitlab)

### 6.4 — Update state mapping for Jira

Update `sanitize_state_reason()`:
- Jira resolutions: `"Done"`, `"Won't Do"`, `"Duplicate"`, `"Cannot Reproduce"`
- Map from riptsk `state_reason` values to Jira resolution names

---

## Phase 7: Command Integration

**Depends on**: Phase 0 (trait split), Phase 5, Phase 6

### 7.1 — Commands with `IssueTracker` only

These commands already work with Jira after the Phase 0 migration — they only need `&dyn IssueTracker`:

- `sync pull/push` — `src/services/sync_engine.rs`
- `tsk new` — `src/commands/issues.rs`

**No additional work needed** for Jira support in these commands.

### 7.2 — Commands with `VersionControl` only

These commands get their provider from the VC layer, which is always GitHub/GitLab. Jira never enters the picture:

- `pr show/edit/create` — `src/commands/pr.rs`
- `branch create/delete` — `src/commands/branch.rs`

**No additional work needed** — these already use `&dyn VersionControl`.

### 7.3 — Commands with both layers

These commands orchestrate across both layers:

**`tsk start`** (`src/commands/start.rs`):
```
start(issue_tracker, vc: Option<VersionControl>):
  1. Create issue via issue_tracker.create_issue()
  2. if vc.is_some():
       Create branch via vc.create_branch(issue_id)
       Create PR via vc.create_pr()
       Store pr_url/pr_number on local issue
  3. else:
       Done — issue-only workflow
```

**`tsk done`** (`src/commands/done.rs`):
```
done(issue_tracker, vc: Option<VersionControl>):
  1. if vc.is_some() and issue has pr_number:
       Merge PR via vc.merge_pr()
       Delete branch via vc.delete_branch()
  2. Check if issue auto-closed (for GitHub/GitLab issue trackers)
  3. If not auto-closed, close via issue_tracker.close_issue()
  4. Update local issue state
```

**`pr merge`** (the merge workflow in `src/commands/pr.rs`):
- Requires both `&dyn IssueTracker` + `&dyn VersionControl`
- After merging, checks issue state via issue tracker

### 7.4 — Improve error messages

Ensure Jira API errors are wrapped with context:
- Auth failures → "Jira authentication failed. Check JIRA_API_TOKEN and JIRA_EMAIL environment variables."
- Permission errors → "Jira permission denied for project {key}. Verify your API token has access."
- Not found → "Jira issue {key} not found in project {project}."

When a command needs VC but none is configured:
- `tsk start` → creates issue only, prints info: "No version control backend configured — skipping branch and PR creation."
- `tsk done` → closes issue only, prints info: "No version control backend configured — skipping PR merge."
- `tsk pr create` → error: "No version control backend configured for project '{name}'. Add 'vc: <github-or-gitlab-backend>' to the backend config."

---

## Phase 8: Testing

### 8.1 — Phase 0 regression tests

**Critical**: After the trait split, all existing tests must pass unchanged. Run `make check` at every step of Phase 0.

### 8.2 — Unit tests for field mapping

Test `BackendIssueRecord` ↔ Jira JSON conversion:
- Status category mapping
- Wiki markup string ↔ `BackendIssueRecord.body` (null handling, round-trip fidelity)
- Label handling
- Assignee mapping (single assignee vs multi)

### 8.3 — Unit tests for state transitions

Test transition logic:
- Finding the right transition ID for a target status category
- Handling projects with custom workflows
- Resolution setting on close

### 8.4 — Unit tests for credential resolution

Test the env var chain:
- `JIRA_API_TOKEN` + `JIRA_EMAIL` present → Cloud basic auth
- `JIRA_API_TOKEN` alone → PAT auth
- None present → appropriate error message

### 8.5 — Unit tests for VC resolution

Test `resolve_vc_for_backend`:
- Backend with `vc` field → resolves linked VC backend
- GitHub/GitLab backend without `vc` → uses itself as VC
- Jira backend without `vc` → returns `None`
- `vc` references nonexistent backend → config validation error
- `vc` references Jira or Local backend → config validation error

### 8.6 — Integration tests (if feasible)

If a test Jira instance is available:
- Create, read, update, close, reopen, delete issue
- Label sync
- Transition through workflow states
- Sync pull/push cycle
- Cross-backend workflow: Jira issue + GitHub PR via `vc` config

---

## Phase 9: Documentation & Config Examples

### 9.1 — Config examples

**Standalone Jira (issue tracking only)**:
```yaml
backends:
  - name: jira-myteam
    backend: jira
    host: https://myteam.atlassian.net
    repo: PROJ
    default_board: sprint
```

**Jira + GitHub (full workflow)**:
```yaml
backends:
  - name: github-repo
    backend: github
    repo: owner/repo

  - name: jira-myteam
    backend: jira
    host: https://myteam.atlassian.net
    repo: PROJ
    default_board: sprint
    vc: github-repo
```

**Jira + GitLab**:
```yaml
backends:
  - name: gitlab-project
    backend: gitlab
    host: https://gitlab.com
    repo: group/project

  - name: jira-myteam
    backend: jira
    host: https://myteam.atlassian.net
    repo: PROJ
    vc: gitlab-project
```

**GitHub standalone (unchanged)**:
```yaml
backends:
  - name: github-repo
    backend: github
    repo: owner/repo
```

### 9.2 — Document required environment variables

```
JIRA_API_TOKEN  — Jira Cloud API token (generate at https://id.atlassian.com/manage-profile/security/api-tokens)
JIRA_EMAIL      — Email associated with the Jira Cloud account (required with JIRA_API_TOKEN)
```

### 9.3 — Document the two-layer architecture

Explain that riptsk now separates:
- **Issue Tracking**: Where issues live (GitHub Issues, GitLab Issues, Jira)
- **Version Control**: Where PRs and branches live (GitHub, GitLab)

For GitHub/GitLab, one backend provides both layers (backward compatible). For Jira, use the `vc` field to link a GitHub/GitLab backend for PRs and branches.

### 9.4 — Document limitations

- Wiki markup ↔ markdown conversion is not performed — each backend's body format is stored as-is
- Custom fields (story points, etc.) require additional configuration (future)
- Jira transitions are workflow-dependent — state changes may fail if the workflow doesn't allow the transition
- Single assignee only (Jira limitation)

---

## File Change Summary

| File | Change Type | Phase | Description |
|---|---|---|---|
| `src/adapters/backend.rs` | Modify | 0 | Split into `IssueTracker` + `VersionControl` traits |
| `src/adapters/github.rs` | Modify | 0 | Implement both traits separately |
| `src/adapters/gitlab.rs` | Modify | 0 | Implement both traits separately |
| `src/adapters/jira.rs` | **New** | 4-5 | `JiraProvider` implementing `IssueTracker` only |
| `src/adapters/mod.rs` | Modify | 4 | Add `pub mod jira;` |
| `src/models/backend.rs` | Modify | 0, 2 | Add `vc` field to `BackendConfig`, add `Jira` to `Backend` enum |
| `src/services/backend_mapping.rs` | Modify | 0, 6 | Split builder, add credential resolution, add Jira conversions |
| `src/services/sync_engine.rs` | Modify | 0 | Accept `&dyn IssueTracker` |
| `src/services/issue_ids.rs` | Modify | 2 | Add `"JR"` scope prefix |
| `src/domain/issue.rs` | Modify | 3 | Add `JiraIssueMeta`, add `jira` field to `IssueFrontmatter` |
| `src/config.rs` | Modify | 2 | Jira config validation, `vc` reference validation |
| `src/commands/sync_cmd.rs` | Modify | 2 | Rename `hosted_backends` → `syncable_backends` |
| `src/commands/start.rs` | Modify | 0, 7 | Accept both layers, skip VC when `None` |
| `src/commands/done.rs` | Modify | 0, 7 | Accept both layers, skip VC when `None` |
| `src/commands/pr.rs` | Modify | 0 | Accept `&dyn VersionControl` |
| `src/commands/branch.rs` | Modify | 0 | Accept `&dyn VersionControl` |
| `src/commands/issues.rs` | Modify | 0 | Accept `&dyn IssueTracker` |
| `Cargo.toml` | Modify | 1 | Add `gouqi`, `keyring` |

---

## Dependency Chain

```
Phase 0 (Trait Split) ← MUST BE FIRST — no Jira code yet, pure refactor
  │
  ├── Phase 1 (Research) — can run in parallel with Phase 0
  │     ├── 1.1 Library ──→ Phase 4
  │     ├── 1.2 Auth ────→ Phase 6
  │     ├── 1.3 States ──→ Phase 5
  │     └── 1.4 Fields ──→ Phase 3, 5
  │
  ├── Phase 2 (Enum & Config) ← depends on Phase 0
  ├── Phase 3 (Domain Model) ← depends on 1.4
  ├── Phase 4 (API Client) ← depends on 1.1, 1.2
  ├── Phase 5 (IssueTracker impl) ← depends on 0, 3, 4
  ├── Phase 6 (Mapping & Creds) ← depends on 0, 1.2, 5
  ├── Phase 7 (Commands) ← depends on 0, 5, 6
  ├── Phase 8 (Testing) ← depends on all
  └── Phase 9 (Docs) ← depends on all
```

**Parallel tracks**:
- Phase 0 + Phase 1 research can run in parallel
- Phase 2 + Phase 3 can run in parallel (after Phase 0)
- Phase 4 can start once Phase 1 research is done
- Phase 7 is mostly done by Phase 0 — Jira-specific changes are minimal
