# Spec Lifecycle Policy

Status: active (permanent)

This document governs how specifications in `docs/spec/` are managed throughout the tsk development lifecycle. It applies to both LLM agents and human contributors.

### 0.1 Source of Truth Invariant

Per [[02-philosophy-design-principles]] §2.4: one source of truth per concern. This invariant extends to specs themselves:

- **Pre-implementation:** The spec is the SoT for intent and behavior.
- **Post-implementation:** Code + tests become the SoT. The spec becomes historical record only.
- **Always:** Specs that describe "why" (philosophy, design principles, non-goals) remain authoritative permanently — they capture rationale that code cannot express.

Two documents must never describe the same behavioral truth simultaneously. When code implements what a spec describes, the spec yields authority.

### 0.2 Spec Status Taxonomy

Every spec progresses through these states:

| Status | Meaning |
|---|---|
| `draft` | Under development, not yet reviewed or accepted |
| `active` | Accepted as the current design intent |
| `implementing` | Active implementation in progress against this spec |
| `implemented` | Fully implemented in code and covered by tests |
| `archived` | Superseded, obsolete, or migrated to code+tests |

Permanent specs (philosophy, design principles, non-goals) remain `active` indefinitely — they do not transition to `implemented` because they describe rationale, not behavior.

### 0.3 Transition Rules

**draft → active:** Spec reviewed and accepted (PR merge or explicit approval).

**active → implementing:** Work begins on implementing the spec. The agent or developer references the spec in commits/PRs.

**implementing → implemented:** All behavioral requirements from the spec are covered by passing tests. The spec is no longer needed to understand what the system does — the code shows that.

**implemented → archived:** The spec is moved to `docs/spec/archive/` or marked with an `[ARCHIVED]` header. This happens when:
- The spec's behavioral content is fully captured by tests
- Keeping the spec in active rotation risks SoT divergence
- The spec has not been referenced in 3+ implementation cycles

### 0.4 What Stays as Spec Permanently

These specs describe "why" and "what not" — concerns that code and tests cannot express:

- [[01-problem-statement]] — The problem being solved
- [[02-philosophy-design-principles]] — Design rationale and invariants
- [[03-design-decisions]] — Key decisions with context
- [[19-non-goals]] — Explicit boundaries
- [[20-open-questions]] — Unresolved design questions

These remain `active` and are maintained as living documents.

### 0.5 What Migrates to Code + Tests

Behavioral specs describe "what" the system does. Once implemented, their truth moves to code:

- Issue file format → parser tests + format validation
- CLI design → integration tests + help text
- Sync architecture → sync engine tests
- View generation → view tests + cache logic

The spec served as input to implementation. Post-implementation, the test suite is the executable specification.

### 0.6 What Becomes ADRs

Key decisions with significant rationale should be extracted into Architecture Decision Records in `docs/adr/` (when that directory is established):

- Technology choices and their trade-offs
- Architectural patterns selected or rejected
- Decisions that future contributors will question without context

ADRs are immutable. When a decision is reversed, a new ADR supersedes the old one.

### 0.7 LLM Agent Workflow

When an agent implements a feature against a spec:

1. **Read the spec** — Load the relevant spec(s) from `docs/spec/` before starting implementation.
2. **Reference the spec** — Cite spec section numbers in commit messages and PR descriptions (e.g., "Implements §6.3 frontmatter parsing").
3. **Implement and test** — Write code and tests that capture the spec's behavioral requirements.
4. **Flag divergence** — If implementation requires deviating from the spec, document the deviation in the PR description and suggest a spec update.
5. **Suggest status update** — After implementation is complete and tests pass, suggest updating the spec's status.

Agents should not modify spec status unilaterally — propose the change for human review.

### 0.8 When to Suggest Archiving

An agent or contributor should suggest archiving a spec when:

- All behavioral requirements are covered by passing tests
- The spec has been in `implemented` status through at least one release cycle
- No open questions or draft sections remain in the spec
- Continued maintenance of the spec would create SoT divergence risk

The suggestion should include a checklist mapping spec sections to their corresponding test coverage.

### 0.9 Cross-References

- [[02-philosophy-design-principles]] — Foundation for §0.1 (SoT invariant) and all lifecycle decisions
- [[archive/17-codebase-structure]] — Where implementation artifacts live
- [[archive/18-implementation-roadmap]] — Sequencing of spec implementation
- [[archive/13-llm-agent-integration]] — How agents interact with the tsk system (complementary to §0.7 which covers how agents interact with specs)
