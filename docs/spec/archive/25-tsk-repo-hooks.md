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
> | Pre-commit hook and hooks CLI | `lib/tsk/hooks.sh, lib/tsk/hooks/pre-commit, tests/unit/hooks.bats` |
>
> ---

# RIPTASK_REPO Data Repository Hooks

### Overview

`tsk` stores issues as markdown+YAML-frontmatter files in a git repo at `$RIPTASK_REPO`. Since riptask encourages an editor-first workflow (users edit issue files directly), invalid data can be committed, bypassing riptask's application-layer validation. A native git pre-commit hook in `$RIPTASK_REPO/.git/hooks/` acts as the last safety net before bad data gets versioned.

This is a self-contained bash script — **not** the Python pre-commit framework. It uses `yq` (already a riptask dependency) for YAML validation.

Only staged files are validated (`git diff --cached --name-only --diff-filter=ACM`). Unstaged changes and deletions are ignored.

---

### Hook script design

The pre-commit hook is:

- **Self-contained** — does NOT source `lib/*.sh`. `$RIPTASK_REPO` is separate from the riptask install path; sourcing lib files would create a fragile runtime dependency on the install location.
- Lives in riptask source at `lib/hooks/pre-commit`, **copied** (not symlinked) to `$RIPTASK_REPO/.git/hooks/pre-commit` on install.
- Includes a version header: `# riptask-hook-version: 1` — enables update detection.
- Dependencies: `bash` ≥ 4.0, `yq` ≥ 4.0, `git` (all already tsk dependencies per [17 — Codebase Structure](17-codebase-structure.md)).

---

### Validation rules

#### Issue files (`issues/*.md`)

| Check | Method | Error |
|---|---|---|
| Frontmatter delimiters | Line 1 is `---`, second `---` exists | `missing frontmatter delimiters` |
| YAML parses | `yq` exits 0 on frontmatter | `invalid YAML in frontmatter` |
| Required fields | `title`, `state`, `board`, `project`, (`id` OR `local_id`), `local_updated_at` non-null | `missing required field: <name>` |
| `state` enum | `backlog \| todo \| in-progress \| review \| done` | `invalid state: "<value>"` |
| `priority` enum (if set) | `low \| medium \| high \| urgent` | `invalid priority: "<value>"` |
| `remote_deleted` boolean (if set) | `true \| false` | `invalid remote_deleted: "<value>"` |
| Filename pattern | `<PREFIX>-<NUMBER>.md` or `LOCAL-<hex>.md` | `invalid filename: "<name>"` |
| No `.REMOTE.md` | Reject `*.REMOTE.md` (temporary conflict files) | `.REMOTE.md should not be committed (use: tsk resolve <ID>)` |

Required fields follow the issue file format defined in [06 — Issue File Format](06-issue-file-format.md). An issue must have either `id` (synced) or `local_id` (pre-sync) — both being null is invalid.

**Filename patterns:**

- Synced issues: `<PREFIX>-<NUMBER>.md` where PREFIX is uppercase letters and NUMBER is digits (e.g. `WHL-042.md`, `GH-017.md`)
- Local issues: `LOCAL-<hex>.md` where hex is lowercase hex characters (e.g. `LOCAL-3f2a1b9c.md`)

#### `.REMOTE.md` bypass

`.REMOTE.md` files are temporary conflict files that should not normally be committed (see the `.REMOTE.md` convention in [06 — Issue File Format](06-issue-file-format.md)). However, `tsk sync pull` must commit them so conflict state survives host switches.

The hook checks the `RIPTASK_HOOK_ALLOW_REMOTE=1` environment variable — set by `tsk sync pull` before its internal commit — to allow `.REMOTE.md` files through. This is more surgical than `--no-verify`, which would skip ALL validation.

```bash
# In tsk sync pull, before committing:
export RIPTASK_HOOK_ALLOW_REMOTE=1
git -C "$RIPTASK_REPO" commit -m "riptask: sync pull — conflicts detected"
unset RIPTASK_HOOK_ALLOW_REMOTE
```

#### Config (`riptask.yaml`)

| Check | Method | Error |
|---|---|---|
| YAML parses | `yq` exits 0 | `invalid YAML` |
| `project_prefix` unique | No duplicates in `remotes[].project_prefix` | `duplicate project_prefix "<value>" in remotes "<a>" and "<b>"` |
| `name` unique | No duplicates in `remotes[].name` | `duplicate remote name "<value>"` |
| `boards[].name` unique | No duplicates | `duplicate board name "<value>"` |

Config structure follows [16 — Configuration](16-configuration.md).

#### Templates (`templates/*.md`)

