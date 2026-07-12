<!-- Staged, not installed: cp -r claude-setup/skills/agentrec-memory ~/.claude/skills/ (or .claude/skills/ for project-level), same as claude-setup/commands/ratchet.md. -->
---
name: agentrec-memory
description: Emit durable, file-grounded memory candidates at the end of a substantive agentrec-tracked turn.
---

# agentrec memory candidates

At the END of a substantive turn (real code/config change, not a trivial
read), run 0–3 times:

```bash
agentrec candidate "<durable, non-obvious, file-grounded fact>" --from <path1>[,<path2>,...]
```

## Hard rules

- **File-groundable only.** A candidate must be about specific files you can
  name in `--from`. If you can't point at the file(s), don't emit it.
- **No narration.** A candidate is a durable *learning* future sessions need,
  never a changelog line describing what this turn did.
- **≤ 3 per turn.** Most turns emit 0. Emit only what will still matter next
  month.
- Nothing else to do: `agentrec candidate` scrubs and validates; the daemon
  hashes pins and dedups. No need to check exit codes or retry.

## Good (durable, file-grounded)

```bash
agentrec candidate "retry logic in http_client.rs assumes idempotent POSTs — non-idempotent callers must set skip_retry" --from src/http_client.rs
agentrec candidate "config.toml's mcp_destructive gate defaults to off; tests assume that default" --from config.toml,cli/tests/integration.rs
```

## Bad (narration or ungrounded — never emit these)

```bash
agentrec candidate "fixed the bug in the parser" --from src/parser.rs        # narration, not a fact
agentrec candidate "the codebase generally prefers small functions" --from  # no --from, not file-grounded
```
