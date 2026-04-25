# Setup: Jira (issues) + GitLab (vc)

Step-by-step guide to configure a riptask RepoProject where **Jira** is the TasksBackend and **GitLab** is the VCBackend for branches, merge requests, and CI.

This is the common enterprise pattern: code review lives on GitLab, but ticketing, planning, and reporting live on Jira. Every branch and MR title carries a Jira key (e.g. `PROJ-123`) so Jira's Development panel auto-populates.

## Prerequisites

- `tsk` installed — see [Installation](../../README.md#installation).
- A GitLab RepoProject you can push to (self-hosted or gitlab.com).
- A Jira Cloud or Server/DC instance with a Jira project key (e.g. `PROJ`) and permission to create and transition issues.
- Git remote for the project already pointing at the GitLab repo.

## 1. Generate credentials

### GitLab token

Create a personal access token with `api` and `write_repository` scopes:

- gitlab.com: **User Settings → Access Tokens**
- Self-hosted: `https://<your-gitlab-host>/-/user_settings/personal_access_tokens`

### Jira token

- **Jira Cloud**: <https://id.atlassian.com/manage-profile/security/api-tokens> — create an API token tied to your Atlassian account email.
- **Jira Server / Data Center**: create a Personal Access Token at **Profile → Personal Access Tokens**.

## 2. Export credentials

Add to your shell profile (`~/.bashrc`, `~/.zshrc`, etc.):

```bash
# GitLab
export GITLAB_TOKEN="glpat-..."

# Jira Cloud
export JIRA_EMAIL="you@company.com"
export JIRA_API_TOKEN="..."

# Jira Server / DC — use the PAT and switch auth mode to bearer
# export JIRA_API_TOKEN="..."
# export JIRA_AUTH_TYPE=bearer
```

Reload the shell, then confirm they are set:

```bash
echo "$GITLAB_TOKEN" && echo "$JIRA_API_TOKEN"
```

Token lookup order and alternatives (e.g. `glab auth`, `jira-cli-go` keychain) are documented in the main [README](../../README.md#backend-authentication).

## 3. Initialize the task store (first time only)

If you have never run `tsk`, initialize the local store:

```bash
tsk init --system
```

This creates `$RIPTASK_REPO` (defaults to `$XDG_DATA_HOME/riptask`) with `issues/`, `templates/`, `config.yaml`, and a git repo.

## 4. Register the GitLab RepoProject

From the project directory (the one with the GitLab git remote):

```bash
cd /path/to/your/project
tsk register
```

`tsk register` detects the `gitlab` remote and adds a RepoProject entry to the selected config layer. Use `tsk register --user` or `tsk register --local` to choose explicitly. Confirm with:

```bash
tsk register --list
```

## 5. Configure the RepoProject

Jira is not auto-detected from git remotes. The supported config shape is:

```bash
tsk config edit   # or: $EDITOR "$RIPTASK_REPO/config.yaml"
```

```yaml
projects:
  - name: my-jira
    vc_backend:
      type: gitlab
      host: https://gitlab.com
      repo: team/my-app
      path: /path/to/your/project
    tasks_backend:
      type: jira
      host: https://mycompany.atlassian.net
      jira_project: mycompany/PROJ
      default_issue_type: Task
    default_board: personal
    repo_project_label: proj::my-app
```

Field notes:

- `tasks_backend.host` is required for Jira and must start with `https://`.
- `tasks_backend.jira_project` is `org/PROJECT_KEY`.
- `default_issue_type` must match an issue type that exists in the Jira project. Check in Jira under **Project settings → Issue types**.
- The VCBackend/TasksBackend split is the critical concept. Without a non-local VCBackend, `tsk start`, `tsk branch`, and `tsk pr` skip branch/MR creation for Jira issues.

## 6. Register the project directory

From the project directory:

```bash
cd /path/to/your/project
tsk register
```

`tsk register` detects the GitLab remote, sets the RepoProject name, stores the local path, and derives `repo_project_label` from the origin URL tail by default — always as `proj::<sanitized-tail>`. To override that label during registration:

```bash
tsk register --repo-project-label platform-api
# stored as proj::platform-api

tsk register --repo-project-label proj::platform-api
# same result — passing the prefix explicitly is accepted
```

`--project-label` is accepted as an alias. Labels stored in `config.yaml` always carry the `proj::` prefix.

## 7. Shared Jira project (label-partitioned)

Multiple RepoProjects can share one Jira project when each RepoProject has a distinct `repo_project_label`.

```yaml
projects:
  - name: billing-api
    vc_backend: { type: gitlab, host: https://gitlab.com, repo: team/billing-api, path: /src/billing-api }
    tasks_backend: { type: jira, host: https://jira.example.com, jira_project: company/PLAT, default_issue_type: Task }
    repo_project_label: proj::billing-api

  - name: auth-service
    vc_backend: { type: gitlab, host: https://gitlab.com, repo: team/auth-service, path: /src/auth-service }
    tasks_backend: { type: jira, host: https://jira.example.com, jira_project: company/PLAT, default_issue_type: Task }
    repo_project_label: proj::auth-service

  - name: web-console
    vc_backend: { type: gitlab, host: https://gitlab.com, repo: team/web-console, path: /src/web-console }
    tasks_backend: { type: jira, host: https://jira.example.com, jira_project: company/PLAT, default_issue_type: Task }
    repo_project_label: proj::web-console
```

Typical flow:

```bash
cd /src/billing-api && tsk register
cd /src/auth-service && tsk register
cd /src/web-console && tsk register --repo-project-label frontend-console
# stored as proj::frontend-console
```

Behavior:

- Pull uses JQL `project = PLAT AND labels = "proj::<suffix>"`.
- Push injects `repo_project_label` (e.g. `proj::billing-api`) into Jira labels.
- Pull strips that partition label back out of local issue labels so it does not become user-managed metadata.
- Smart Commits and the Jira Development panel continue to key off the Jira issue key (`PLAT-123`), not the label.

## 8. Smoke test: create and sync

```bash
tsk new -p my-jira --title "Wire up deploy pipeline"
tsk sync push -p my-jira
```

Open the issue in Jira. It should exist under Jira project `PROJ` with type `Task`. The returned `PROJ-N` key becomes the canonical ID.

Pull from Jira to populate additional issues:

```bash
tsk sync pull -p my-jira
tsk ls -p my-jira
```

## 9. Full loop: start → MR → done

From the project directory:

```bash
# Create a Jira issue and open a GitLab MR in one step
tsk start -p my-jira --title "Refactor auth middleware"
```

What happens:

1. A Jira issue is created (e.g. `PROJ-214`).
2. A branch `PROJ-214-refactor-auth-middleware` is cut from the project's default branch on GitLab.
3. A draft MR is opened on GitLab. Title and body include the Jira key — `PROJ-214: Refactor auth middleware` — so Jira's Development panel links the MR automatically.

Commit, push, and iterate as usual, then:

```bash
tsk pr edit                    # tweak MR title/body
tsk pr                         # view the MR URL
tsk done                       # merge the MR and transition the Jira issue to Done
```

`tsk done` with a Jira TasksBackend:

- Merges the MR on GitLab (via the RepoProject's VCBackend).
- Transitions the Jira issue to its **Done** status and sets resolution to `Done`. `--reason not_planned` → `Won't Do`; `--reason duplicate` → `Duplicate`.

## 10. Verify the Jira ↔ GitLab link

On the Jira issue page, open the **Development** panel (right sidebar). You should see:

- The branch `PROJ-214-refactor-auth-middleware`
- Commits containing `PROJ-214` in the message
- The MR with its current state (open, merged, closed)

If this panel is empty, the GitLab ↔ Jira integration app is not installed on your Jira instance. Fix it once per Jira project:

- Jira Cloud: **Apps → Explore more apps → search "GitLab for Jira"** → install → connect your GitLab group.
- GitLab side: **Project → Settings → Integrations → Jira** → fill host, email (Cloud) or username (Server), token, and enable "Enable Jira issue references" + "Enable Jira issues transitions".

`tsk` already does its part by stamping `PROJ-xxx` into every branch, commit-adjacent context, and MR title — but the Jira Development panel only populates if the integration app is installed.

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `tsk start` says "No version control backend configured" | The RepoProject's VCBackend is local or otherwise not configured for remote branch/PR work. |
| `tsk new -p my-jira` fails with 401 | `JIRA_API_TOKEN` / `JIRA_EMAIL` wrong, or Server/DC instance needs `JIRA_AUTH_TYPE=bearer`. |
| `tsk sync push` fails with "issue type does not exist" | `default_issue_type` doesn't match a type in the Jira project. Check **Project settings → Issue types**. |
| MR is created but Jira shows nothing in Development panel | GitLab ↔ Jira app not installed — see step 9. The key is in the MR title regardless. |
| `tsk done` merges MR but leaves Jira issue open | The Jira workflow has no transition named `Done` reachable from the current status. Ask the Jira admin to expose a `Done`-category transition, or transition manually. |
| `PROJ-123` not recognized as a Jira issue key | The JiraProject field is wrong, or a RepoProject key override is needed — see the configuration docs in the main [README](../../README.md#configuration). |

## Rollout tips for teams

- **Branch names carry the whole integration.** Train everyone to either use `tsk start` or include `PROJ-123` in every manually-named branch. No key, no link.
- **Standardize `default_issue_type`** per backend — splitting planning across Story/Task/Bug works better when the default matches team convention.
- **Scope boards** with `-p my-jira` so personal `local` boards don't bleed into team planning.
- **Don't fight Jira workflows.** Status and resolution are separate fields with admin-controlled transitions. If `tsk done` can't close an issue, the workflow needs adjustment at the Jira level, not in `tsk`.

## Related

- [Backend Sync](backend-sync.md) — credentials, config, and `tsk sync` in depth
- [Branch → PR → Done](branch-pr-done.md) — the full remote workflow
- [Feature Lifecycle](feature-lifecycle.md) — riptask contributor workflow (for reference)
- [Branch Strategy](branch-strategy.md) — riptask contributor workflow (for reference)
