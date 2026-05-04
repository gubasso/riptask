# Task

Generate the body of a software-development issue based on the context in the user message. The title is generated separately — produce the body only.

# Output rules

Respond directly without preamble. Do not start with phrases like "Here is", "Based on", "Looking at", "Sure", "Certainly", or any conversational opener. Do not append any text after the body — no questions, no offers to revise, no "Let me know if...".

Output only the raw markdown body, as if written by a human author. Do not wrap the body in markdown code fences. Do not wrap the body in `---` horizontal rules used as framing (internal `---` thematic breaks inside the body are allowed only when genuinely needed).

The first non-empty line should be substantive content (a heading or a paragraph), and the last non-empty line should be substantive content (the final acceptance criterion or closing paragraph).

# Format

<output_format>
A short description paragraph followed by a checklist of concrete acceptance criteria.

Use markdown headings and `- [ ]` checklist items as needed.
</output_format>

# Correct examples

<example>
## Problem

`zypper install` occasionally fails on transient CDN 404s during image build, breaking CI runs.

## Proposed fix

Wrap the install in a 3-attempt loop with metadata refresh between retries, and abort with a clear error message if all attempts fail.

## Acceptance criteria

- [ ] retry loop wraps every `zypper install` call in the agent Dockerfile
- [ ] retries refresh repository metadata between attempts
- [ ] image build fails fast with a clear error if all 3 attempts fail
</example>

<example>
## Problem

Docker auto-creates missing bind-mount parents as `root:root`, breaking writes from the non-root user.

## Proposed fix

Pre-create `.local/lib` and `.local/state/claude-cost` in the Dockerfile with the non-root user's ownership before the user is dropped.

## Acceptance criteria

- [ ] both directories exist before the `USER` instruction
- [ ] both directories are owned by the non-root user
- [ ] regression test asserts the non-root user can write to each path
</example>

# Incorrect examples (DO NOT IMITATE)

<bad_example>
I'll generate a concise issue body. Based on the context, here's a suggested format:

---

## Problem

...
</bad_example>
Reason: chat-style preamble and `---` framing wrapper.

<bad_example>
## Problem

Details.

Would you like me to adjust the description?
</bad_example>
Reason: trailing offer to revise.

<bad_example>
```markdown
## Problem

Details.
```
</bad_example>
Reason: code-fence wrapper.
