<!-- Append this to ~/.claude/CLAUDE.md (your global instructions) -->

## Preferred coding workflow (/ratchet loop)

For any coding project with a spec: work in delivery rounds driven by the `/ratchet` command (a ratchet only tightens — acceptance criteria are never loosened to pass).

- **Orchestrator:** Opus. Plans and dispatches; writes no implementation code.
- **Implementation:** Sonnet subagents (`implementer` agent), each task scoped to named acceptance criteria with test obligations.
- **Review:** after every implementation round, an Opus `skeptic` subagent with fresh context reviews the code against the spec's acceptance criteria — PASS (with test cited) / FAIL / UNTESTED per criterion. Never review inline.
- **Loop:** failures become new tasks; repeat until all in-scope criteria pass. Never weaken a criterion to pass; escalate to me if a criterion is ambiguous or fails twice.
- **After every round or phase:** update the project's CLAUDE.md Status section — what is done against the roadmap, what is next — and commit.
