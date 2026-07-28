# Synthetic `import claude` fixture corpus

All content in this directory tree is hand-authored and synthetic. **No bytes were copied
from any real `~/.claude/projects` transcript on this machine.** This corpus exists to give
the P1 `import claude --dry-run` resolver (`cli/src/importcmd.rs`) something independent to
be tested against; the person authoring these fixtures did not read or design that resolver's
implementation (see P1.md, Pinned decision 7).

Each scenario directory below is a self-contained `--source`-style root: it has its own
`projects/` (and, where needed, a sibling `file-history/`) so a test can point `--source` at
exactly one scenario directory (Pinned decision 1).

---

## 1. `t1_and_t15/`

**Proves:** AC4 — T1 (inline `originalFile`) and T1.5 (`file-history` blob via
`snapshot.trackedFileBackups`) resolution, both within one session.

- Session id: `a1111111-1111-4111-8111-111111111111`
- File: `projects/proj-a/a1111111-1111-4111-8111-111111111111.jsonl`, 3 lines, `cwd:
  "/fake/repo"` on every line:
  1. A `snapshot` line recording `trackedFileBackups["/fake/repo/notes.txt"].backupFileName =
     "aa11bb22cc33dd44@v1"`.
  2. An `Edit` result for `/fake/repo/notes.txt` — **no `originalFile`** — this is the T1.5
     case; the resolver must fetch pre-edit bytes from the file-history blob below, not from
     this line.
  3. An `Edit` result for `/fake/repo/config.json` — **`originalFile` present inline** — this
     is the T1 case.
- File-history blob: `file-history/a1111111-1111-4111-8111-111111111111/aa11bb22cc33dd44@v1`
  — the T1.5 known-good pre-edit content for `notes.txt`, verbatim.

### Known-good content + independently-computed hashes

**T1.5 — `notes.txt` pre-edit content** (this exact byte sequence is the fixture file itself:
`file-history/a1111111-1111-4111-8111-111111111111/aa11bb22cc33dd44@v1`):

```
line one
line two
```

(two lines, each newline-terminated, no trailing blank line beyond the final `\n`)

Computed directly against the fixture file, out-of-band, before any importer code existed:

```
$ shasum -a 256 cli/tests/fixtures/import/claude/t1_and_t15/file-history/a1111111-1111-4111-8111-111111111111/aa11bb22cc33dd44@v1
e9024f1a07d29d52ad3aa5e1a18e94db1f3a9fd32b89e39d47c472cd99071e13  cli/tests/fixtures/import/claude/t1_and_t15/file-history/a1111111-1111-4111-8111-111111111111/aa11bb22cc33dd44@v1
```

**Expected `sha256(resolved before bytes)` for the `notes.txt` T1.5 entry:**
`e9024f1a07d29d52ad3aa5e1a18e94db1f3a9fd32b89e39d47c472cd99071e13`

**T1 — `config.json`'s `toolUseResult.originalFile` content** (embedded as a JSON string in
line 3 of the transcript; the raw bytes it decodes to are):

```
{
  "setting": "old_value"
}
```

(JSON object, 2-space indent, trailing newline)

Computed by writing that exact content to a standalone temp file and hashing it — independent
of how the importer will later extract/unescape the JSON string field:

```
$ printf '{\n  "setting": "old_value"\n}\n' > /tmp/config_original.json && shasum -a 256 /tmp/config_original.json
366a38a17f288f5fce4ed5d4f2e5a06c42152d29417473a91666d8ac1cac6b43  /tmp/config_original.json
```

**Expected `sha256(resolved before bytes)` for the `config.json` T1 entry:**
`366a38a17f288f5fce4ed5d4f2e5a06c42152d29417473a91666d8ac1cac6b43`

(Cross-check: decoding the transcript's JSON string for `toolUseResult.originalFile` in line 3
via `python3 -c 'import json; print(json.loads(open(...).readlines()[2])["toolUseResult"]["originalFile"])'`
and hashing the decoded string reproduces the same `366a38a1...` digest — confirmed during
fixture authoring.)

---

## 2. `t2_t3_candidate/`

**Proves:** T2-candidate vs T3 classification (detection only — no byte resolution for T2 in
this phase, per P1.md "T2 is detected, not resolved").

- Session id: `b2222222-2222-4222-8222-222222222222`
- File: `projects/proj-a/b2222222-2222-4222-8222-222222222222.jsonl`, 2 lines, both `cwd:
  "/fake/repo2"`, neither has `originalFile` nor a matching `snapshot`/`trackedFileBackups`
  entry (T1 and T1.5 are both ruled out for both lines):
  1. Edits `/fake/repo2/tracked.txt` — **intended to become T2-candidate**.
  2. Edits `/fake/repo2/untracked.txt` — **intended to become T3 (`None`)**.

