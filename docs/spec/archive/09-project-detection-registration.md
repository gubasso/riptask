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
> | Project detection | `lib/tsk/detect.sh, tests/unit/detect.bats` |
>
> ---

# Project Detection & Registration

### Mental model

- `$RIPTSK_REPO` (`~/.local/share/riptsk/`) is a **passive database**. It is never the working directory for `tsk` commands.
- Commands are run from **inside project repos** (`~/code/project-a/`, etc.)
- `tsk` reads `git remote get-url origin` from `$PWD` to determine the project context
- All `tsk` state lives in `$RIPTSK_REPO` — nothing is written back to project repos

### Detection flow

On every `tsk` command that requires a project context:

```
1. git -C $PWD remote get-url origin
   ├── failure (not in a git repo) → error or --project flag required
   └── failure (git repo, no remote) → local project (no sync)

2. normalize URL to host:slug
   git@gitlab.penguin-labs.io:chrono/wormhole-router.git
     → host: gitlab.penguin-labs.io
     → slug: chrono/wormhole-router

3. infer remote type from host:
   ├── host contains "github.com"  → type: github (SoT: gh)
   ├── host contains "gitlab"      → type: gitlab (SoT: glab)
   └── anything else               → type: local  (SoT: $RIPTSK_REPO)
       (codeberg, gitea, gitolite, self-hosted, etc.)

4. look up project in $RIPTSK_REPO/riptsk.yaml remotes[]
   ├── remote-backed repo  → match normalized host:slug against remotes[].repo
   ├── no-remote repo      → match realpath($PWD) against remotes[].path
   └── not found           → trigger registration flow
```

A **project** is any git repository — with or without a remote. A **remote** is the remote counterpart of a project. Remote type determines the source of truth:

| Scenario | SoT | Sync mechanism |
|---|---|---|
| GitHub remote | `gh` | `tsk sync` via `gh` CLI |
| GitLab remote | `glab` | `tsk sync` via `glab` CLI |
| Other remote (codeberg, gitolite…) | local | `$RIPTSK_REPO` git only |
| No remote | local | `$RIPTSK_REPO` git only |

For `type: local` projects, there is no bidirectional sync against a remote issue tracker. Issues are managed entirely within `$RIPTSK_REPO` and shared across hosts via `$RIPTSK_REPO` git push/pull. IDs are permanent (not provisional) since there is no remote to assign a "real" ID.

### First-run registration

When a project is not yet in `riptsk.yaml`, `tsk` prompts for configuration and registers it. The remote type is auto-detected from the host — not prompted:

```
$ cd ~/code/wormhole-router
$ tsk new --title "fix wormhole stabilizer"

Project not registered: gitlab.penguin-labs.io/chrono/wormhole-router
Detected type: gitlab
Let's set it up.

Issue prefix (e.g. WHL, FSH): WHL
Default board: [personal] penguin-chrono-labs
Default org (optional): penguin-chrono-labs

Registered. Continuing with tsk new...
```

For non-GitHub/GitLab remotes, the type is `local`:

```
$ cd ~/code/ice-shelf-tracker
$ tsk new --title "add feature X"

Project not registered: codeberg.org/ppuffin/ice-shelf-tracker
Detected type: local (no gh/glab sync)
Let's set it up.

Issue prefix (e.g. ICE): ICE
Default board: [personal] personal
Default org (optional):

Registered. Continuing with tsk new...
```

For git repos with no remote:

```
$ cd ~/code/penguin-scratch
$ tsk new --title "prototype idea"

No remote detected. Registering as local project.
Let's set it up.

Project name: penguin-scratch
Issue prefix (e.g. WAD): WAD
Default board: [personal] personal
Default org (optional):

Registered. Continuing with tsk new...
```

### Prefix uniqueness validation

`project_prefix` must be unique across all registered remotes. Since prefixes are the namespace that prevents ID collisions between projects (e.g. `WHL-042` vs `FSH-042`), duplicate prefixes would make IDs ambiguous.

Validation is enforced at two points:

1. **Interactive registration** — after the user enters a prefix, check it against all existing `remotes[].project_prefix` values. If taken, reject immediately:

```
Issue prefix (e.g. WHL, FSH): WHL
Error: prefix "WHL" is already used by remote "wormhole-router".
Issue prefix (e.g. WHL, FSH): _
```

2. **Config load** — on every `tsk` invocation that parses `riptsk.yaml`, validate that all `project_prefix` values are unique. If a duplicate is found (e.g. from a manual edit or git merge), abort with a clear error:

```
Error: duplicate project_prefix "WHL" in riptsk.yaml (remotes "wormhole-router" and "other-project").
Fix riptsk.yaml before continuing.
```

Writes to `$RIPTSK_REPO/riptsk.yaml`:

```yaml
# GitLab remote — syncs via glab
remotes:
  - name: wormhole-router
    type: gitlab
    host: https://gitlab.penguin-labs.io
    repo: chrono/wormhole-router
    project_prefix: WHL
    default_board: penguin-chrono-labs
    default_org: penguin-chrono-labs

# Non-GitHub/GitLab remote — local task management only
  - name: ice-shelf-tracker
    type: local
    host: https://codeberg.org
    repo: ppuffin/ice-shelf-tracker
    project_prefix: ICE
    default_board: personal
    default_org: ~

# No remote — local task management only
  - name: penguin-scratch
    type: local
    repo: ~
    path: /home/ppuffin/code/penguin-scratch
    project_prefix: WAD
    default_board: personal
    default_org: ~
```

### URL normalization

`tsk` normalizes remote URLs from all common forms to `host:slug` for matching:

```
git@gitlab.penguin-labs.io:chrono/project.git    → gitlab.penguin-labs.io:chrono/project
https://gitlab.penguin-labs.io/chrono/project    → gitlab.penguin-labs.io:chrono/project
https://gitlab.penguin-labs.io/chrono/project/   → gitlab.penguin-labs.io:chrono/project
```

Normalization and type inference are both in `lib/detect.sh`. A single string comparison against the stored slug is sufficient — no need to enumerate URL variants in `riptsk.yaml`.

### Type inference rules

After normalizing the remote URL to `host:slug`, type is inferred from the host:

```
host == "github.com"           → type: github
host contains "gitlab"         → type: gitlab  (covers gitlab.com, gitlab.penguin-labs.io, etc.)
host is anything else          → type: local   (codeberg.org, gitea.*, gitolite, etc.)
no remote at all               → type: local
```

The inferred type is shown during registration for confirmation but is not prompted. The `type` field is stored in `riptsk.yaml` and used for all subsequent operations. If the heuristic is wrong (e.g. a self-hosted GitHub Enterprise on a non-`github.com` domain), the user can override `type` manually in `riptsk.yaml`.

### No-remote project lookup

Projects with no git remote cannot be re-detected from a URL, so `tsk` stores the repo's absolute path in a `path` field when registering them:

```yaml
- name: penguin-scratch
  type: local
  repo: ~
  path: /home/ppuffin/code/penguin-scratch
  project_prefix: WAD
  default_board: personal
  default_org: ~
```

On subsequent runs inside a no-remote repo, `tsk` resolves `realpath("$PWD")` and compares it to `remotes[].path`. This gives no-remote projects a deterministic lookup key without relying on `name` or directory basename heuristics.

### Commands outside a project repo

Some commands are project-agnostic and work from anywhere:

- `tsk ls --all`
- `tsk board --all`
- `tsk view`
- `tsk session start/end`
- `tsk recur run`
- `tsk config`

Project-scoped commands outside a git repo require `--project <name>` or `--board <name>` to be explicit.
