# Claude workflow setup — install instructions

Cowork sessions cannot write into `.claude/` directories (protected), so these files are staged here. Install with:

```bash
# Project-level (this repo):
mkdir -p .claude/commands .claude/agents
cp claude-setup/commands/ratchet.md .claude/commands/
cp claude-setup/agents/*.md .claude/agents/

# Global (all projects):
mkdir -p ~/.claude/commands ~/.claude/agents
cp claude-setup/commands/ratchet.md ~/.claude/commands/
cp claude-setup/agents/*.md ~/.claude/agents/

# Global CLAUDE.md: append the snippet
cat claude-setup/GLOBAL-CLAUDE-MD-SNIPPET.md >> ~/.claude/CLAUDE.md
```

Project-level files win over global when both exist. After installing, `/ratchet` is available in Claude Code, and the `implementer` (Sonnet) / `skeptic` (Opus) agents are dispatched by name with pinned models. (Named `/ratchet` because a ratchet only tightens — acceptance criteria are never loosened to pass.)
