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

*Amended after review to add a sibling valid file, proving the broken file doesn't abort
processing of others.*

- File: `projects/proj-a/broken.jsonl`, 5 lines, **every single line is syntactically invalid
  JSON** (verified with `json.loads` in Python — all 5 raise `JSONDecodeError`):
  1. Unbalanced brace (opening `{` never closed).
  2. Truncated array (`"content": [}` — mismatched bracket/brace).
  3. Stray non-JSON text with no braces at all.
  4. Unbalanced brace (opening `{` never closed) on an otherwise plausible-looking tool-result
     line.
  5. Bare `{{{{` garbage.
- No valid JSON appears anywhere in this file.
- File: `projects/proj-a/valid-sibling.jsonl`, 1 line, hand-authored, unremarkable T1 `Edit`
  entry (`originalFile` set, `cwd: "/fake/repo9"`) — same shape as the other scenarios' valid
  lines. Its only purpose is proving `broken.jsonl`'s malformed content doesn't abort the scan
  of other files in the same project directory: this session must still count importable.

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

## 8. `t15_relative_key/`

**Proves:** the T1.5 path-normalization fix — a `trackedFileBackups` key that is **relative**
to the session's `cwd` must resolve, not just an already-absolute key. Relative keys vastly
outnumber absolute ones on the real corpus, though the exact ratio drifts (it's a live, rolling
≤30-day window): re-verified 2026-07-29 at 1196 of 1401 relative; an earlier same-day
measurement on a larger pre-decay snapshot saw 1606 of 1883. Also the RED→GREEN regression test
for the reported defect: fails against the pre-fix raw-string-compare lookup, passes once keys
are normalized against `cwd` at harvest/lookup time.

