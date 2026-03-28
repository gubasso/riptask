# riptsk

A plaintext, git-backed issue tracker that lives in your terminal.

Issues are Markdown files with YAML frontmatter, stored in a local Git
repository and optionally synced with GitHub or GitLab. Everything works
offline; AI features are available but never required.

## Features

- **Plaintext-first** -- issues are readable Markdown files you can grep, edit,
  and version-control with standard tools
- **Git-backed** -- every change is committed; sync across machines with
  `git push`/`pull`
- **Offline by default** -- no network needed for day-to-day work
- **Multi-remote sync** -- pull/push issues to GitHub and GitLab projects with
  conflict detection and resolution
- **Kanban board** -- terminal tree view grouped by status
- **Recurring tasks** -- schedule daily/weekly/monthly/yearly issues from
  templates
- **Sessions** -- one-command start/end workflow for multi-host setups
- **AI-optional** -- generate issue bodies, auto-triage, summarize, and ask
  questions about your backlog (bring your own AI CLI)

## Quick start

```bash
make install          # install to ~/.cargo/bin
tsk init              # create the issue repository
tsk new --title "My first issue"
tsk board             # view the kanban board
```

## Installation

```bash
# Install to ~/.cargo/bin
make install

# Equivalent cargo command
cargo install --path .

# Remove
make uninstall
```

> **Note:** After installing, run `tsk init` to create the issue repository
> before using any other command.

`make install` installs the Rust binary to `~/.cargo/bin/tsk`.

Shell completions are generated on demand:

```bash
tsk completions bash > ~/.local/share/bash-completion/completions/tsk
tsk completions zsh > "${fpath[1]}/_tsk"
tsk completions fish > ~/.config/fish/completions/tsk.fish
```

## Usage

### Issue lifecycle

```bash
tsk new                                  # interactive new issue
tsk new --title "Fix login bug"          # quick create with title
tsk new --template bug --priority high   # use a template
tsk new --title "Draft spec" --ai        # AI generates the body

tsk ls                                   # list issues (default: todo)
tsk ls --status in-progress              # filter by status
tsk ls --priority high --project myapp   # combine filters
tsk ls --all                             # all statuses

tsk show <ID>                            # display full issue
tsk edit <ID>                            # open in $EDITOR
tsk status <ID> in-progress              # change status
tsk close <ID>                           # move to done
tsk reopen <ID>                          # move back from done
tsk rm <ID>                              # delete issue
tsk path <ID>                            # print file path
```

When `fzf` is installed, omitting `<ID>` opens an interactive picker.

### Branching & pull requests

Each issue carries an `id-slug` field (e.g., `42-fix-login-bug`) that doubles
as the branch name. The workflow enforces a strict chain:
**task → branch → PR/MR**.

```bash
tsk new --title "Fix login bug"      # 1. create the issue (required first)
tsk branch WHL-042                   # 2. create & push branch from issue
# ... work on the branch, commit, push ...
tsk pr                               # 3. open PR/MR from current branch
tsk done WHL-042                     # 4. merge, clean up, and mark done
```

`tsk branch` must be run from within the project's git working directory (not
`$RIPTSK_REPO`). It creates the branch locally and pushes it to `origin`.

`tsk pr` detects the current branch, finds the matching issue (by `branch` or
`id-slug` frontmatter field), and creates a pull request (GitHub) or merge
request (GitLab) with:

- **Title**: `Resolve "<issue title>"`
- **Body**: `Closes #<issue_number>` followed by the issue body

The issue status is automatically changed to `in-progress` if it was in
`backlog` or `todo`. The PR/MR URL is stored in the issue's `pr_url`
frontmatter field.

`tsk done` completes the PR workflow for an issue. It is a convenience
wrapper that combines several individual commands into a single operation.
In order, it:

1. Validates that the project working tree is clean.
2. Merges the PR — skips if already merged, supports confirmation and
   auto-merge (`tsk pr merge <ID>`).
3. Checks out the default branch and pulls the latest changes
   (`git checkout` + `git pull`).
4. Deletes the remote and local branch, clears the issue's `branch` and
   `id_slug` metadata (`tsk branch <ID> -D`).
5. Changes the issue status to `done` (`tsk close <ID>`).

When auto-commit is enabled, `tsk done` creates a single commit in the
`$RIPTSK_REPO` (the tasks repository, not the project repository) covering
all issue metadata changes (steps 4-5). When running the steps individually,
each command (`tsk branch -D`, `tsk close`) auto-commits to `$RIPTSK_REPO`
separately.

#### Manual step-by-step (equivalent to `tsk done`)

Each step of `tsk done` can be performed individually:

```bash
tsk pr merge <ID>                          # 1. merge the PR (or --auto-merge)
git checkout <default-branch> && git pull   # 2. switch to default branch
tsk branch <ID> -D                         # 3. delete branches + clear metadata
tsk close <ID>                             # 4. set status to done
```

