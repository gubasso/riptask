# CLAUDE.md

## Versioning

This project is pre-1.0.0. There is NO backward compatibility requirement. Breaking changes are allowed and expected — do not add migration paths, compatibility shims, or legacy support for old formats.

## Terminology

The terms in this glossary have single meanings — do not introduce ambiguous synonyms. Use the `code identifier` column in Rust source and the `YAML / doc form` column elsewhere.

| Concept | Code identifier | YAML / doc form | Meaning |
|---|---|---|---|
| **RepoProject** | `RepoProject` (struct), `repo_project` (vars) | `project` (generic), `repo_project` when precision needed | The coding project — a git repo (or a local non-git directory). The unit the user works on. What GitHub/GitLab call a "project" or "repo". |
| **VCBackend** | `VCBackendSpec` (struct), `vc_backend` (field/vars) | `vc_backend` | Version control nature: GitHub, GitLab, or Local. Provides branches, PRs/MRs, CI. The `kind` field is a `BackendKind`. |
| **TasksBackend** | `TasksBackendSpec` (struct), `tasks_backend` (field/vars) | `tasks_backend` | Issue tracking nature: GitHub, GitLab, Jira, or Local. Provides issues/tasks CRUD. The `kind` field is a `BackendKind`. |
| **BackendKind** | `BackendKind` (enum) | `type` (YAML key within `vc_backend` / `tasks_backend`) | The provider-identity enum shared by both backend specs: `Github`, `Gitlab`, `Jira`, `Local`. Replaces the pre-refactor `Backend` enum. |
| **RepoProject label** | `repo_project_label` | `repo_project_label` (preferred) or `project_label` | String used to partition a shared Jira project by RepoProject. Always carries the fixed `proj::` prefix (e.g. `proj::my-api`) so the Jira label unambiguously identifies a RepoProject reference. Injected into Jira labels on push; filtered on pull. |
| **RepoProject key** | `repo_project_key` (new synonym) / `key` | `key` | Short ID prefix (e.g. `RIPTASK` in `RIPTASK--123`). One per RepoProject. `BackendConfig.key` is renamed into the RepoProject.) |
| **JiraProject** | `JiraProject`, `jira_project` | `jira_project` | The external Jira container (e.g. `PROJ`). Stored as `"org/PROJ"`. Replaces the `repo:` field when a TasksBackend is of type Jira. |
| **Backend** (plain) | — | — | **Banned** as a standalone term for a project-level concept. Use "VCBackend" (`VCBackendSpec`) or "TasksBackend" (`TasksBackendSpec`) explicitly. The pre-refactor `Backend` provider-identity enum has been renamed to `BackendKind`. |

Rules:

- The word "project" in code or docs, unqualified, always means RepoProject.
- "Jira project" (two words) is always the external Jira concept; never abbreviated to "project" alone.
- `BackendConfig` is replaced entirely by `RepoProject` (plus inline `VCBackendSpec` and `TasksBackendSpec`).
- The `--project` / `-p` CLI flag keeps its name; it now literally selects a RepoProject by name (no change in behavior — just cleanly matches the terminology).
- `IssueFrontmatter.project: String` keeps its name; it stores `RepoProject.name`.

## Config Hierarchy

riptask loads config from three YAML layers, lowest precedence to highest:

- System: `$RIPTASK_REPO/config.yaml`
- User: `$XDG_CONFIG_HOME/riptask/config.yaml`
- Local: `<project-root>/.riptask/config.yaml`

Layer files are parsed as partial config and merged before validation. `--user` is the canonical user-scope flag; `--global` is an alias. `tsk config set` and `tsk config edit` accept `--system`, `--user`, `--global`, and `--local`; unscoped writes use Local inside a project and User otherwise.

`tsk init` does not register projects. It always prompts interactively if no scope flag is given; in non-TTY contexts it requires `--system`, `--user`, or `--local`.

## Pre-commit configs

- `.pre-commit-config.yaml` — active repository config

## Linting & Testing

The justfile wraps the primary local checks directly:

```bash
just lint   # cargo fmt --check + cargo clippy --all-targets --all-features -- -D warnings
just test   # cargo nextest run
just check  # runs both lint and test
```

- `.pre-commit-config.yaml` still defines commit-time and pre-push hooks for formatting, linting, and security checks.

## Backend Implementation Priority

When implementing backend (GitHub/GitLab) functionality, prefer in this order:
1. octocrab/gitlab native Rust crate methods
2. Direct REST API or GraphQL calls
3. gh/glab CLI commands
4. Other approaches
