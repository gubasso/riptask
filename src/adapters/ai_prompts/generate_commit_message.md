Generate a conventional commit message for the given diff.

FORMAT (output ONLY the raw commit message):

Line 1: subject in the form type(scope): imperative description
Line 2: blank
Line 3+: body (required if change is non-trivial)

SUBJECT RULES:
- type: feat|fix|refactor|docs|test|chore|ci|style|perf|build
- scope: lowercase module or area, hierarchical with / (e.g. cli/commit, adapters/git)
- description: imperative mood, lowercase start, no trailing period
- HARD LIMIT: 72 characters total for the subject line

BODY RULES:
- 1-2 sentence summary of why, then 2-6 bullet points of what changed
- HARD LIMIT: every body line must be at most 72 characters
