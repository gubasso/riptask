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
> | Template CRUD | `lib/tsk/template.sh, tests/integration/issue_lifecycle.bats` |
>
> ---

# Templates

### Overview

Templates live in `$RIPTASK_REPO/templates/` and define the starting content for new issues. `tsk init` ships default templates (`task.md`, `bug.md`, `feature.md`, `weekly-review.md`). Users can create, edit, and remove their own custom templates alongside the defaults.

### Template file format

Markdown with YAML frontmatter (same delimiters as issue files):

```markdown
---
template_name: Bug Report
default_labels: [bug]
default_state: todo
default_priority: high
---

## Description

[Describe the bug]

## Steps to reproduce

1. ...

## Expected behavior

[What should happen]
```

**Frontmatter fields:**

| Field | Type | Required | Default |
|---|---|---|---|
| `template_name` | string | No | Filename stem (e.g. `bug` from `bug.md`) |
| `default_labels` | list | No | `[]` |
| `default_state` | enum | No | Falls back to `defaults.state` in `riptask.yaml`, then `todo` |
| `default_priority` | enum | No | Falls back to `defaults.priority` in `riptask.yaml`, then `medium` |

All `default_*` fields are optional. When omitted, `tsk new` falls back to `riptask.yaml` `defaults:` values, then hardcoded defaults.

Priority mapping at issue creation time is explicit:
- `none` → omit template priority and fall back to `defaults.priority` from `riptask.yaml`
- `critical` → issue priority `urgent`
- `low`, `medium`, `high` → map 1:1

The body (everything after the frontmatter closing `---`) is copied verbatim into the new issue.

### CLI: `tsk template` subcommand group

```bash
tsk template list
# List all templates in $RIPTASK_REPO/templates/
# Columnar output: filename, template_name
# Exit 0 even if no templates exist (empty list)

tsk template show [<name>]
# Print template contents to stdout
# If <name> omitted: fzf picker (see below)
# Exit 2 if template not found

tsk template new [<name>]
# Create $RIPTASK_REPO/templates/<name>.md from skeleton (see below)
# Opens $EDITOR for immediate editing
# If <name> omitted: prompt for name
# Exit 1 if template already exists

tsk template edit [<name>]
# Open $RIPTASK_REPO/templates/<name>.md in $EDITOR
# Runs validation after editor closes (see below)
# If <name> omitted: fzf picker
# Exit 2 if template not found

tsk template rm [<name>]
# Remove $RIPTASK_REPO/templates/<name>.md
# Prompts for confirmation before deletion
# If <name> omitted: fzf picker
# Exit 2 if template not found

tsk template validate [<name>]
# Validate template YAML (see validation rules below)
# If <name> omitted: validate all templates in $RIPTASK_REPO/templates/
# Exit 0 if valid, exit 1 if any validation errors
```

All subcommands that accept `[<name>]` match against the filename stem (e.g. `bug` matches `templates/bug.md`).

### Template skeleton

`tsk template new` creates a new file from this skeleton:

```markdown
---
template_name: <name>
default_labels: []
default_state: todo
default_priority: medium
---

## Description

[Describe the issue]

## Tasks

- [ ] ...
```

Where `<name>` is replaced with the user-provided template name.

### `tsk new` template selection

Templates integrate with `tsk new` via a `--template` / `-T` flag:

```bash
tsk new --template bug
# Use templates/bug.md as the starting point

tsk new -T feature
# Short form
```

**Selection order:**

1. `--template` / `-T` flag — explicit selection
2. Interactive mode (stdin is TTY, no `--template`): fzf picker over `$RIPTASK_REPO/templates/`
3. Non-interactive fallback: `defaults.template` from `riptask.yaml` (default: `task`)

When a template is selected, `tsk new`:
1. Copies the template body into the new issue
2. Applies `default_labels`, `default_state`, `default_priority` from the template frontmatter (these can still be overridden by explicit `--state`, `--priority`, `--labels` flags). Priority maps as follows: `none` falls back to `defaults.priority`, `critical` maps to issue priority `urgent`, and all other values map 1:1. If the resolved `default_state` is not in the target board's `states[]` list, `tsk new` prints a warning and falls back to the board's first defined state
3. Opens `$EDITOR` for further editing (unless `--title` and other required fields make it fully non-interactive)

### Validation

Validation runs automatically after `tsk template edit` closes `$EDITOR`, and can be invoked explicitly via `tsk template validate`. Checks are minimal:

1. **Frontmatter delimiters** — file starts with `---` and contains a closing `---`
2. **YAML parsing** — frontmatter parses successfully via `yq`
3. **Enum values** — if `default_state` is present, it must be one of the global issue states: `backlog`, `todo`, `in-progress`, `review`, `done`; if `default_priority` is present, it must be one of: `none`, `low`, `medium`, `high`, `critical`

Validation errors are printed to stderr with the template filename and specific error. `tsk template edit` re-opens `$EDITOR` if validation fails (with the error displayed), giving the user a chance to fix the issue.

### fzf template picker

Used by `tsk template show`, `tsk template edit`, `tsk template rm`, and `tsk new` (when no `--template` flag in interactive mode).

**Display format:**

```
bug          Bug Report
feature      Feature Request
task         Task
weekly-review Weekly Review
```

Columnar: filename stem (left-aligned) + `template_name` from frontmatter.

**Preview pane:** Shows the template body (everything after frontmatter), rendered via `cat`. Uses the same fzf preview pattern as the issue picker (see [21 — fzf Interactive Selection](21-fzf-interactive-selection.md)).

When stdin is not a TTY, omitting `<name>` produces an error (exit 1) — consistent with all other fzf-dependent commands.

### Exit codes

Template commands follow the project-wide exit code conventions (see [08 — CLI Design](08-cli-design.md)):

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | General error (validation failure, template already exists, missing name in non-interactive mode) |
| 2 | Not found (template not found) |
| 5 | Configuration error |