**IMPORTANT — for whoever writes the integration test against this fixture:** you must
`git init` a repo at a tempdir standing in for `/fake/repo2`, then create and commit
`tracked.txt` there so the classifier's git-tracked-path check finds it tracked. Leave
`untracked.txt` untracked (or absent) in that same tempdir git repo — that absence is what
makes it classify as T3, not T2-candidate.

No `file-history/` directory is included in this scenario — there is no T1.5 case here.

---

## 3. `opaque/`

**Proves:** a session with zero file entries but a legitimate opaque tool call (no `filePath`
in `toolUseResult`) still counts "importable" per the Pinned decision 3 predicate (opaque
calls are legitimate, not failures).

- Session id: `c3333333-3333-4333-8333-333333333333`
- File: `projects/proj-a/c3333333-3333-4333-8333-333333333333.jsonl`, 1 line: a `Bash`-shaped
  `toolUseResult` (`stdout`/`stderr`/`interrupted`, **no `filePath`**), `cwd:
  "/fake/repo3"` present. No edits anywhere in the file.

---

## 4. `sidechain/`

**Proves:** AC5 — dual exclusion signal (path **and** field), each sufficient alone.

- Session id: `d4444444-4444-4444-8444-444444444444`
- Top-level file: `projects/proj-a/d4444444-4444-4444-8444-444444444444.jsonl`, 2 lines, `cwd:
  "/fake/repo4"` on every line:
  1. A plain user prompt line (no file entry).
  2. An `Edit` result for `/fake/repo4/README.md`, **`originalFile` set** (so it has bytes
     that would resolve at T1 if it were not excluded) **and `isSidechain: true`** — this is
     the field-signal exclusion case. Deliberately the *only* file-producing line in the
     top-level file, so the expected turn count is unambiguous.
- Sidechain file: `projects/proj-a/d4444444-4444-4444-8444-444444444444/subagents/agent-x1.jsonl`
  — a separate file, 2 lines, `cwd: "/fake/repo4"` present, one plain `Edit` result for
  `/fake/repo4/helper.py` with `originalFile` set. **These lines deliberately do NOT set
  `isSidechain: true`** — exclusion here must come from the literal path segment
  `/subagents/` alone (Pinned decision 2), which is the point of this half of the test.

**Assertion whoever writes the integration test must check:** BOTH of these must produce
**zero top-level turns** — `sessionD`'s top-level file yields zero turns because its one
file-producing line is `isSidechain: true` (field-signal exclusion), and
`subagents/agent-x1.jsonl` yields zero turns because it is excluded **by path alone**
(`/subagents/` segment) even though neither of its lines sets `isSidechain: true`
(path-signal exclusion). Each signal is independently sufficient (Pinned decision 2); this
fixture exercises the field signal in the top-level file and the path signal in the subagent
file, so both must be proven for AC5 to hold.

---

## 5. `malformed/`

**Proves:** AC6 — a file whose every line is malformed JSON reports one skip per line, exits
0, and does not abort the run or other files.

- File: `projects/proj-a/broken.jsonl`, 5 lines, **every single line is syntactically invalid
  JSON** (verified with `json.loads` in Python — all 5 raise `JSONDecodeError`):
  1. Unbalanced brace (opening `{` never closed).
  2. Truncated array (`"content": [}` — mismatched bracket/brace).
  3. Stray non-JSON text with no braces at all.
  4. Unbalanced brace (opening `{` never closed) on an otherwise plausible-looking tool-result
     line.
  5. Bare `{{{{` garbage.
- No valid JSON appears anywhere in this file.

---

## 6. `missing_cwd/`

**Proves:** AC8's `cwd` branch — a transcript missing the required `cwd` field entirely.

- Session id used inside the lines: `f6666666-6666-4666-8666-666666666666`
- File: `projects/proj-a/no-cwd.jsonl`, 3 lines, all valid JSON, all carry `sessionId` and
  (lines 2–3) a normal `toolUseResult` with `filePath`/`originalFile` — otherwise unremarkable
  edit entries. **No line in the entire file has a `cwd` field at all** (confirmed: `grep -c
  '"cwd"' no-cwd.jsonl` → `0`).

---

## 7. `missing_touluseresult/`

**Proves:** AC8's `toolUseResult` branch — schema drift where a line is structurally a
tool-result-shaped turn but the `toolUseResult` key itself is absent.

