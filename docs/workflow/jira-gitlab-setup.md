# Setup: Jira (issues) + GitLab (vc)

Step-by-step guide to configure a riptsk project where **Jira** tracks issues and **GitLab** hosts branches, merge requests, and CI.

This is the common enterprise pattern: code review lives on GitLab, but ticketing, planning, and reporting live on Jira. Every branch and MR title carries a Jira key (e.g. `PROJ-123`) so Jira's Development panel auto-populates.

## Prerequisites

- `tsk` installed — see [Installation](../../README.md#installation).
- A GitLab project you can push to (self-hosted or gitlab.com).
- A Jira Cloud or Server/DC instance with a project key (e.g. `PROJ`) and permission to create and transition issues.
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
tsk init
```

This creates `$RIPTSK_REPO` (defaults to `$XDG_DATA_HOME/riptsk`) with `issues/`, `templates/`, `riptsk.yaml`, and a git repo.

## 4. Register the GitLab backend

From the project directory (the one with the GitLab git remote):

```bash
cd /path/to/your/project
tsk register
```

`tsk register` detects the `gitlab` remote and adds an entry to `riptsk.yaml`. Confirm with:

```bash
tsk register --list
```

## 5. Add the Jira backend manually

Jira is **not** auto-detected from git remotes — it must be configured by hand. Open `riptsk.yaml`:

```bash
tsk config edit   # or: $EDITOR "$RIPTSK_REPO/riptsk.yaml"
```

Add a Jira backend that points its `vc` field at the GitLab backend you just registered:

```yaml
backends:
  - name: my-gitlab
    type: gitlab
    host: https://gitlab.com          # or your self-hosted host
    repo: team/my-app                  # group/project

  - name: my-jira
    type: jira
    host: https://mycompany.atlassian.net
    repo: mycompany/PROJ               # org / PROJECT_KEY
    default_issue_type: Task           # Task, Story, Bug, etc.
    vc: my-gitlab                      # delegate branches and MRs here
```

Field notes:

- `host` is required for Jira and must start with `https://`.
- `repo` for Jira is `org/PROJECT_KEY` — the key is what appears in issue IDs like `PROJ-123`.
- `default_issue_type` must match an issue type that exists in the Jira project. Check in Jira under **Project settings → Issue types**.
- `vc: my-gitlab` is the critical line — without it, `tsk start`, `tsk branch`, and `tsk pr` will skip branch/MR creation for Jira issues.

## 6. Point the project directory at Jira

`tsk register` bound the cwd to the GitLab backend. To make `tsk` use Jira for new issues from this project, re-register with an explicit backend:

```bash
tsk register --backend my-jira
tsk register --list
```

The same directory can be registered against multiple backends; scope commands with `-p my-jira` / `-p my-gitlab` or use `--all` when needed.

## 7. Smoke test: create and sync

```bash
tsk new -p my-jira --title "Wire up deploy pipeline"
tsk sync push -p my-jira
```

Open the issue in Jira. It should exist under project `PROJ` with type `Task`. The returned `PROJ-N` key becomes the canonical ID.

Pull from Jira to populate additional issues:

```bash
tsk sync pull -p my-jira
tsk ls -p my-jira
```

## 8. Full loop: start → MR → done

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

`tsk done` with a Jira backend:

- Merges the MR on GitLab (via `vc: my-gitlab`).
- Transitions the Jira issue to its **Done** status and sets resolution to `Done`. `--reason not_planned` → `Won't Do`; `--reason duplicate` → `Duplicate`.

## 9. Verify the Jira ↔ GitLab link

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
| `tsk start` says "No version control backend configured" | `vc:` missing on the Jira backend, or the referenced backend name doesn't exist. |
| `tsk new -p my-jira` fails with 401 | `JIRA_API_TOKEN` / `JIRA_EMAIL` wrong, or Server/DC instance needs `JIRA_AUTH_TYPE=bearer`. |
| `tsk sync push` fails with "issue type does not exist" | `default_issue_type` doesn't match a type in the Jira project. Check **Project settings → Issue types**. |
| MR is created but Jira shows nothing in Development panel | GitLab ↔ Jira app not installed — see step 9. The key is in the MR title regardless. |
| `tsk done` merges MR but leaves Jira issue open | The Jira workflow has no transition named `Done` reachable from the current status. Ask the Jira admin to expose a `Done`-category transition, or transition manually. |
| `PROJ-123` not recognized as a Jira issue key | `repo` field isn't `org/PROJECT_KEY`, or `key` override is needed — see the `key` field in the main [README](../../README.md#manual-backend-configuration). |

## Rollout tips for teams

- **Branch names carry the whole integration.** Train everyone to either use `tsk start` or include `PROJ-123` in every manually-named branch. No key, no link.
- **Standardize `default_issue_type`** per backend — splitting planning across Story/Task/Bug works better when the default matches team convention.
- **Scope boards** with `-p my-jira` so personal `local` boards don't bleed into team planning.
- **Don't fight Jira workflows.** Status and resolution are separate fields with admin-controlled transitions. If `tsk done` can't close an issue, the workflow needs adjustment at the Jira level, not in `tsk`.

## Related

- [Backend Sync](backend-sync.md) — credentials, config, and `tsk sync` in depth
- [Branch → PR → Done](branch-pr-done.md) — the full remote workflow
- [Feature Lifecycle](feature-lifecycle.md) — riptsk contributor workflow (for reference)
- [Branch Strategy](branch-strategy.md) — riptsk contributor workflow (for reference)