`tsk pr show <ID>` can be used beforehand to inspect the PR state.

Options (shared by both `tsk done` and `tsk pr merge`):

- `--merge-method <merge|squash|rebase>`: choose the merge strategy.
- `--auto-merge`: enable auto-merge and wait for required checks to pass.
- `--yes`, `-y`: skip the confirmation prompt before merging.
- `--timeout <SECONDS>`: set how long to wait for auto-merge before failing.

Additional `tsk done` options:

- `--project`, `-p`: limit issue resolution to one or more specific projects.
- `--all-projects`, `-a`: allow issue resolution across all registered projects.

### Kanban board

The default board lanes are: **todo**, **in-progress**, and **done**. Custom
boards and statuses can be defined in the `boards` section of `riptsk.yaml`.

```bash
tsk board                    # show default board
tsk board --board work       # show a specific board
tsk board --all              # show all boards
tsk board --open             # open board directory in ui.opener (e.g. nvim)
```

To open the board in `nvim`, set `ui.opener` in `riptsk.yaml`:

```bash
tsk config set ui.opener nvim
```

Change issue status between lanes and reorder within a lane:

```bash
tsk status <ID> in-progress  # change status (move to a different lane)
tsk close <ID>               # move to done
tsk reopen <ID>              # move back from done

tsk reorder <status>         # interactive reorder (requires fzf)
tsk reorder-up <ID>          # move issue up in its lane
tsk reorder-down <ID>        # move issue down in its lane
```

### Sync with GitHub / GitLab

Requires a configured remote in `riptsk.yaml` and an API token
(`GITHUB_TOKEN`/`GH_TOKEN` for GitHub, `GITLAB_TOKEN` for GitLab).

```bash
tsk sync                     # pull then push (default)
tsk sync pull                # fetch remote issues
tsk sync pull 42             # fetch one issue
tsk sync push                # push local changes
tsk sync push 42             # push one issue
tsk sync status              # show unsynced issues
tsk sync resolve <ID>        # finish conflict resolution
tsk sync pull --auto-triage  # AI auto-categorize on pull

tsk ls --conflicts           # list conflicting issues
tsk sync resolve <ID> --take-local
tsk sync resolve <ID> --take-remote
```

#### Conflict resolution

When `tsk sync pull` detects a conflict, `tsk` keeps both clean versions as
`issues/<ID>.LOCAL.md` and `issues/<ID>.REMOTE.md`, then overwrites the main
`issues/<ID>.md` file with git-style conflict markers. That main file is
intentionally not parseable until you resolve it.

Use `tsk ls --conflicts` to find unresolved conflicts. `tsk show <ID>` still
prints the raw file, and `tsk edit <ID>` still opens the file in your editor so
you can resolve the markers manually.

After editing, run one of:

```bash
tsk sync resolve <ID>               # accept your manual edit, remove backups
tsk sync resolve <ID> --take-local  # restore the local backup
tsk sync resolve <ID> --take-remote # restore the remote backup
```

### Templates

```bash
tsk template list            # list available templates
tsk template show bug        # display a template
tsk template new             # create a new template
tsk template edit bug        # edit in $EDITOR
tsk template rm bug          # delete
tsk template validate bug    # check YAML and fields
```

Built-in templates: `task`, `bug`, `feature`, `weekly-review`.

### Recurring tasks

```bash
tsk recur list               # list recurring definitions
tsk recur new                # create a new recurrence interactively
tsk recur run                # create all issues due today
tsk recur run 2026-04-01     # create issues due on a specific date
tsk recur skip <recur_id>    # skip without creating
```

Recurring definitions live in `riptsk.yaml` and support daily, weekly (with
`day_of_week`), monthly (with `day_of_month`), and yearly frequencies.

### Sessions

For multi-host workflows where `$RIPTSK_REPO` is a Git repository:

```bash
tsk session start            # git pull + sync pull
tsk session end              # sync push + git commit + git push
```

### AI features

riptsk is **agent-agnostic** -- you provide your own AI CLI command and riptsk
injects the right context via template placeholders. Any CLI tool that reads a
prompt and writes to stdout will work (`claude`, `llm`, `ollama`, `sgpt`,
a custom script, etc.).

#### Setup

1. Enable AI and set your command in `riptsk.yaml`:

```yaml
ai:
  enabled: true
  command: "cat {{input_file}} | claude -p --model haiku --system-prompt {{system}}"
```

Or via the CLI:

```bash
tsk config set ai.enabled true
tsk config set ai.command "cat {{input_file}} | claude -p --model haiku --system-prompt {{system}}"
```

2. Make sure your AI CLI is authenticated and on `$PATH`.

#### Template placeholders

Your `ai.command` is a shell command template with these placeholders:

