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
- **Kanban board** -- terminal tree view grouped by state
- **Recurring tasks** -- schedule daily/weekly/monthly/yearly issues from
  templates
- **Sessions** -- one-command start/end workflow for multi-host setups
- **AI-optional** -- generate issue bodies, auto-triage, summarize, and ask
  questions about your backlog (requires `claude` or `llm` CLI)

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
tsk ls --state in-progress               # filter by state
tsk ls --priority high --project myapp   # combine filters
tsk ls --all                             # all states

tsk show <ID>                            # display full issue
tsk edit <ID>                            # open in $EDITOR
tsk move <ID> in-progress                # change state
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
```

`tsk branch` must be run from within the project's git working directory (not
`$RIPTSK_REPO`). It creates the branch locally and pushes it to `origin`.

`tsk pr` detects the current branch, finds the matching issue (by `branch` or
`id-slug` frontmatter field), and creates a pull request (GitHub) or merge
request (GitLab) with:

- **Title**: `Resolve "<issue title>"`
- **Body**: `Closes #<issue_number>` followed by the issue body

The issue is automatically moved to `in-progress` if it was in `backlog` or
`todo`. The PR/MR URL is stored in the issue's `pr_url` frontmatter field.

### Kanban board

The default board lanes are: **todo**, **in-progress**, and **done**. Custom
boards and states can be defined in the `boards` section of `riptsk.yaml`.

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

Move issues between lanes (states) and reorder within a lane:

```bash
tsk move <ID> in-progress    # change state (move to a different lane)
tsk close <ID>               # move to done
tsk reopen <ID>              # move back from done

tsk reorder <state>          # interactive reorder (requires fzf)
tsk reorder-up <ID>          # move issue up in its lane
tsk reorder-down <ID>        # move issue down in its lane
```

### Sync with GitHub / GitLab

Requires a configured remote in `riptsk.yaml` and an API token
(`GITHUB_TOKEN`/`GH_TOKEN` for GitHub, `GITLAB_TOKEN` for GitLab).

```bash
tsk sync                     # pull then push (default)
tsk sync pull                # fetch remote issues
tsk sync push                # push local changes
tsk sync status              # show unsynced issues
tsk sync pull --auto-triage  # AI auto-categorize on pull

tsk ls --conflicts           # list conflicting issues
tsk resolve <ID>             # resolve in $EDITOR
tsk resolve <ID> --take-local
tsk resolve <ID> --take-remote
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

Requires `ai.enabled: true` in `riptsk.yaml` and the `claude` or `llm` CLI with
an Anthropic API key.

```bash
tsk new --title "Refactor auth" --ai   # AI writes the issue body
tsk summarize                          # summarize all issues
tsk summarize --cycle 2026-Q1          # summarize a cycle
tsk ask "What are the highest priority bugs?"
```

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
state: in-progress
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

Key frontmatter fields: `id`, `title`, `state`, `board`, `project`, `org`,
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
| `claude` or `llm` | AI features |
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

- **defaults** -- default board, state, priority, template for new issues
- **remotes** -- GitHub/GitLab project connections
- **boards** -- kanban board definitions with custom state lists
- **ui** -- opener command, tree depth, fzf options
- **ai** -- model, enabled features
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
