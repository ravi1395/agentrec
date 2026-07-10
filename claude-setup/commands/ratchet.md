# /ratchet — spec-driven delivery loop

*A ratchet only tightens: each round clicks progress forward against the spec, and an acceptance criterion, once set, is never loosened to pass.*

Drive the goal in $ARGUMENTS (default: the "Next" item in CLAUDE.md Status) to completion against the spec suite. Loop until every in-scope acceptance criterion passes skeptical review. Doc authority order: PROTOCOL.md > IMPLEMENTATION.md decision register > SPEC.md (or the project's equivalent).

## Roles

- **Orchestrator (Opus — this session):** scopes, decomposes, dispatches, arbitrates. Writes no implementation code.
- **Implementers (`implementer` agent, Sonnet):** small scoped tasks, each naming its AC ids and test obligations.
- **Skeptic (`skeptic` agent, Opus, fresh context):** reviews code against ACs. Always a subagent, never inline — a reviewer who watched the implementation inherits its rationalizations.

## Loop

1. **SCOPE** — pick the milestone slice; list the exact AC ids in scope (e.g. M2 = F1–F4, G1–G6, H1–H7, L+, M+).
2. **PLAN** — decompose into implementer-sized tasks; show the plan; wait for approval on the first iteration of a new goal.
3. **IMPLEMENT** — dispatch implementers in parallel where files don't overlap. Build, tests, lint, and format checks green before review.
4. **REVIEW** — dispatch the skeptic with only: the AC ids in scope, the doc paths, and the changed files. No implementation narrative.
5. **ARBITRATE** — every FAIL/UNTESTED becomes a new task → back to 3. Never weaken an AC to pass; spec changes need explicit founder approval.
6. **CLOSE** — all in-scope ACs PASS → update CLAUDE.md Status (Done / Next), commit with AC ids in the message.
7. Repeat from 1. **Escalate to the founder when:** an AC is ambiguous, the same AC fails two consecutive reviews, or scope must change.

## Rules

- Nothing is "done" without the skeptic's PASS. An untested criterion is an unmet criterion.
- House rules apply: never delete (archive to `/archive`), append-only history, clarify before starting.
