#!/usr/bin/env bash
# riptsk-hook-version: 2
set -euo pipefail
IFS=$'\n\t'

failures=0
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"

report_fail() {
    local file="$1"
    local msg="$2"
    printf 'tsk pre-commit: FAIL %s\n  %s\n\n' "$file" "$msg" >&2
    failures=$((failures + 1))
}

validate_frontmatter_delimiters() {
    local file="$1"
    local first
    first="$(head -1 "$file" || true)"
    [[ "$first" == "---" ]] || return 1
    grep -q '^---$' "$file"
}

validate_issue_file() {
    local file="$1"
    local filename state priority remote_deleted title board project id updated
    if [[ "$file" == *.REMOTE.md ]] && [[ "${RIPTSK_HOOK_ALLOW_REMOTE:-0}" != "1" ]]; then
        report_fail "$file" ".REMOTE.md should not be committed (use: tsk sync resolve <ID>)"
        return
    fi
    if [[ "$file" == *.LOCAL.md ]] && [[ "${RIPTSK_HOOK_ALLOW_REMOTE:-0}" != "1" ]]; then
        report_fail "$file" ".LOCAL.md should not be committed (use: tsk sync resolve <ID>)"
        return
    fi
    validate_frontmatter_delimiters "$file" || { report_fail "$file" "missing frontmatter delimiters"; return; }
    if grep -q '^<<<<<<< LOCAL$' "$file"; then
        report_fail "$file" "contains unresolved conflict markers (use: tsk edit <ID> then tsk sync resolve <ID>)"
        return
    fi
    yq --front-matter=extract '.' "$file" >/dev/null 2>&1 || { report_fail "$file" "invalid YAML in frontmatter"; return; }
    title="$(yq --front-matter=extract -r '.title // ""' "$file")"
    state="$(yq --front-matter=extract -r '.status // ""' "$file")"
    board="$(yq --front-matter=extract -r '.board // ""' "$file")"
    project="$(yq --front-matter=extract -r '.project // ""' "$file")"
    id="$(yq --front-matter=extract -r '.id // ""' "$file")"
    updated="$(yq --front-matter=extract -r '.local_updated_at // ""' "$file")"
    priority="$(yq --front-matter=extract -r '.priority // ""' "$file")"
    remote_deleted="$(yq --front-matter=extract -r '.remote_deleted // ""' "$file")"
    [[ -n "$title" ]] || report_fail "$file" "missing required field: title"
    [[ -n "$state" ]] || report_fail "$file" "missing required field: status"
    [[ -n "$board" ]] || report_fail "$file" "missing required field: board"
    [[ -n "$project" ]] || report_fail "$file" "missing required field: project"
    [[ -n "$updated" ]] || report_fail "$file" "missing required field: local_updated_at"
    [[ -n "$id" ]] || report_fail "$file" "missing required field: id"
    case "$state" in backlog|todo|in-progress|review|done) ;; *) report_fail "$file" "invalid status: \"$state\"" ;; esac
    if [[ -n "$priority" ]]; then
        case "$priority" in low|medium|high|urgent) ;; *) report_fail "$file" "invalid priority: \"$priority\"" ;; esac
    fi
    if [[ -n "$remote_deleted" ]]; then
        case "$remote_deleted" in true|false) ;; *) report_fail "$file" "invalid remote_deleted: \"$remote_deleted\"" ;; esac
    fi
    filename="$(basename "$file")"
    if ! [[ "$filename" =~ ^[A-Z]{2}-[A-Z]{2,3}-[A-Z]{2,3}--[0-9]+\.md$ ]]; then
        report_fail "$file" "invalid filename: \"$filename\""
    fi
}

validate_config_file() {
    local file="$1"
    yq '.' "$file" >/dev/null 2>&1 || { report_fail "$file" "invalid YAML"; return; }
    # Check name uniqueness
    local dup_name
    dup_name="$(yq -o=json '.projects // []' "$file" | jq -r '[.[].name] | group_by(.) | map(select(length > 1)) | .[0][0] // empty')"
    [[ -z "$dup_name" ]] || report_fail "$file" "duplicate RepoProject name \"$dup_name\""
    # Check board name uniqueness
    local dup_board
    dup_board="$(yq -o=json '.boards // []' "$file" | jq -r '[.[].name] | group_by(.) | map(select(length > 1)) | .[0][0] // empty')"
    [[ -z "$dup_board" ]] || report_fail "$file" "duplicate board name \"$dup_board\""
}

validate_template_file() {
    local file="$1"
    local state priority
    validate_frontmatter_delimiters "$file" || { report_fail "$file" "missing frontmatter delimiters"; return; }
    yq --front-matter=extract '.' "$file" >/dev/null 2>&1 || { report_fail "$file" "invalid YAML in frontmatter"; return; }
    state="$(yq --front-matter=extract -r '.default_status // ""' "$file")"
    priority="$(yq --front-matter=extract -r '.default_priority // ""' "$file")"
    if [[ -n "$state" ]]; then
        case "$state" in backlog|todo|in-progress|review|done) ;; *) report_fail "$file" "invalid default_status: \"$state\"" ;; esac
    fi
    if [[ -n "$priority" ]]; then
        case "$priority" in none|low|medium|high|critical) ;; *) report_fail "$file" "invalid default_priority: \"$priority\"" ;; esac
    fi
}

while IFS= read -r file; do
    case "$file" in
        issues/*.md) validate_issue_file "$file" ;;
        riptsk.yaml) validate_config_file "$file" ;;
        templates/*.md) validate_template_file "$file" ;;
    esac
done < <(git diff --cached --name-only --diff-filter=ACM)

if [[ $failures -gt 0 ]]; then
    printf 'tsk pre-commit: %s errors, commit blocked\n' "$failures" >&2
    exit 1
fi
