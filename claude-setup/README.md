# Claude workflow setup — install instructions

Cowork sessions cannot write into `.claude/` directories (protected), so these files are staged here. Install with:

```bash
# Project-level (this repo):
mkdir -p .claude/commands .claude/agents .claude/skills
cp claude-setup/commands/ratchet.md .claude/commands/
cp claude-setup/agents/*.md .claude/agents/
cp -r claude-setup/skills/* .claude/skills/

# Global (all projects):
mkdir -p ~/.claude/commands ~/.claude/agents
cp claude-setup/commands/ratchet.md ~/.claude/commands/
cp claude-setup/agents/*.md ~/.claude/agents/

# Global CLAUDE.md: append the snippet
cat claude-setup/GLOBAL-CLAUDE-MD-SNIPPET.md >> ~/.claude/CLAUDE.md
```

Project-level files win over global when both exist. After installing, `/ratchet` is available in Claude Code, and the `implementer` (Sonnet) / `skeptic` (Opus) agents are dispatched by name with pinned models. (Named `/ratchet` because a ratchet only tightens — acceptance criteria are never loosened to pass.)

## Skills

| Skill | Purpose |
|---|---|
| `agentrec-memory` | Emit file-grounded memory candidates at the end of a substantive turn. |
| `agentrec-release` | Cut a release: bump all seven version sites in lockstep, gate, commit, tag, push. Overrides the global `ship` skill in this repo; registry publishing stays a human hand-off. |

`.claude/` is gitignored, so the installed copies are machine-local — `claude-setup/skills/` is the tracked source of truth. Re-copy after pulling changes to either.
