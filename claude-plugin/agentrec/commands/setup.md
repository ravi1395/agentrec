---
description: Install the agentrec binary (if missing) and wire this repo for recording — one step.
---

Set up agentrec recording for the current repository. Follow these steps in order:

1. **Check the binary.** Run `command -v agentrec && agentrec --version`. If present, skip to step 3.

2. **Install the binary** (pick whichever the user prefers; both verify a sha256 checksum):
   - `curl -fsSL https://raw.githubusercontent.com/ravi1395/agentrec/main/install.sh | sh` (installs to `~/.local/bin`, no sudo), or
   - `brew install ravi1395/agentrec/agentrec`

   If `~/.local/bin` is not on PATH, the installer prints the exact line to add — show it to the user.

3. **Initialize the repo WITHOUT repo-local hooks:**

   ```
   agentrec init --no-hook
   ```

   `--no-hook` is required, not optional: this plugin already provides the UserPromptSubmit/Stop hooks globally. Running bare `agentrec init` would add a second, repo-local copy of the same hooks and every turn signal would be emitted twice.

4. **Verify:** run `agentrec status`. Expect a healthy store line and no DEGRADED banner. If anything looks wrong, run `agentrec doctor` and report its output to the user.

5. Tell the user recording starts with the daemon: `agentrec init` installed a service unit (launchd/systemd) unless `--no-service` was passed; `agentrec status` shows daemon liveness. From here on, `agentrec log`, `agentrec diff`, `agentrec blame <file>[:line]`, and `agentrec undo` work against the recorded history — and `agentrec import claude` backfills attribution from existing Claude Code session transcripts.

Do not run `agentrec undo` or `agentrec purge` during setup — they are destructive verbs and belong to the user.