| Placeholder | Value | Notes |
|---|---|---|
| `{{system}}` | Shell-escaped system prompt | Already quoted — do **not** wrap in extra quotes |
| `{{input}}` | Shell-escaped input content | Already quoted — do **not** wrap in extra quotes; may hit shell arg limits on large diffs |
| `{{input_file}}` | Path to a temp file with raw input | Recommended for large inputs (PR diffs, issue corpora) |

The command is executed via `sh -c`, stdout is captured as the AI response, and
a non-zero exit code is treated as an error.

#### Recommended configurations

**Claude Code CLI** (recommended -- handles large inputs via stdin):

```yaml
ai:
  command: "cat {{input_file}} | claude -p --model haiku --system-prompt {{system}}"
```

**llm** (Simon Willison's CLI):

```yaml
ai:
  command: "cat {{input_file}} | llm -s {{system}}"
```

**Ollama** (local models):

```yaml
ai:
  command: "cat {{input_file}} | ollama run llama3 --system {{system}}"
```

**Simple inline** (for CLIs that accept short prompts as arguments):

```yaml
ai:
  command: "my-ai-cli --system {{system}} --prompt {{input}}"
```

#### Feature toggles

Individual AI features can be enabled or disabled:

```yaml
ai:
  enabled: true
  command: "..."
  features:
    new_body_gen: true   # tsk new --ai
    triage: true         # tsk sync pull --auto-triage
    summarize: true      # tsk summarize
    ask: true            # tsk ask
```

#### Usage

```bash
tsk new --title "Refactor auth" --ai   # AI writes the issue body
tsk pr                                 # AI generates PR description
tsk pr edit                            # AI updates PR description
tsk summarize                          # summarize all issues
tsk summarize --cycle 2026-Q1          # summarize a cycle
tsk ask "What are the highest priority bugs?"
```

Use `--no-ai` with `tsk pr` or `tsk pr edit` to skip AI generation.

### Configuration

```bash
tsk config                   # show current config
tsk config set key value     # set a config value
tsk register                 # register current project
tsk register --list          # list registered projects
```

### Git hooks

```bash
tsk hooks install            # install pre-commit hook
tsk hooks status             # check hook status
tsk hooks update             # update hook
tsk hooks uninstall          # remove hook
```

## Issue format

Issues are Markdown files stored in `$RIPTSK_REPO/issues/`. Each file has YAML
frontmatter followed by a Markdown body:

```markdown
---
id: WHL-042
title: Fix wormhole stabilizer retry logic
status: in-progress
board: personal
project: wormhole-router
priority: high
labels: [bug, temporal-drift]
assignee: ppuffin
cycle: 2026-Q1
order: 1
due: 2026-03-20
---

## Description

The stabilizer retry logic does not back off correctly...
```

Key frontmatter fields: `id`, `title`, `status`, `board`, `project`, `org`,
`priority` (low/medium/high/urgent), `labels`, `assignee`, `milestone`,
`cycle`, `order`, `due`, `recurring`.

Local issues get a provisional `LOCAL-<hex>` ID until first sync, when they
receive a project-prefixed ID (e.g., `WHL-042`).

## Dependencies

### Required

- **git**
- **Rust toolchain** (for building from source)

### Optional

| Tool | Used for |
|------|----------|
| `fzf` | Interactive issue selection |
| Any AI CLI (`claude`, `llm`, `ollama`, etc.) | AI features (configured via `ai.command`) |
| `bash` | Managed hook script execution after `tsk hooks install` |
| `yq`, `jq` | Managed hook validation after `tsk hooks install` |

## Configuration

riptsk follows the [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/latest/):

| Variable | Default | Contents |
|----------|---------|----------|
| `$RIPTSK_REPO` (`$XDG_DATA_HOME/riptsk`) | `~/.local/share/riptsk` | Issues, templates, `riptsk.yaml` |
| `$XDG_CONFIG_HOME/riptsk` | `~/.config/riptsk` | `config.env` |
| `$XDG_CACHE_HOME/riptsk` | `~/.cache/riptsk` | View cache, ID map |

The main configuration file is `riptsk.yaml` in `$RIPTSK_REPO`. Run `tsk init` to
generate one with sensible defaults. Key sections:

- **defaults** -- default board, status, priority, template for new issues
- **remotes** -- GitHub/GitLab project connections
- **boards** -- kanban board definitions with custom status lists
- **ui** -- opener command, tree depth, fzf options
- **ai** -- command template, enabled features
- **sync** -- conflict detection toggle
- **recurring** -- recurring task definitions

## Development

```bash
make lint    # cargo fmt --check + cargo clippy
make test    # cargo nextest run
make check   # both lint and test
```

Pre-commit hooks enforce formatting, linting, and security checks on commit and
push. See `.pre-commit-config.yaml` for the full hook list.
