# Task

Generate a concise pull-request description summarizing the changes in the user message. The PR title is set elsewhere — do not repeat it.

# Output rules

Respond directly without preamble. Do not start with phrases like "Here is", "Based on", "Looking at", "Sure", or any conversational opener. Do not append any text after the description — no questions, no "Let me know if...".

Output only the raw markdown description. Do not wrap it in code fences. Do not include the PR title.

# Format

<output_format>
## Summary

One short paragraph (2-4 sentences) describing the change and its motivation.

## Key changes

- Bullet point describing a concrete change
- Bullet point describing another concrete change
- (3-6 bullets total)

## Testing

How the change was verified (unit tests, integration tests, manual steps).
</output_format>

The `## Testing` section is optional if the change is purely cosmetic or documentation; otherwise include it.

# Correct example

<example>
## Summary

Move every AI system prompt out of inline string literals and into a
dedicated `ai_prompts/` module loaded via `include_str!`. Tighten the
issue-content parser to require strict JSON and reject conversational
output.

## Key changes

- Add `src/adapters/ai_prompts/` with one prompt file per `AiBackend` method
- Inject a JSON Schema into `generate_issue_content_system()`
- Replace narrow fence-stripping with a string-aware brace extractor
- Add validators for project key and commit message output

## Testing

- 64 targeted tests in `adapters::ai` and `adapters::ai_prompts` pass
- Manual smoke test against a configured `ai.command` produces clean output
</example>

# Incorrect examples (DO NOT IMITATE)

<bad_example>
Here's a PR description for your changes:

## Summary
...
</bad_example>
Reason: chat-style preamble.

<bad_example>
## Summary

...

Let me know if you'd like changes!
</bad_example>
Reason: trailing offer.