| Check | Method | Error |
|---|---|---|
| Frontmatter delimiters | Same as issues | `missing frontmatter delimiters` |
| YAML parses | Same as issues | `invalid YAML in frontmatter` |
| `default_state` enum (if set) | `backlog \| todo \| in-progress \| review \| done` | `invalid default_state: "<value>"` |
| `default_priority` enum (if set) | `none \| low \| medium \| high \| critical` | `invalid default_priority: "<value>"` |

Note: template `default_priority` uses a different enum than issue `priority` — it includes `none` and `critical` per [24 — Templates](24-templates.md). At issue creation time, `none` falls back to `defaults.priority` from `riptask.yaml` and `critical` maps to issue priority `urgent`.

---

### Error format

```
tsk pre-commit: FAIL issues/WHL-042.md
  missing required field: board

tsk pre-commit: FAIL riptask.yaml
  duplicate project_prefix "WHL" in remotes "wormhole-router" and "other-project"

tsk pre-commit: 2 errors, commit blocked
```

- Prefix `tsk pre-commit:` makes the error origin clear in mixed output.
- Each error names the file and the specific problem.
- Summary count at end.
- All errors are collected before exiting — the hook does not stop at the first error.

---

### CLI: `tsk hooks` subcommand

```bash
tsk hooks install
# Copy lib/hooks/pre-commit to $RIPTASK_REPO/.git/hooks/pre-commit
# chmod +x the installed hook
# Refuse if a non-tsk hook already exists (no version header) — use --force to override
# Exit 0 on success, exit 1 if refused

tsk hooks install --force
# Overwrite any existing hook, even non-riptask-managed ones

tsk hooks update
# Compare installed hook version vs source version
# Replace if source version > installed version
# No-op (exit 0) if already up to date
# Refuse to update non-tsk hooks (no version header) — use --force

tsk hooks status
# Show: installed version, available version, whether update is available
# Exit 0 if hook is installed and up to date
# Exit 1 if hook is missing or outdated

tsk hooks uninstall
# Remove $RIPTASK_REPO/.git/hooks/pre-commit
# Only if riptask-managed (has version header) — refuse otherwise
# Use --force to remove non-tsk hooks
```

`tsk init` calls `hooks_install` internally after `git init` + initial commit ([08 — CLI Design](08-cli-design.md)).

---

### Versioning & updates

The hook script contains a version header on line 2:

```bash
#!/usr/bin/env bash
# riptask-hook-version: 1
```

Version management rules:

- `tsk hooks install` writes the hook with the version from the tsk source.
- `tsk hooks update` reads the installed version via `grep`, compares to source version, replaces if source is newer.
- Non-tsk hooks (no `# riptask-hook-version:` header) are never overwritten without `--force`.
- `tsk hooks uninstall` reads the header before removing — refuses to remove non-tsk hooks without `--force`.

When the hook source is updated in a new tsk release, the version number is bumped. Users run `tsk hooks update` to pick up the new version.

---

### Performance

- Only staged files are validated (not the whole repo).
- Batch `yq` calls: extract all required fields per file in a single expression (`.title, .state, .board, .project, .id, .local_id, .local_updated_at`) to minimize subprocess overhead.
- Typical commit touches 5–50 files × 1–2 `yq` calls = sub-second total validation time.

---

### What's NOT in v1

- **No `commit-msg` hook.** Commit message conventions from [15 — Version Control & Backup](15-version-control-backup.md) are suggestions, not rules. Enforcing them would be annoying for manual commits. Can be revisited as opt-in later.
- **No auto-update check.** `tsk session start` could warn about stale hooks, but that's a future enhancement.
- **No custom hook chaining.** If users need additional hooks, they can wrap the tsk hook in their own script. A hook chaining mechanism is out of scope for v1.

---

### Cross-references

- [06 — Issue File Format](06-issue-file-format.md) — frontmatter schema, required fields, `.REMOTE.md` convention
- [08 — CLI Design](08-cli-design.md) — `tsk hooks` subcommand listing
- [10 — Sync Architecture](10-sync-architecture.md) — `tsk sync pull` commits `.REMOTE.md` files with `RIPTASK_HOOK_ALLOW_REMOTE=1`
- [16 — Configuration](16-configuration.md) — `riptask.yaml` structure and uniqueness constraints
- [17 — Codebase Structure](17-codebase-structure.md) — `lib/hooks/pre-commit` and `lib/hooks.sh` file locations
- [22 — Testing](22-testing.md) — `tests/unit/hooks.bats` and `tests/integration/hooks.bats`
- [24 — Templates](24-templates.md) — template frontmatter schema, `default_priority` enum
