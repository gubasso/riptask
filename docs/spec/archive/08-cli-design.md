> **[ARCHIVED]** — This spec was the design input for implementation. Code + tests are
> now the source of truth for this behavior. This document is retained as historical
> record only.
>
> - **Status:** archived
> - **Date archived:** 2026-03-18
> - **Source of truth:** code + tests
>
> | Concern | Source of Truth |
> |---------|----------------|
> | CLI commands and dispatch | `bin/tsk, lib/tsk/*.sh, tests/e2e/cli_dispatch.bats` |
>
> ---

# CLI Design

### Binary name

`tsk` — single executable, dispatches to subcommands.

### Environment

```bash
TSK_REPO=~/.local/share/tsk  # path to data repo (default: ~/.local/share/tsk)
TSK_AI_MODEL=...           # override default AI model
EDITOR=nvim                # used by tsk edit, tsk new
```

### Commands

#### Issue lifecycle

```bash
tsk new
# Interactive: prompts for title, project, board, state, priority
# Auto-detects project from $PWD git remote if registered
# Creates issues/<ID>.md from template (LOCAL-<hash> for gh/glab projects, <PREFIX>-<seq> for local projects)
# Regenerates views

tsk new --title "Fix wormhole stabilizer" \
        --project wormhole-router \
        --board penguin-chrono-labs \
        --state todo \
        --priority high \
        --template bug
# Non-interactive. All flags optional — missing ones are prompted or defaulted.
# --template / -t selects a template from $TSK_REPO/templates/ (see [24 — Templates](24-templates.md))

tsk new --title "Fix wormhole stabilizer" --ai
# Same as above but pre-fills body via LLM before opening editor
# Auto-commits $TSK_REPO if the new issue is local (see spec 15)

tsk edit [<ID>]
# Opens $EDITOR on issues/<ID>.md
# Updates local_updated_at on save
# Regenerates views
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk show [<ID>]
# Prints rendered issue to stdout
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk move [<ID>] [<state>]
# Patches state: in frontmatter
# Updates local_updated_at
# Regenerates views
# Auto-commits $TSK_REPO if all affected issues are local (see spec 15)
# If omitted: interactive fzf selection — picks issue (non-done), then state
#   (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk close [<ID>]
# Sets state: done
# Updates local_updated_at
# Regenerates views
# Auto-commits $TSK_REPO if all affected issues are local (see spec 15)
# If omitted: interactive fzf selection — only non-done issues shown
#   (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk reopen [<ID>]
# Sets state: todo
# Updates local_updated_at
# Regenerates views
# Auto-commits $TSK_REPO if all affected issues are local (see spec 15)
# If omitted: interactive fzf selection — only done issues shown
#   (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk rm [<ID>]
# Removes issues/<ID>.md
# If synced: prompts to also close on remote or just remove locally
# Regenerates views
# Auto-commits $TSK_REPO if all affected issues are local (see spec 15)
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk path [<ID>]
# Prints absolute path to issues/<ID>.md
# Used by editor/file manager integrations
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))
```

#### Listing and filtering

```bash
tsk ls
# All non-done issues, all projects

tsk ls --state in-progress
tsk ls --state in-progress --priority high
tsk ls --project wormhole-router
tsk ls --board penguin-chrono-labs
tsk ls --cycle 2026-Q1
tsk ls --assignee ppuffin
tsk ls --all                       # include done/closed
tsk ls --unsynced                 # only provisional LOCAL-* issues
tsk ls --conflicts                 # only issues with unresolved conflicts
```

Issues with an unresolved `conflict:` field show a `[CONFLICT]` marker in `tsk ls` output. `.REMOTE.md` files are never shown in `tsk ls`.

#### View management

```bash
tsk board
# tree of the board for the current project (detected from $PWD)

tsk board --all
# tree of all kanbans

tsk board --board penguin-chrono-labs
# explicit board override

tsk board --open
# open using ui.opener from tsk.yaml (nvim -R, yazi, lf — see config)

tsk board --path
# print path only, for composition:
#   tsk board --path | xargs lf
#   nvim -R $(tsk board --path)

tsk view
# wipe and regenerate all views/
```

When inside a project repo, `tsk board` resolves that project's `default_board` from `tsk.yaml` and shows `views/kanban/<board>/`. The kanban tree is board-keyed, not project-keyed.

#### Reordering

```bash
tsk reorder <state>
# interactive reorder within a lane using fzf
# context-aware: uses current project from $PWD

tsk reorder <state> --board <board>
# explicit board

tsk reorder-up [<ID>]
# move one position up in current lane
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk reorder-down [<ID>]
# move one position down in current lane
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))
```

#### Sync

```bash
tsk sync
# full bidirectional sync: pull then push, all configured remotes
# regenerates views after completion

tsk sync pull
# remote → local, all remotes
# if run from inside a project repo: scope to that remote only

tsk sync push
# local → remote, all dirty issues
# if run from inside a project repo: scope to that remote only

tsk sync pull --all
# explicit override: sync all remotes regardless of $PWD

tsk sync pull --remote chrono-wormhole
# scope to a named remote

tsk sync status
# dry-run: show what would be pushed/pulled without executing
# compares local_updated_at vs remote updated_at for each issue
# lists unresolved conflicts first, prominently

tsk sync pull --force
# bypass conflict detection for this pull (uses last-write-wins)
# remote unconditionally overwrites local for all changed issues

tsk sync pull --triage
# after pull: run AI triage on new issues (suggest state/priority/labels)
# requires ai.enabled: true in tsk.yaml

tsk sync pull --triage --auto-triage
# same as --triage, but applies suggestions without per-issue confirmation
```

#### Session management

