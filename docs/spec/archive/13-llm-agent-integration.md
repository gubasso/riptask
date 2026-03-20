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
> | AI features | `lib/tsk/ai.sh, tests/integration/ai.bats` |
>
> ---

# LLM / Agent Integration

AI features are **always opt-in**. If `ai.enabled: false` (the default) or no supported AI CLI (`claude` or `llm`) is installed, all commands degrade gracefully to their non-AI path. No hard failures.

### Practical integrations

**Issue body generation (`tsk new --ai`)**

A cheap, fast model (Haiku) pre-fills description, tasks checklist, and suggested labels from the title and project context. The editor opens with the pre-filled body for review and editing. One-shot generation, low-stakes.

**Triage on pull (`tsk sync pull --triage`)**

For each new issue arriving from remote, the model suggests `state`, `priority`, `labels`, `cycle`. Presented as a y/n confirmation per issue. `--auto-triage` skips confirmation and applies suggestions silently.

**Status summary (`tsk summarize`)**

Pipe all issue frontmatters + bodies for a project/cycle through the model and get a status summary. Useful for standup notes, weekly reports, or onboarding context.

```bash
tsk summarize --cycle 2026-Q1
# "3 in-progress, 2 blocked. WHL-042 (wormhole stabilizer) is oldest open item.
#  WHL-041 has no assignee. Nothing due this week except GH-008 on Friday."
```

**Freeform query (`tsk ask`)**

Natural language query over `issues/*.md`. The model receives relevant file contents as context.

```bash
tsk ask "what is blocking the IceOS 5.0 milestone?"
tsk ask "what did I close last week?"
tsk ask "which issues have no assignee?"
```

### Integration design

Single module: `lib/ai.sh`. One function, all AI features route through it.

```bash
ai_call() {
    local system_prompt="$1"
    local user_prompt="$2"
    local config_model
    config_model="$(yq '.ai.model // ""' "$TSK_REPO/tsk.yaml" 2>/dev/null || true)"
    local model="${TSK_AI_MODEL:-${config_model:-claude-haiku-4-5-20251001}}"

    # uses claude CLI, llm (simonw/llm), or direct API call
    # preference: claude CLI if available, then llm, then error
    if command -v claude &>/dev/null; then
        echo "$user_prompt" | claude --system "$system_prompt" --model "$model"
    elif command -v llm &>/dev/null; then
        echo "$user_prompt" | llm --system "$system_prompt" -m "$model"
    else
        log_error "No AI CLI found. Install claude or llm, or set ai.enabled: false."
        return 1
    fi
}
```

Model precedence is: `$TSK_AI_MODEL` environment variable > `ai.model` in `tsk.yaml` > hardcoded default.

### `tsk.yaml` AI config

```yaml
ai:
  enabled: false                    # opt-in, never on by default
  model: claude-haiku-4-5-20251001  # default model
  features:
    new_body_gen: true
    triage: true
    summarize: true
    ask: true
```
