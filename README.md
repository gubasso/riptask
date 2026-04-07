# riptsk

A plaintext issue tracker for the terminal.

- Markdown issues with YAML frontmatter in a local git-backed task store
- Optional sync with GitHub, GitLab, and Jira
- AI features are available, but never required

## Table of contents

- [Quick start](#quick-start)
- [Architecture](#architecture)
- [Installation](#installation)
- [Core concepts](#core-concepts)
- [Issue lifecycle](#issue-lifecycle)
- [Branch → PR → Done workflow](#branch--pr--done-workflow)
- [Sync with GitHub, GitLab, Jira](#sync-with-github-gitlab-jira)
- [Templates](#templates)
- [Recurring tasks](#recurring-tasks)
- [Sessions](#sessions)
- [AI integration](#ai-integration)
- [Configuration](#configuration)
- [Issue format](#issue-format)
- [Task store maintenance](#task-store-maintenance)
- [Dependencies](#dependencies)
- [Development](#development)

## Quick start

```bash
make install

tsk init
tsk new --title "My first issue"
tsk ls
tsk board
```

The default board is `personal`. New issues default to status `backlog`.

## Architecture

High-level architecture:

```mermaid
flowchart LR
    User[User terminal] --> CLI[tsk CLI]
    CLI --> Repo[RIPTSK_REPO<br/>issues, templates, riptsk.yaml, .git]
    CLI --> Cache[XDG_CACHE_HOME riptsk<br/>views, id_map, backend_state, session]
    CLI --> ProjRepo[Project repo<br/>registered backend cwd]
    CLI --> Sync[Sync engine]
    Sync --> GitHub[GitHub]
    Sync --> GitLab[GitLab]
    Sync --> Jira[Jira]
    ProjRepo --> BranchPR[branch, pr, done]
    BranchPR --> GitHub
    BranchPR --> GitLab
    Jira -. vc points to gh or glab .-> BranchPR
```

Sync data flow:

```mermaid
flowchart TD
    Remote[GitHub GitLab Jira] -->|tsk sync pull| Mapper[backend mapping]
    Mapper --> Local[issues ID.md frontmatter and body]
    Local -->|tsk sync push| Mapper2[backend mapping]
    Mapper2 --> Remote
    Local --> Conflict{conflict}
    Conflict -->|yes| Markers[ID.md with markers]
    Conflict -->|yes| Backups[ID.LOCAL.md and ID.REMOTE.md]
    Markers -->|tsk sync resolve| Local
```

Issue lifecycle:

```mermaid
flowchart LR
    New[tsk new] --> Backlog[backlog]
    Backlog --> Todo[todo]
    Todo --> Branch[tsk branch]
    Branch --> InProgress[tsk pr<br/>status in progress]
    InProgress --> Merge[tsk pr merge]
    Merge --> Done[tsk done<br/>status done]
```

AI command pipeline:

```mermaid
flowchart LR
    Cmd[tsk new --ai, pr, pr edit, summarize, ask, commit] --> Gate{command opt-in or ai enabled, plus feature flag}
    Gate -->|yes| Tmpl[ai.command template]
    Tmpl --> Subst[substitute system, input or input_file]
    Subst --> Shell[sh -c]
    Shell --> External[external AI CLI]
    External --> Stdout[stdout into tsk]
```

XDG and task store layout:

```mermaid
flowchart TD
    XDG[XDG env] --> Repo[RIPTSK_REPO<br/>default XDG_DATA_HOME riptsk]
    XDG --> Conf[XDG_CONFIG_HOME riptsk config.env]
    XDG --> Cache[XDG_CACHE_HOME riptsk]
    Repo --> R1[issues]
    Repo --> R2[templates]
    Repo --> R3[riptsk.yaml]
    Repo --> R4[.git]
    Cache --> C1[views]
    Cache --> C2[id_map.json]
    Cache --> C3[backend_state.json]
    Cache --> C4[session.json]
```

## Installation

```bash
make install

# equivalent
cargo install --path .
```

After installing, run `tsk init` before using the task store.

Shell completions:

```bash
tsk completions bash > ~/.local/share/bash-completion/completions/tsk
tsk completions zsh > "${fpath[1]}/_tsk"
tsk completions fish > ~/.config/fish/completions/tsk.fish
```

## Core concepts

### Task store vs project repos

`tsk` keeps issues, templates, and `riptsk.yaml` in one local task store repository. By default that repository lives at `$XDG_DATA_HOME/riptsk`, or `~/.local/share/riptsk` when `XDG_DATA_HOME` is unset.

Project repositories are separate. Commands like `tsk branch`, `tsk pr`, `tsk done`, `tsk clone`, and `tsk unclone` operate in a project repo, while issue files and task-store commits live under `$RIPTSK_REPO`.

### Directory layout

`tsk` resolves paths like this:

| Location | Default | Contents |
|---|---|---|
| `$RIPTSK_REPO` | `$XDG_DATA_HOME/riptsk` | `issues/`, `templates/`, `riptsk.yaml`, `.git/` |
| `$XDG_CONFIG_HOME/riptsk/config.env` | `~/.config/riptsk/config.env` | Optional `RIPTSK_REPO=...` override |
| `$XDG_CACHE_HOME/riptsk` | `~/.cache/riptsk` | `views/`, `backend_state.json`, `id_map.json`, `deleted_keys.json`, `session.json` |

`tsk init` creates the task store, seeds built-in templates, writes `riptsk.yaml`, and initializes a git repo there.

## Issue lifecycle

Create, inspect, and manage issues:

```bash
tsk new
tsk new --title "Fix login bug"
tsk new --template bug --priority high
tsk new --title "Write spec" --edit
tsk new --title "Refactor auth" --ai

tsk ls
tsk ls --status backlog
tsk ls --priority high --project my-app
tsk ls --all
tsk ls --conflicts

tsk show <ID>
tsk edit <ID>
tsk status <ID> in-progress
tsk close <ID>
tsk reopen <ID>
tsk rm <ID>
tsk path <ID>
```

Notes:

- `tsk ls` defaults to open issues unless `--all` is used.
- `tsk show` prints the raw issue file. If a file contains unresolved sync markers, `tsk` warns but still prints it.
- When `fzf` is installed, commands that take an issue ID can often pick interactively with `--pick` or by omitting the ID where supported.

### Kanban board

The default board is `personal`, with lanes:

- `backlog`
- `todo`
- `in-progress`
- `done`

Board and ordering commands:

```bash
tsk board
tsk board --board personal
tsk board --all
tsk board --open
tsk board --path

tsk view

tsk reorder backlog
tsk reorder-up <ID>
tsk reorder-down <ID>
```

Custom boards and lane sets are defined in `riptsk.yaml` under `boards`.

### Work-clones

For per-issue working directories:

```bash
tsk clone <ID>
tsk unclone
tsk unclone --force
```

`tsk clone` creates a sibling work-clone for an issue branch. `tsk unclone` pushes the current work-clone, removes that clone directory, and prints the main repo path to return to.

## Branch → PR → Done workflow

The main remote workflow is issue first, then branch, then PR:

```bash
tsk new --title "Fix login bug"
tsk branch <ID>
# work in the project repo
tsk pr
tsk done <ID>
```

### `tsk branch`

Run `tsk branch` from the project repository, not from `$RIPTSK_REPO`.

```bash
tsk branch <ID>
tsk branch <ID> -d
tsk branch <ID> -D
tsk branch <ID> -D --yes
```

Creating a branch:

- resolves the issue
- creates or reuses the remote branch for the issue
- checks out the local branch
- stores both `branch` and `id-slug` in the issue frontmatter

Deleting a branch with `-d` or `-D` clears `branch` and `id-slug` from the issue.

### `tsk pr`

```bash
tsk pr
tsk pr <ID>
tsk pr create <ID>
tsk pr edit <ID>
tsk pr edit <ID> --no-ai
tsk pr edit <ID> -y
tsk pr show <ID>
tsk pr show <ID> --json
tsk pr merge <ID>
```

`tsk pr` without a subcommand behaves like `tsk pr create`.

Verified behavior:

- the current branch must match the issue branch
- creating a PR records `pr_url` and `pr_number`
- if the issue was `backlog` or `todo`, `tsk pr` moves it to `in-progress`
- Jira-backed issues use the Jira issue key in the PR title/body when Jira metadata exists

### `tsk done`

`tsk done` is the convenience wrapper for the full finish flow:

```bash
tsk done <ID>
tsk done <ID> --merge-method squash
tsk done <ID> --auto-merge
tsk done <ID> --timeout 900
tsk done <ID> --force-push
tsk done <ID> --yes
```

When a version-control backend is available and the issue has a branch, `tsk done`:

1. merges the PR
2. checks out the default branch and pulls
3. deletes the remote branch
4. deletes the local branch
5. clears `branch` and `id-slug`
6. marks the issue `done`
7. syncs the issue back to its backend
8. regenerates cached views

When the issue is local-only or Jira-without-`vc`, `tsk done` skips PR merge, marks the issue `done`, syncs issue state if applicable, and regenerates views.

Manual equivalent:

```bash
tsk pr show <ID>
tsk pr merge <ID> --merge-method squash --timeout 900
git checkout <default-branch>
git pull
tsk branch <ID> -D
tsk close <ID>
```

### `tsk start`

`tsk start` wraps issue creation or selection, then runs branch + PR creation when a version-control backend is available.

```bash
tsk start --title "Implement parser"
tsk start 42
tsk start --pick
tsk start --title "Refactor auth" --template feature --priority high
```

## Sync with GitHub, GitLab, Jira

Backends are configured in `riptsk.yaml` under `backends`.

### Backend authentication

GitHub token lookup order:

1. `GITHUB_TOKEN`
2. `GH_TOKEN`
3. `gh auth token`

GitLab token lookup order:

1. `GITLAB_TOKEN`
2. `glab auth status --show-token`

Jira authentication:

- Jira Cloud: `JIRA_API_TOKEN` plus `JIRA_EMAIL`
- Jira Server or Data Center: `JIRA_API_TOKEN`
- Optional override: `JIRA_AUTH_TYPE=bearer`
- Fallback: `jira-cli-go` config and keychain

Examples:

```bash
export GITHUB_TOKEN="ghp_..."
export GITLAB_TOKEN="glpat-..."

export JIRA_EMAIL="you@example.com"
export JIRA_API_TOKEN="..."
export JIRA_AUTH_TYPE=bearer
```

### Auto-detect with `tsk register`

From a project directory:

```bash
tsk register
tsk register --list
```

Detection rules:

- `github.com` remote -> `github`
- any host containing `gitlab` -> `gitlab`
- local git repo with no supported remote -> `local`
- non-git directory -> `local`

Jira is not auto-detected from git remotes. Add Jira backends manually in `riptsk.yaml`.

`tsk` also performs best-effort project auto-registration on startup when the current directory is not already registered.

### Manual backend configuration

```yaml
backends:
  - name: my-github
    type: github
    repo: myorg/my-app

  - name: my-gitlab
    type: gitlab
    host: https://gitlab.internal.example
    repo: team/my-app

  - name: my-jira
    type: jira
    host: https://myteam.atlassian.net
    repo: myorg/PROJ
    default_issue_type: Task
    vc: my-github

  - name: side-project
    type: local
    path: /home/user/projects/side-project
```

Field notes:

- `type` is one of `github`, `gitlab`, `jira`, `local`
- `repo` format is `owner repo` style by backend convention:
  GitHub uses `owner/repo`, GitLab uses `group/project`, Jira uses `org/PROJECT_KEY`
- Jira `host` is required and must start with `https://`
- Jira `vc` may point to a GitHub or GitLab backend for branch and PR commands
- `path` is used for local path-based project detection

### Sync commands

```bash
tsk sync
tsk sync pull
tsk sync pull 42
tsk sync push
tsk sync push 42
tsk sync status
tsk sync resolve <ID>
tsk sync resolve <ID> --take-local
tsk sync resolve <ID> --take-remote
```

`tsk sync` without a subcommand runs pull, then push.

Project selection can be scoped with the standard project flags where supported:

```bash
tsk sync -p my-github
tsk sync --backend my-gitlab
tsk done -p my-github <ID>
tsk done -a <ID>
```

### Conflict resolution

On pull conflicts, `tsk` writes:

- `issues/<ID>.md` with conflict markers
- `issues/<ID>.LOCAL.md`
- `issues/<ID>.REMOTE.md`

Then resolve with:

```bash
tsk edit <ID>
tsk sync resolve <ID>
```

Or restore one side directly:

```bash
tsk sync resolve <ID> --take-local
tsk sync resolve <ID> --take-remote
```

## Templates

Template commands:

```bash
tsk template list
tsk template show bug
tsk template new bugfix
tsk template edit bug
tsk template rm bug
tsk template validate bug
tsk template validate
```

Built-in templates:

- `task`
- `bug`
- `feature`
- `weekly-review`

Notes:

- `show`, `new`, `edit`, and `rm` require a template name
- `validate` accepts an optional template name; without one it validates all templates

## Recurring tasks

Recurring definitions live in `riptsk.yaml` under `recurring`.

```bash
tsk recur list
tsk recur run
tsk recur run 2026-04-01
tsk recur skip weekly-review

tsk recur new \
  --id weekly-review \
  --title-pattern "Weekly review %Y-%m-%d" \
  --frequency weekly \
  --template weekly-review \
  --day-of-week friday \
  --board personal \
  --status backlog
```

Supported frequencies:

- `daily`
- `weekly`
- `monthly`
- `yearly`

Notes:

- `--id`, `--title-pattern`, and `--frequency` are required
- weekly recurrences require `--day-of-week`
- monthly recurrences require `--day-of-month`
- `run` creates due issues and updates `last_run`

## Sessions

Session commands:

```bash
tsk session start <ID>
tsk session end
```

`tsk session start`:

- records the current branch in cache
- resolves the issue
- creates or checks out the session branch
- stores session state in `session.json`

`tsk session end`:

- commits the task store if `$RIPTSK_REPO` has uncommitted changes
- checks out the previous project branch
- clears the saved session state

It does not run sync, pull, or push automation by itself.

## AI integration

`tsk` is AI-provider agnostic. You configure one shell command template in `riptsk.yaml`, and `tsk` renders it with context before executing it with `sh -c`.

### Setup

```yaml
ai:
  enabled: true
  command: "cat {{input_file}} | claude -p --model haiku --system-prompt {{system}}"
```

Or with `config set`:

```bash
tsk config set ai.enabled true
tsk config set ai.command "cat {{input_file}} | claude -p --model haiku --system-prompt {{system}}"
```

### Placeholders

| Placeholder | Meaning |
|---|---|
| `{{system}}` | Shell-escaped system prompt |
| `{{input}}` | Shell-escaped input content |
| `{{input_file}}` | Path to a temp file containing the raw input |

`ai.command` must contain either `{{input}}` or `{{input_file}}`.

### Feature toggles

These live in YAML under `ai.features`:

```yaml
ai:
  enabled: true
  command: "..."
  features:
    new_body_gen: true
    triage: true
    summarize: true
    ask: true
    commit: true
```

`ai.features.*` are YAML-only settings. They are not supported by `tsk config set`.

### Command coverage

Current AI-gated commands:

- `tsk new --ai`
- `tsk pr`
- `tsk pr edit`
- `tsk summarize`
- `tsk ask`
- `tsk commit`

`tsk new` is explicit opt-in. It only runs AI when you pass `--ai`; `config.ai.enabled`
does not turn AI on by default for `tsk new`. Other AI-enabled commands still follow
their normal config-driven defaults.

### Example configurations

Claude:

```yaml
ai:
  command: "cat {{input_file}} | claude -p --model haiku --system-prompt {{system}}"
```

llm:

```yaml
ai:
  command: "cat {{input_file}} | llm -s {{system}}"
```

Ollama:

```yaml
ai:
  command: "cat {{input_file}} | ollama run llama3 --system {{system}}"
```

Inline prompt style:

```yaml
ai:
  command: "my-ai-cli --system {{system}} --prompt {{input}}"
```

### Usage

```bash
tsk new --title "Refactor auth" --ai
tsk pr
tsk pr edit <ID>
tsk pr edit <ID> --no-ai
tsk summarize
tsk summarize --project my-app
tsk summarize --board personal
tsk summarize --cycle 2026-Q1
tsk ask "What are the highest priority bugs?"

tsk commit
tsk commit --no-ai
tsk commit --edit
tsk commit --message "docs(readme): refresh architecture docs"
```

## Configuration

The main config file is `$RIPTSK_REPO/riptsk.yaml`.

Example:

```yaml
version: 1
auto_commit: false

defaults:
  board: personal
  status: backlog
  priority: medium
  assignee: null
  template: task

backends:
  - name: my-github
    type: github
    repo: myorg/my-app
    default_board: personal

boards:
  - name: personal
    statuses: [backlog, todo, in-progress, done]

ui:
  opener: "nvim -R"
  tree_depth: 2
  fzf_opts: "--border"

ai:
  enabled: false
  command: null
  features:
    new_body_gen: true
    triage: true
    summarize: true
    ask: true
    commit: true

sync:
  conflict_detection: true

recurring: []
```

### Settable via `tsk config set`

Exactly these keys are supported:

- `auto_commit`
- `defaults.board`
- `defaults.status`
- `defaults.priority`
- `defaults.assignee`
- `defaults.template`
- `ui.opener`
- `ui.tree_depth`
- `ui.fzf_opts`
- `ai.enabled`
- `ai.command`
- `sync.conflict_detection`

Everything else is YAML-only, including:

- `backends`
- `boards`
- `recurring`
- `ai.features.*`

## Issue format

Issues are Markdown files in `$RIPTSK_REPO/issues/`.

Example:

```markdown
---
id: WHL-042
title: Fix wormhole stabilizer retry logic
status: in-progress
board: personal
project: wormhole-router
priority: high
labels: [bug, temporal-drift]
assignees: [ppuffin]
cycle: 2026-Q1
order: 1
due: 2026-03-20
id-slug: 42-fix-wormhole-stabilizer-retry-logic
branch: 42-fix-wormhole-stabilizer-retry-logic
pr_url: https://github.com/myorg/wormhole-router/pull/128
pr_number: 128
local_updated_at: "2026-04-07T10:15:00Z"
github:
  repo: myorg/wormhole-router
  issue_id: 42
  updated_at: "2026-04-07T10:15:00Z"
---

## Description

The stabilizer retry logic does not back off correctly...
```

Frontmatter fields in the current schema include:

- identity and workflow: `id`, `title`, `status`, `board`, `project`, `org`, `order`
- planning: `priority`, `labels`, `assignees`, `milestone`, `cycle`, `due`, `weight`, `recurring`
- sync state: `state_reason`, `local_updated_at`, `remote_deleted`
- branch and PR state: `id-slug`, `branch`, `pr_url`, `pr_number`
- backend metadata blocks: `github`, `gitlab`, `jira`
- backend-specific fields: `confidential`, `discussion_locked`, `issue_type`, `locked`, `lock_reason`

Legacy `assignee: <name>` is still accepted when reading older issue files, but new docs and examples should use `assignees`.

## Task store maintenance

The task store is the riptsk repository itself, not your project repository.

```bash
tsk store commit
tsk store commit --edit

tsk store hooks install
tsk store hooks status
tsk store hooks update
tsk store hooks uninstall
```

`tsk store commit` stages `issues/`, `templates/`, and `riptsk.yaml`, then creates a task-store commit. Hook management installs the bundled pre-commit hook into `$RIPTSK_REPO/.git/hooks/pre-commit`.

## Dependencies

### Required

- `git`
- Rust toolchain for building from source

### Optional

| Tool | Used for |
|---|---|
| `fzf` | Interactive issue selection and reorder flows |
| Any AI CLI | `ai.command` integrations |
| `bash` | Managed task-store pre-commit hook execution |
| `yq` and `jq` | Managed task-store hook validation |

## Development

```bash
make lint
make test
make check
```