```bash
tsk session start
# git -C $TSK_REPO pull
# tsk sync pull
# reports: N issues updated, M local-only issues received from other hosts

tsk session end
# tsk sync push
# git -C $TSK_REPO add -A
# git -C $TSK_REPO commit -m "tsk: session end $(hostname) $(date +%Y-%m-%d)"
# git -C $TSK_REPO push
# reports: N issues pushed, M commits synced
```

`session end` stages all changes in `$TSK_REPO` via `git add -A`. This is safe because `$TSK_REPO` contains only issue data, templates, config, and the comment-only `.gitignore` — cache lives outside the repo. This is broader than lifecycle auto-commit ([15 — Version Control & Backup](15-version-control-backup.md)), which stages only the specific files affected by the operation.

#### Recurring tasks

```bash
tsk recur list
# list all recurring task definitions from tsk.yaml

tsk recur new
# interactive: choose template, set frequency, start/end dates
# writes definition to tsk.yaml

tsk recur run
# instantiate all recurring tasks that are due
# idempotent: safe to run from cron

tsk recur skip [<recur-id>]
# skip this occurrence without disabling the recurrence
# If omitted: interactive fzf selection from recurring definitions
#   (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))
```

#### Template management

```bash
tsk template list
# list all templates (name + template_name from frontmatter)

tsk template show [<name>]
# print template to stdout
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk template new [<name>]
# create template from skeleton, open $EDITOR
# If omitted: prompt for name

tsk template edit [<name>]
# edit existing template in $EDITOR, validate on close
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk template rm [<name>]
# remove template with confirmation
# If omitted: interactive fzf selection (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md))

tsk template validate [<name>]
# check YAML validity (all templates if omitted)
```

See [24 — Templates](24-templates.md) for full details on template format, validation, and `tsk new` integration.

#### Conflict resolution

```bash
tsk resolve <ID>
# default: user has manually edited issues/<ID>.md to incorporate remote changes
# removes conflict: from frontmatter
# deletes issues/<ID>.REMOTE.md
# updates local_updated_at
# regenerates views

tsk resolve <ID> --take-remote
# replaces local file content with the remote version
# removes conflict: from frontmatter
# deletes issues/<ID>.REMOTE.md
# updates local_updated_at
# regenerates views

tsk resolve <ID> --take-local
# keeps local file as-is
# removes conflict: from frontmatter
# deletes issues/<ID>.REMOTE.md
# updates local_updated_at
# regenerates views
```

`tsk resolve` handles a missing `.REMOTE.md` gracefully (the user may have deleted it manually — the `conflict:` field in frontmatter is the authoritative conflict indicator).

#### Project registration

```bash
tsk register
# detect $PWD git remote
# prompt for project config (prefix, board, org)
# write entry to $TSK_REPO/tsk.yaml

tsk register --list
# list all registered projects
```

#### AI

```bash
tsk summarize
# summarize current project's issues via LLM
# context-aware from $PWD

tsk summarize --cycle 2026-Q1
tsk summarize --board penguin-chrono-labs

tsk ask "<question>"
# freeform query over issues/ using LLM
# tsk ask "what is blocking the IceOS 5.0 milestone?"
# tsk ask "what did I work on last week?"
```

#### Repository hooks

```bash
tsk hooks install
# copy pre-commit hook to $TSK_REPO/.git/hooks/; refuse if non-tsk hook exists (--force to override)

tsk hooks update
# replace if source version > installed version

tsk hooks status
# show installed vs available version

tsk hooks uninstall
# remove only if tsk-managed (has version header)
```

See [25 — TSK_REPO Data Repository Hooks](25-tsk-repo-hooks.md) for full details on validation rules and hook design.

#### Utility

```bash
tsk init
# initialize a new tasks repo in $TSK_REPO (default: ~/.local/share/tsk)
# creates: issues/ templates/ tsk.yaml .gitignore
#   (.gitignore is comment-only; cache lives outside the repo)
# also creates ~/.cache/tsk/ for cache files
# runs git init + initial commit

tsk config
# show current tsk.yaml

tsk config set <key> <value>
# update a config value

tsk version
tsk <command> -h/--help    # primary: per-command help
tsk help [command]         # alias (same code path)
```

#### Commit helper

```bash
tsk commit [-e|--edit]
# stage issues/ templates/ tsk.yaml
# generate commit message summarizing changes since last commit
# without --edit: commit immediately with generated message
# with --edit: open $EDITOR with generated message pre-filled, user can modify before commit
# does NOT auto-push (explicit git push or tsk session end)
# covers deferred changes not auto-committed: synced issue edits, comments, tags, config, etc.
```

The generated message follows the conventions from `docs/spec/15-version-control-backup.md`:

```
tsk: close WHL-042 — wormhole stabilizer fix merged
tsk: new WHL-043 — investigate temporal image drift
tsk: move WHL-041 in-progress → review, close WHL-042
```

For multiple changes, combine into a single summary line or multi-line message:

```
tsk: 3 issues updated

- close WHL-042 — wormhole stabilizer fix merged
- new WHL-043 — investigate temporal image drift
- move WHL-041 in-progress → review
```

### Interactive selection

When any ID-taking command is invoked without its `<ID>` argument and stdin is a TTY, `tsk` opens an fzf picker for interactive selection. Each command pre-filters the picker to show only relevant issues (e.g., `tsk close` shows only non-done issues, `tsk reopen` shows only done issues). `tsk move` supports two-stage selection: pick issue, then pick target state. When stdin is not a TTY, missing arguments produce an error (exit 1) to preserve scriptability. See [21 — fzf Interactive Selection](21-fzf-interactive-selection.md) for full details.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | General error |
| 2 | Not found |
| 3 | Sync conflict requiring manual resolution |
| 4 | Remote unreachable |
| 5 | Configuration error |
| 6 | Project not registered (run tsk register) |