- Session id: `e7777777-7777-4777-8777-777777777777`
- File: `projects/proj-a/no-tur.jsonl`, 3 lines, `cwd: "/fake/repo7"` on every line:
  1. An `assistant` line issuing a `tool_use` (`Edit`, id `toolu_0000000070`) — normal, no
     `toolUseResult` expected on a tool-*use* line.
  2. A `user` line whose `message.content` carries a `type: "tool_result"` block with nested
     `tool_use_id: "toolu_0000000070"` — this nested field is the **realistic** signal a real
     transcript uses to mark a tool-result-shaped line — but **the top-level `toolUseResult`
     key itself is absent** (confirmed: `grep -c toolUseResult no-tur.jsonl` → `0`).
  3. A second such line, same shape, also referencing `tool_use_id: "toolu_0000000070"`
     (no dangling references — every `tool_use_id`/`toolUseID` in this file points at the one
     `tool_use` id that actually appears on line 1).

Note: both lines 2 and 3 also carry a **top-level `toolUseID` field** duplicating the nested
`tool_use_id`. This top-level field is authoring convenience for this fixture, not a
confirmed real-corpus shape — do not key AC8's "structurally a tool result" detection on it
alone; the nested `message.content[].type == "tool_result"` block is the signal to rely on.

---

## Expected session counts per Pinned decision 3's importability predicate

Pinned decision 3 defines the AC1 numerator/denominator: denominator = every `.jsonl` file
directly under `<source>/projects/<project>/` (one level, never under `subagents/`);
numerator = a denominator session counts as importable iff (a) not entirely sidechain, and
(b) it completes without hitting the AC8 loud-failure path — a session with some malformed
lines still counts importable if at least one line parses and the session completes; a
session with zero file-entries but at least one opaque call still counts importable. Applying
that predicate to each scenario here (each scenario is its own `--source` root):

| Scenario | Denominator (top-level `.jsonl` files) | Importable | Why |
|---|---|---|---|
| `t1_and_t15/` | 1 | 1 | All lines parse; both entries resolve (T1, T1.5); no AC8 field missing. |
| `t2_t3_candidate/` | 1 | 1 | All lines parse; no AC8 field missing (T2 detection doesn't gate importability). |
| `opaque/` | 1 | 1 | Zero file-entries, but the one line is a legitimate opaque `Bash` call — importable per decision 3(b)'s explicit carve-out. |
| `sidechain/` | **1** | 1 (but 0 turns) | `subagents/agent-x1.jsonl` is **two levels** below `projects/proj-a/` (under `<sessionId>/subagents/`), so it is **outside the denominator entirely** — it is not "a session that fails", it's not counted at all. Only the top-level `d4444444-...jsonl` counts, and it counts as importable (it completes cleanly) even though it produces **zero turns** (its one file-producing line is sidechain-excluded). Importability and turn-count are different axes — this fixture is the one that makes that distinction visible. |
| `malformed/` | 1 | **0** | Every line fails to parse (AC6), so **zero** lines parse — decision 3(b)'s "at least one line parses" carve-out does not apply here. This is the one a test author is most likely to get wrong, since AC6 only promises exit 0 + no abort and says nothing about importability. |
| `missing_cwd/` | 1 | **0** | Hits the AC8 loud-failure path (missing required field `cwd`) — decision 3(b) explicitly excludes this from importable. |
| `missing_touluseresult/` | 1 | **0** | Hits the AC8 loud-failure path (missing required field `toolUseResult` on a line that structurally needs it). |

---

## Directory layout

```
cli/tests/fixtures/import/claude/
├── FIXTURES.md                                  (this file)
├── t1_and_t15/
│   ├── projects/proj-a/a1111111-1111-4111-8111-111111111111.jsonl
│   └── file-history/a1111111-1111-4111-8111-111111111111/aa11bb22cc33dd44@v1
├── t2_t3_candidate/
│   └── projects/proj-a/b2222222-2222-4222-8222-222222222222.jsonl
├── opaque/
│   └── projects/proj-a/c3333333-3333-4333-8333-333333333333.jsonl
├── sidechain/
│   └── projects/proj-a/
│       ├── d4444444-4444-4444-8444-444444444444.jsonl
│       └── d4444444-4444-4444-8444-444444444444/subagents/agent-x1.jsonl
├── malformed/
│   └── projects/proj-a/broken.jsonl
├── missing_cwd/
│   └── projects/proj-a/no-cwd.jsonl
└── missing_touluseresult/
    └── projects/proj-a/no-tur.jsonl
```
