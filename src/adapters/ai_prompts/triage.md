# Task

Suggest triage values (status, priority, labels) for the issue described in the user message.

# Output rules

Respond directly without preamble. Do not start with phrases like "Here is", "Based on", "Looking at", "Sure", "Certainly", or any conversational opener. Do not append any text after the JSON object — no explanations, no follow-up questions.

Output exactly one JSON object. Do not wrap it in code fences. Do not add a `json` language tag. The very first character of your response must be `{` and the very last character must be `}`.

# Schema

The output must validate against the JSON Schema injected at the end of this prompt under `<output_schema>`. Field constraints (also enforced by the schema):

- `status` — one of `"todo" | "doing" | "blocked" | "done" | null`. Use `null` if no clear suggestion applies.
- `priority` — one of `"low" | "medium" | "high" | "critical" | null`. Use `null` if no clear suggestion applies.
- `labels` — an array of short string labels. May be empty. Each label is at most 64 characters. At most 10 items.

The object must contain ONLY these three keys. Do not add `notes`, `summary`, `reasoning`, or any other field.

# Correct examples

<example>
{"status":"todo","priority":"high","labels":["bug","auth"]}
</example>

<example>
{"status":"doing","priority":"medium","labels":["refactor"]}
</example>

<example>
{"status":null,"priority":null,"labels":[]}
</example>

# Incorrect examples (DO NOT IMITATE)

<bad_example>
Here is the triage:
{"status":"todo","priority":"high","labels":["bug"]}
</bad_example>
Reason: prose preamble.

<bad_example>
{"status":"todo","priority":"high","labels":["bug"],"notes":"Looks important"}
</bad_example>
Reason: extra field `notes`.

<bad_example>
{"status":"in-progress","priority":"urgent","labels":["bug"]}
</bad_example>
Reason: `status` and `priority` use values outside the allowed enums.
