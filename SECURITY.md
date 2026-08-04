# Security policy

## Supported versions

The latest release on the [releases page](https://github.com/ravi1395/agentrec/releases) is supported. Older releases don't receive fixes — upgrade first.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting: <https://github.com/ravi1395/agentrec/security/advisories/new>. Please don't open a public issue for a security report.

## Scope

agentrec is a local-only tool: no network code exists in the current codebase (v1–v2 by design), so there is no service to attack — the surface is the binary, the daemon, and the files under `.agentrec/`.

Before reporting, read the **Threat model** section of the README. In particular: agentrec is built against accidents, not adversaries. Anything running under your uid can write `.agentrec/` directly, and the record is deliberately not tamper-evident against its own user — reports that reduce to "a process with my uid can modify my local files" are documented behavior, not vulnerabilities. Reports about secret leakage (scrub bypass, snapshot of a secret-pattern file), path traversal, symlink escape, or `undo` corrupting files it should not touch are very much in scope.
