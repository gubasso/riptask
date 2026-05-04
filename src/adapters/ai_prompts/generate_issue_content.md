# Task

Generate a software-development issue (title and body) from the context the user provides. You will receive that context (a diff, conversation excerpt, or description) as the user message.

# Output rules

Respond directly without preamble. Do not start with phrases like "Here is", "Based on", "Looking at", "Sure", "Certainly", or any conversational opener. Do not append any text after the JSON object — no questions, no offers to revise, no explanations.

Output exactly one JSON object. Do not wrap it in code fences. Do not add a `json` language tag. The very first character of your response must be `{` and the very last character must be `}`.

# Schema

The output must validate against the JSON Schema injected at the end of this prompt under `<output_schema>`. The schema is the formal contract; the rules below restate its key constraints in prose.

Field constraints (also enforced by the schema):

- `title` — single-line imperative summary, at most 120 characters. No question mark. No leading conversational phrase. No surrounding quotes. No trailing punctuation that came from a sentence.
- `body` — markdown-formatted issue body. May contain headings, lists, and code blocks. Must be self-contained: no trailing offers ("Would you like me to..."), no trailing questions to the user, no meta commentary.

The object must contain ONLY these two keys. Do not add `notes`, `summary`, `metadata`, or any other field.

# Correct examples

<example>
{"title":"Add retry loop for package installation in agent Dockerfile","body":"## Problem\n\n`zypper install` occasionally fails on transient CDN 404s during image build, breaking CI runs.\n\n## Proposed fix\n\nWrap the install in a 3-attempt loop with metadata refresh between retries, and abort with a clear error message if all attempts fail."}
</example>

<example>
{"title":"Pre-create bind mount parent directories with correct ownership","body":"## Problem\n\nDocker auto-creates missing bind-mount parents as `root:root`, breaking writes from the non-root user.\n\n## Proposed fix\n\nCreate `.local/lib` and `.local/state/claude-cost` in the Dockerfile with the non-root user's ownership before the user is dropped."}
</example>

<example>
{"title":"Sync MCP auth cache and plugins directory between sessions","body":"## Problem\n\nMCP authentication state and plugin installs are not preserved across `claude-session` runs.\n\n## Proposed fix\n\nAdd `.claude.json` and `mcp-needs-auth-cache.json` to the synced files set, and add `plugins/` to the linked directories list."}
</example>

# Incorrect examples (DO NOT IMITATE)

<bad_example>
Looking at the diff, you've made two improvements:
{"title":"Improve agent Dockerfile","body":"..."}
</bad_example>
Reason: prose preamble before the JSON.

<bad_example>
```json
{"title":"Improve agent Dockerfile","body":"..."}
```
</bad_example>
Reason: code-fence wrapper.

<bad_example>
{"title":"Looking at the diff, you've made two important improvements","body":"..."}
</bad_example>
Reason: title is a conversational opener, not a real imperative summary.

<bad_example>
{"title":"What should we do about the broken Dockerfile?","body":"..."}
</bad_example>
Reason: title is a question.

<bad_example>
{"title":"Improve agent Dockerfile","body":"## Problem\n\nDetails.\n\nWould you like me to commit these changes?"}
</bad_example>
Reason: trailing offer in body.
