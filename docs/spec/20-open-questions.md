# Open Questions

Status: active (permanent)

### Known edge cases to handle in implementation

**[RESOLVED] Cross-reference integrity on rename.**
When `LOCAL-3f2a` → `WHL-043`, scan all `$RIPTSK_REPO/issues/*.md` for references to `LOCAL-3f2a` in any frontmatter field (`blocks:`, `related:`, `parent:`) and update them atomically before git commit.

**[RESOLVED] Remote deletion.**
If an issue is deleted or transferred on the remote, do not silently remove the local file. Set `remote_deleted: true` in frontmatter, preserve the last known local `state`, and emit a warning. The user decides what to do with it.

**[RESOLVED] Self-hosted GitLab.**
Pass `host:` from `riptsk.yaml` remote entry to `glab` via `GITLAB_HOST` env variable or `--hostname` flag. Document the glab authentication setup for self-hosted instances.

**[RESOLVED] Multiple remotes, same numeric ID.**
Remote state cache key is always `<type>:<repo>:<issue_id>` — never just the numeric ID. `WHL-042` and `FSH-042` can coexist.

**[RESOLVED] Label conflict prevention.**
On push, only manage `status::*` scoped labels. Read current labels from remote before push to preserve non-status labels. Never do a full label replace.

**[RESOLVED] `tsk edit` save detection.**
After opening `$EDITOR`, `tsk` needs to know if the file was actually saved to update `local_updated_at` and regenerate views. Compare mtime before/after editor exits. If unchanged, skip update.

**`views/` path strategy — RESOLVED.**
~~Symlinks use relative paths (`../../../issues/WHL-042.md`). If the views directory depth changes (new view type added deeper), symlink paths must be recalculated.~~ Views now live in `$XDG_CACHE_HOME/riptsk/views/` and use **file copies** from `$RIPTSK_REPO/issues/<ID>.md`. Path depth is irrelevant for copies. If `$RIPTSK_REPO` moves, `tsk view` rebuilds everything.

**[RESOLVED] Issue body with YAML-like content.**
Issue bodies may contain YAML code blocks that confuse frontmatter parsers. `yq` operates only on the frontmatter block (between `---` delimiters) — never on the body. The `lib/frontmatter.sh` split/join must be reliable here.

**[RESOLVED] Stacked conflicts (already conflicted + new pull).**
If an issue already has `conflict:` set when a new pull arrives, skip it and warn. Never create `.REMOTE.REMOTE.md` — the user must resolve the existing conflict first via `tsk resolve`.

**[RESOLVED] Missing `remote_state.json` baseline.**
If the remote state cache is missing (first sync, cache cleared, new host), there is no baseline to detect conflicts. Fall back to last-write-wins (current behavior). After the pull completes, `remote_state.json` is populated and subsequent pulls can detect conflicts normally.

**[RESOLVED] User deletes `.REMOTE.md` without `tsk resolve`.**
The `conflict:` field in frontmatter is the authoritative conflict indicator, not the `.REMOTE.md` file. If the user deletes the `.REMOTE.md` file manually, push is still blocked (frontmatter still has `conflict:`). `tsk resolve` handles missing `.REMOTE.md` gracefully — it removes the `conflict:` field and updates `local_updated_at` regardless.

**[RESOLVED] `.REMOTE.md` files in `$RIPTSK_REPO` git.**
`.REMOTE.md` files are committed to `$RIPTSK_REPO` so conflict state survives host switches and `$RIPTSK_REPO` git pull/push. They are excluded from views and `tsk ls` by filtering on the `.REMOTE.md` suffix.

**[RESOLVED] Recurring task `last_run` merge conflict.**
When `riptsk.yaml` is merged across hosts, `last_run` fields may conflict if both hosts ran `tsk recur run` independently. Resolution: keep the later date. The dedup check in `tsk recur run` (title pattern match) prevents duplicate issues even if `last_run` is stale.

**`index.json` cache — DEFERRED.** The index.json cache described in the original data repository spec was never implemented. Query operations work directly against issue files via frontmatter parsing. This is adequate for the current scale and may be revisited if performance requires it.

**[RESOLVED] Cache reconstruction on new host.**
When setting up a new host, `~/.cache/riptsk/` is empty. `remote_state.json` is missing, so the first `tsk sync pull` falls back to last-write-wins. After the first pull completes, the cache is populated and subsequent pulls use conflict detection. `id_map.json` cannot be fully reconstructed from `issues/*.md` alone because `local_id` is cleared after first push. However, the map is only needed for in-flight renames — once an issue has its permanent ID, the mapping is no longer referenced. On a new host, `id_map.json` starts empty; any in-flight `LOCAL-*` issues will be re-pushed and re-renamed normally. There is no `index.json` cache to reconstruct at present; query operations read issue files directly.