- Session id: `a1515151-1515-4151-8151-151515151515`
- File: `projects/proj-a/a1515151-1515-4151-8151-151515151515.jsonl`, 2 lines, `cwd:
  "/fake/repo15"`:
  1. A `file-history-snapshot` line recording `trackedFileBackups["src/notes.txt"]` — **a
     relative key**, unlike `t1_and_t15/`'s absolute one — `.backupFileName =
     "dd44ee55ff66aa77@v1"`.
  2. An `Edit` result for `/fake/repo15/src/notes.txt` (absolute, as `toolUseResult.filePath`
     always is) — **no `originalFile`** — must resolve via the relative key joined against
     `cwd`. `oldString: "alpha value"` is present and matches the blob, so this must be
     content-proven (`t15_unverified` stays 0).
- File-history blob: `file-history/a1515151-1515-4151-8151-151515151515/dd44ee55ff66aa77@v1`.

### Known-good content + independently-computed hash

```
alpha value
beta value
```

```
$ shasum -a 256 cli/tests/fixtures/import/claude/t15_relative_key/file-history/a1515151-1515-4151-8151-151515151515/dd44ee55ff66aa77@v1
678cd3ef69c16e338a717c91a0ecdec9c394b4992360d2680539fbaf028b8396  cli/tests/fixtures/import/claude/t15_relative_key/file-history/a1515151-1515-4151-8151-151515151515/dd44ee55ff66aa77@v1
```

**Expected `sha256(resolved before bytes)`:** `678cd3ef69c16e338a717c91a0ecdec9c394b4992360d2680539fbaf028b8396`

---

## 9. `t15_stale_blob/`

**Proves:** the T1.5 "never fabricate" verification guard — a backup blob resolves and is
readable, but its content does **not** contain the edit's `oldString`, so the blob is not
actually this edit's pre-state. The entry must be refused as T1.5 (never fabricate a
plausible-but-wrong `before`) and fall through to T2/T3 — here T3, since `/fake/repo16` is not
backed by any git repo in this fixture. The rejection must be counted in
`t15_rejected_unverifiable`, distinct from silently folding into T3.

- Session id: `a1616161-1616-4161-8161-161616161616`
- File: `projects/proj-a/a1616161-1616-4161-8161-161616161616.jsonl`, 2 lines, `cwd:
  "/fake/repo16"`:
  1. A `file-history-snapshot` line recording `trackedFileBackups["/fake/repo16/stale.txt"]`
     (absolute key, to isolate this test to the verification guard rather than path
     normalization) `.backupFileName = "cc11dd22ee33ff44@v1"`.
  2. An `Edit` result for `/fake/repo16/stale.txt`, `oldString: "original text"` — **the blob
     content below does not contain this string**.
- File-history blob: `file-history/a1616161-1616-4161-8161-161616161616/cc11dd22ee33ff44@v1`
  — deliberately unrelated content (`totally unrelated content\n`), simulating a stale/mismatched
  backup recorded for a different edit.

---

## 10. `t15_cwd_after_snapshot/`

**Proves:** the T1.5 ordering trap in Fix 1 — the `file-history-snapshot` line carrying
`trackedFileBackups` appears **before** any line in the session carries `cwd`. A relative key
harvested at that point cannot be resolved yet; it must be held pending and resolved once a
later line establishes `cwd`, not silently dropped.

**This is the dominant real-corpus shape, not a corner case:** a corpus sweep on 2026-07-29
found `file-history-snapshot` lines **never** carry their own `cwd` field (0 of 468 such lines
on this machine's `~/.claude` corpus) — `cwd` always comes from a later (or, in `t1_and_t15/`'s
simpler fixture, coincidentally-same-line) part of the session. This fixture is what makes that
the tested path rather than an untested assumption; the `t1_and_t15/` and `t15_relative_key/`
fixtures putting `cwd` on the snapshot line itself are the synthetic simplification, not the
real shape.

- Session id: `a1717171-1717-4171-8171-171717171717`
- File: `projects/proj-a/a1717171-1717-4171-8171-171717171717.jsonl`, 3 lines:
  1. A `file-history-snapshot` line — **no `cwd` field at all** — recording
     `trackedFileBackups["docs/readme.txt"]` (relative key) `.backupFileName =
     "ee55ff66aa77bb88@v1"`.
  2. A plain `user` chat line, `cwd: "/fake/repo17"` — the first line in the session to
     establish `cwd`, with no `toolUseResult` of its own.
  3. An `Edit` result for `/fake/repo17/docs/readme.txt`, `oldString: "readme original"` —
     matches the blob below, so this must resolve T1.5 once `cwd` from line 2 is applied to
     line 1's pending relative key.
- File-history blob: `file-history/a1717171-1717-4171-8171-171717171717/ee55ff66aa77bb88@v1`.

### Known-good content + independently-computed hash

```
readme original
more text
```

```
$ shasum -a 256 cli/tests/fixtures/import/claude/t15_cwd_after_snapshot/file-history/a1717171-1717-4171-8171-171717171717/ee55ff66aa77bb88@v1
59c14830f5913e2fb18b82eb129dfb6189bf39b80a7a858c0b9b86758e1ae963  cli/tests/fixtures/import/claude/t15_cwd_after_snapshot/file-history/a1717171-1717-4171-8171-171717171717/ee55ff66aa77bb88@v1
```

**Expected `sha256(resolved before bytes)`:** `59c14830f5913e2fb18b82eb129dfb6189bf39b80a7a858c0b9b86758e1ae963`

---

## 11. `t15_unverified_no_oldstring/`

**Proves:** the T1.5 "unverified" branch of Fix 2 — an entry whose `toolUseResult` has **no
`oldString` field at all** (some ops don't carry one). The containment check can't run, so
classification instead relies on the structural argument from Fix 1 (T1.5 fabrication round): no
other edit to this path has been classified since the backup was recorded (`edits_since_backup ==
0`), which is sufficient on its own to trust the blob as this edit's pre-state — but the entry
must still be counted `t15_unverified`, distinct from a content-proven match, since no byte-level
proof ran.

**Amended (T1.5 fabrication round, Fix 4):** the blob content used to be described as "no relation
to any oldString" — i.e. the fixture asserted a sha256 for bytes its own comment admitted were
unrelated to anything, which is exactly the shape of bug this round exists to eliminate. The blob
below is now a plausible pre-edit cell body consistent with the `NotebookEdit`'s `newString`
(`"print('hi')"`), i.e. what this fixture proves is "structurally-inferred, not content-proven"
classification of a *genuine* clean snapshot→edit sequence, not classification of admittedly
unrelated bytes.

**Corpus-untested branch, called out honestly:** a full real-corpus gate run (2026-07-29,
1626 sessions) found `oldString` present and non-empty on every single one of the 243 T1.5
candidates — `t15_unverified` measured `0` there. This fixture is the *only* coverage this
branch has; it exists purely as a defensive path for a schema shape the corpus didn't happen to
exercise this time.

- Session id: `a1818181-1818-4181-8181-181818181818`
- File: `projects/proj-a/a1818181-1818-4181-8181-181818181818.jsonl`, 2 lines, `cwd:
  "/fake/repo18"`:
  1. A `file-history-snapshot` line recording `trackedFileBackups["/fake/repo18/data.bin"]`
     (absolute key) `.backupFileName = "ff11aa22bb33cc44@v1"`.
  2. A `NotebookEdit`-shaped result for `/fake/repo18/data.bin` — no `originalFile`, and
     **no `oldString` key at all** (only `newString: "print('hi')"`) — the only edit to this
     path in the session, so `edits_since_backup` is 0 at classification time.
- File-history blob: `file-history/a1818181-1818-4181-8181-181818181818/ff11aa22bb33cc44@v1`.

### Known-good content + independently-computed hash

```
print('hello')
```

```
$ shasum -a 256 cli/tests/fixtures/import/claude/t15_unverified_no_oldstring/file-history/a1818181-1818-4181-8181-181818181818/ff11aa22bb33cc44@v1
03e693d9f2f687e0f40e36a8df7fcb4d1c22974012b7c2a55c000eb30f305824  cli/tests/fixtures/import/claude/t15_unverified_no_oldstring/file-history/a1818181-1818-4181-8181-181818181818/ff11aa22bb33cc44@v1
```

**Expected `sha256(resolved before bytes)`:** `03e693d9f2f687e0f40e36a8df7fcb4d1c22974012b7c2a55c000eb30f305824`

---

## 12. `t15_intervening_edit/`

**Proves:** Fix 1 (T1.5 fabrication round) — the shape every prior T1.5 fixture lacked: a
snapshot followed by **two** edits to the same path with no intervening snapshot. The blob is
edit #1's true pre-state (must classify T1.5, content-proven) but is stale for edit #2. Edit #2's
`oldString` (`"shared line\n"`) is deliberately chosen to still be present, byte-for-byte, in the
stale blob's untouched second line — so the pre-fix `oldString`-only containment guard would have
**incorrectly accepted** it as T1.5. The intervening-edit check (tracked per-path,
`edits_since_backup`) must reject edit #2 regardless, counted in `t15_rejected_stale`, distinct
from `t15_rejected_unverifiable` (which is a content-proven rejection — this one is rejected
before any content check runs).

- Session id: `a1919191-1919-4191-8191-191919191919`
- File: `projects/proj-a/a1919191-1919-4191-8191-191919191919.jsonl`, 3 lines, `cwd:
  "/fake/repo19"`:
  1. A `file-history-snapshot` line recording `trackedFileBackups["/fake/repo19/multi.txt"]`
     (absolute key) `.backupFileName = "9988776655443322@v1"`.
  2. Edit #1: `oldString: "alpha original\n"`, `newString: "alpha changed\n"` — matches the
     blob's first line; must resolve T1.5, content-proven.
  3. Edit #2 (same path, no intervening snapshot): `oldString: "shared line\n"`, `newString:
     "shared line updated\n"` — this string IS present in the blob (its second line is
     unchanged since the blob predates edit #1 too), so a content-only guard would wrongly
     accept it. Must be rejected as stale instead, falling through to T3 (no git repo backs
     `/fake/repo19` in this fixture).
- File-history blob: `file-history/a1919191-1919-4191-8191-191919191919/9988776655443322@v1`.

### Known-good content + independently-computed hash

```
alpha original
shared line
```

```
$ shasum -a 256 cli/tests/fixtures/import/claude/t15_intervening_edit/file-history/a1919191-1919-4191-8191-191919191919/9988776655443322@v1
99931c6269c8f9e306b7ce0ac5fe986ab1121e83df93153eb3f66cf298384c56  cli/tests/fixtures/import/claude/t15_intervening_edit/file-history/a1919191-1919-4191-8191-191919191919/9988776655443322@v1
```

**Expected `sha256(resolved before bytes)` for edit #1:**
`99931c6269c8f9e306b7ce0ac5fe986ab1121e83df93153eb3f66cf298384c56`

**Edit #2 must not resolve any bytes** (`sha256: null`), classified `t3`, counted in
`t15_rejected_stale`.

---

## 13. `t15_redundant_backup_announcement/`

**Proves:** a refinement discovered during real-corpus ground-truth validation of Fix 1 (not in
the original brief) — a `trackedFileBackups` snapshot line that re-announces the exact SAME
`backupFileName` for a path (confirmed on the real corpus: consecutive snapshot lines with
identical `backupFileName` AND identical `backupTime` — a manifest-style re-list of
already-known backups, not evidence of a fresh backup) must NOT reset the intervening-edit
staleness counter. Only a genuinely different `backupFileName` may reset it
(`record_backup`'s `is_new_backup` check). Validating the Fix-1-only implementation against
ground truth (real `~/.claude` corpus entries carrying both an inline `originalFile` and a
resolvable T1.5 backup) found this exact shape was the dominant remaining source of fabricated
T1.5 classifications: 4 of 4 initial post-Fix-1 mismatches had a redundant same-name
re-announcement sitting between the stale backup and the edit that used it.

- Session id: `a2020202-2020-4202-8202-202020202020`
- File: `projects/proj-a/a2020202-2020-4202-8202-202020202020.jsonl`, 4 lines, `cwd:
  "/fake/repo20"`:
  1. A `file-history-snapshot` line recording `trackedFileBackups["/fake/repo20/redundant.txt"]`
     (absolute key) `.backupFileName = "aabbccdd11223344@v1"`.
  2. Edit #1: `oldString: "line A\n"`, `newString: "line A changed\n"` — matches the blob's
     first line; must resolve T1.5, content-proven.
  3. A SECOND `file-history-snapshot` line re-announcing the identical `backupFileName =
     "aabbccdd11223344@v1"` for the same path (`isSnapshotUpdate: true`) — simulating the
     real-corpus manifest re-list shape. Must NOT reset the staleness this path already
     accrued from edit #1.
  4. Edit #2 (same path): `oldString: "line B\n"`, `newString: "line B changed\n"` — this
     string IS present in the blob (its second line is unchanged), so a content-only guard
     (or a staleness rule that reset on line 3's redundant re-announcement) would wrongly
     accept it. Must be rejected as stale, falling through to T3 (no git repo backs
     `/fake/repo20` in this fixture).
- File-history blob: `file-history/a2020202-2020-4202-8202-202020202020/aabbccdd11223344@v1`.

### Known-good content + independently-computed hash

```
line A
line B
```

```
$ shasum -a 256 cli/tests/fixtures/import/claude/t15_redundant_backup_announcement/file-history/a2020202-2020-4202-8202-202020202020/aabbccdd11223344@v1
64c3a782a5345755c8a957545fc2b274fc373b51196864c63cea25409951c4fb  cli/tests/fixtures/import/claude/t15_redundant_backup_announcement/file-history/a2020202-2020-4202-8202-202020202020/aabbccdd11223344@v1
```

**Expected `sha256(resolved before bytes)` for edit #1:**
`64c3a782a5345755c8a957545fc2b274fc373b51196864c63cea25409951c4fb`

**Edit #2 must not resolve any bytes** (`sha256: null`), classified `t3`, counted in
`t15_rejected_stale`.

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
| `malformed/` | **2** | **1** | `broken.jsonl`: every line fails to parse (AC6), so **zero** lines parse — decision 3(b)'s "at least one line parses" carve-out does not apply, so it is not importable. `valid-sibling.jsonl` (added after review): all lines parse, no AC8 field missing, so it counts importable — proving `broken.jsonl` doesn't abort the scan of its sibling. |
| `missing_cwd/` | 1 | **0** | Hits the AC8 loud-failure path (missing required field `cwd`) — decision 3(b) explicitly excludes this from importable. |
| `missing_touluseresult/` | 1 | **0** | Hits the AC8 loud-failure path (missing required field `toolUseResult` on a line that structurally needs it). |
| `t15_relative_key/` | 1 | 1 | All lines parse; the relative `trackedFileBackups` key resolves to T1.5 against `cwd`; no AC8 field missing. |
| `t15_stale_blob/` | 1 | 1 | All lines parse; the entry falls through to T3 (rejected as unverifiable T1.5), which does not gate importability. |
| `t15_cwd_after_snapshot/` | 1 | 1 | All lines parse; `cwd` is established by line 2 (session-level, per decision 14), so the session is importable and the pending relative key resolves. |
| `t15_unverified_no_oldstring/` | 1 | 1 | All lines parse; no AC8 field missing; the missing-`oldString` entry still classifies T1.5 (unverified), which does not gate importability. |
| `t15_intervening_edit/` | 1 | 1 | All lines parse; no AC8 field missing; edit #1 resolves T1.5, edit #2 falls through to T3 (rejected as stale) — the fallthrough doesn't gate importability. |
| `t15_redundant_backup_announcement/` | 1 | 1 | All lines parse; no AC8 field missing; edit #1 resolves T1.5, edit #2 falls through to T3 (rejected as stale despite the redundant same-name re-announcement) — the fallthrough doesn't gate importability. |

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
│   └── projects/proj-a/
│       ├── broken.jsonl
│       └── valid-sibling.jsonl                    (added after review, see scenario 5)
├── missing_cwd/
│   └── projects/proj-a/no-cwd.jsonl
├── missing_touluseresult/
│   └── projects/proj-a/no-tur.jsonl
├── t15_relative_key/
│   ├── projects/proj-a/a1515151-1515-4151-8151-151515151515.jsonl
│   └── file-history/a1515151-1515-4151-8151-151515151515/dd44ee55ff66aa77@v1
├── t15_stale_blob/
│   ├── projects/proj-a/a1616161-1616-4161-8161-161616161616.jsonl
│   └── file-history/a1616161-1616-4161-8161-161616161616/cc11dd22ee33ff44@v1
├── t15_cwd_after_snapshot/
│   ├── projects/proj-a/a1717171-1717-4171-8171-171717171717.jsonl
│   └── file-history/a1717171-1717-4171-8171-171717171717/ee55ff66aa77bb88@v1
├── t15_unverified_no_oldstring/
│   ├── projects/proj-a/a1818181-1818-4181-8181-181818181818.jsonl
│   └── file-history/a1818181-1818-4181-8181-181818181818/ff11aa22bb33cc44@v1
├── t15_intervening_edit/
│   ├── projects/proj-a/a1919191-1919-4191-8191-191919191919.jsonl
│   └── file-history/a1919191-1919-4191-8191-191919191919/9988776655443322@v1
└── t15_redundant_backup_announcement/
    ├── projects/proj-a/a2020202-2020-4202-8202-202020202020.jsonl
    └── file-history/a2020202-2020-4202-8202-202020202020/aabbccdd11223344@v1
```
