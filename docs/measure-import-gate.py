#!/usr/bin/env python3
"""Phase 2.0 entry-gate measurement, v2 — THROWAWAY.

v1 findings that forced this rewrite:
  * 583/1858 files are `subagents/agent-*.jsonl` — sidechain transcripts the spec
    already excludes. Counting them made the gate read 68%. Wrong denominator.
  * `~/.claude/file-history/<session>/<hash>@<vN>` holds VERBATIM pre-edit file
    bytes. The spec's 3-tier ladder does not mention it. Measured here as T1.5.
  * "cwd is a live git repo" is a bad tier-2 proxy: git holds COMMITTED states
    only, so mid-session intermediate before-bytes are absent even in a live
    repo. Tier 2 is reported as an upper bound and labelled as such.
"""
import json
import os
import pathlib
import subprocess
import sys
from collections import Counter

ROOT = pathlib.Path.home() / ".claude" / "projects"
FILE_HISTORY = pathlib.Path.home() / ".claude" / "file-history"

NAMING = {"Edit", "Write", "MultiEdit", "NotebookEdit"}
OPAQUE = {"Bash", "Task", "Agent", "BashOutput", "KillShell"}

# sessionId -> set of backup basenames present on disk
_fh_index: dict[str, set[str]] | None = None
_git_tracked: dict[tuple[str, str], bool] = {}


def fh_index() -> dict[str, set[str]]:
    global _fh_index
    if _fh_index is None:
        _fh_index = {}
        if FILE_HISTORY.is_dir():
            for d in FILE_HISTORY.iterdir():
                if d.is_dir():
                    _fh_index[d.name] = {f.name for f in d.iterdir() if f.is_file()}
    return _fh_index


def git_tracks(repo: str, path: str) -> bool:
    """UPPER BOUND on tier 2: file has at least one committed blob in this repo.

    Does not prove the pre-edit state at the turn's timestamp is recoverable --
    uncommitted intermediate edits never entered git at all.
    """
    key = (repo, path)
    if key in _git_tracked:
        return _git_tracked[key]
    ok = False
    if repo and os.path.isdir(repo) and path:
        r = subprocess.run(
            ["git", "-C", repo, "log", "--oneline", "-1", "--", path],
            capture_output=True, text=True,
        )
        ok = r.returncode == 0 and bool(r.stdout.strip())
    _git_tracked[key] = ok
    return ok


def analyse(path: pathlib.Path, is_subagent: bool) -> dict:
    st = {
        "lines": 0, "unparseable": 0, "cwd": None, "session": None,
        "prompts": 0, "naming_calls": 0, "opaque_calls": 0,
        "edit_results": 0, "t1": 0, "t15": 0, "t2": 0, "t3": 0,
        "sidechain_lines": 0, "fh_backup_names": set(), "edit_paths": [],
    }
    pending: dict[str, str] = {}
    with path.open("r", errors="replace") as fh:
        for line in fh:
            st["lines"] += 1
            try:
                d = json.loads(line)
            except Exception:
                st["unparseable"] += 1
                continue
            if not isinstance(d, dict):
                st["unparseable"] += 1
                continue

            if d.get("type") == "file-history-snapshot":
                snap = d.get("snapshot") or {}
                for _p, meta in (snap.get("trackedFileBackups") or {}).items():
                    if isinstance(meta, dict) and meta.get("backupFileName"):
                        st["fh_backup_names"].add(meta["backupFileName"])
                continue

            if d.get("isSidechain") and not is_subagent:
                st["sidechain_lines"] += 1
                continue
            if st["cwd"] is None and d.get("cwd"):
                st["cwd"] = d["cwd"]
            if st["session"] is None and d.get("sessionId"):
                st["session"] = d["sessionId"]

            msg = d.get("message") or {}
            content = msg.get("content")

            if d.get("type") == "user":
                if isinstance(content, str) and content.strip():
                    st["prompts"] += 1
                elif isinstance(content, list) and not any(
                    isinstance(b, dict) and b.get("type") == "tool_result"
                    for b in content
                ):
                    st["prompts"] += 1

            if isinstance(content, list):
                for b in content:
                    if not isinstance(b, dict) or b.get("type") != "tool_use":
                        continue
                    name = b.get("name")
                    if name in NAMING:
                        st["naming_calls"] += 1
                        pending[b.get("id")] = name
                    elif name in OPAQUE:
                        st["opaque_calls"] += 1

            tur = d.get("toolUseResult")
            if isinstance(tur, dict):
                tool = None
                if isinstance(content, list):
                    for b in content:
                        if isinstance(b, dict) and b.get("type") == "tool_result":
                            tool = pending.pop(b.get("tool_use_id"), None)
                if tool is None and ("originalFile" in tur or "filePath" in tur):
                    tool = "Edit?"
                if tool is None:
                    continue
                st["edit_results"] += 1
                fp = tur.get("filePath") or tur.get("file_path") or ""
                if tur.get("originalFile") or tur.get("type") == "create":
                    st["t1"] += 1
                else:
                    st["edit_paths"].append(fp)  # resolved after the file is read
    return st


def main() -> int:
    all_files = sorted(ROOT.rglob("*.jsonl"))
    top = [f for f in all_files if "/subagents/" not in str(f)]
    sub = [f for f in all_files if "/subagents/" in str(f)]
    print(f"corpus: {len(all_files)} files = {len(top)} top-level "
          f"+ {len(sub)} subagent (spec: sidechains skipped, not imported)")

    agg = Counter()
    verdicts = Counter()
    failures: list[tuple[str, str]] = []
    fh_hits = 0
    fh_total_refs = 0
    idx = fh_index()

    for i, f in enumerate(top, 1):
        if i % 250 == 0:
            print(f"  ... {i}/{len(top)}", file=sys.stderr, flush=True)
        try:
            st = analyse(f, is_subagent=False)
        except Exception as e:
            verdicts["crash"] += 1
            failures.append((str(f), f"analyser crash: {e}"))
            continue

        for k in ("lines", "unparseable", "prompts", "naming_calls",
                  "opaque_calls", "edit_results", "t1", "sidechain_lines"):
            agg[k] += st[k]

        # T1.5: does this session have real backup bytes on disk?
        sess = st["session"] or f.stem
        present = idx.get(sess, set())
        for name in st["fh_backup_names"]:
            fh_total_refs += 1
            if name in present:
                fh_hits += 1

        # resolve remaining entries down the ladder
        for fp in st["edit_paths"]:
            if present and fp:
                agg["t15"] += 1          # session has recoverable backup bytes
            elif st["cwd"] and git_tracks(st["cwd"], fp):
                agg["t2"] += 1           # UPPER BOUND (committed states only)
            else:
                agg["t3"] += 1           # provenance-only, honestly non-revertible

        if st["lines"] == 0:
            verdicts["empty"] += 1
            failures.append((str(f), "zero lines"))
        elif st["unparseable"] == st["lines"]:
            verdicts["fail_unparseable"] += 1
            failures.append((str(f), "every line unparseable"))
        elif not st["cwd"]:
            verdicts["fail_no_cwd"] += 1
            failures.append((str(f), "no cwd — cannot map session to repo"))
        elif st["prompts"] == 0 and st["naming_calls"] == 0:
            verdicts["fail_no_turns"] += 1
            failures.append((str(f), "no prompts, no file-naming calls"))
        else:
            verdicts["import_ok"] += 1

    n = len(top)
    ok = verdicts["import_ok"]
    print("\n=== G1  FILE-LEVEL IMPORT GATE (spec: >=90% of real transcripts) ===")
    print(f"  importable  {ok}/{n} = {100*ok/n:.1f}%   [top-level sessions]")
    for k, v in verdicts.most_common():
        if k != "import_ok":
            print(f"  {k:18s} {v}")

    er = agg["edit_results"] or 1
    print("\n=== G2  BEFORE-BYTES LADDER (per file-entry) ===")
    print(f"  edit/write results                 {agg['edit_results']}")
    print(f"  T1  transcript originalFile|create {agg['t1']:5d} = {100*agg['t1']/er:5.1f}%")
    print(f"  T1.5 file-history backup on disk   {agg['t15']:5d} = {100*agg['t15']/er:5.1f}%  <- NOT IN SPEC")
    print(f"  T2  git-tracked (UPPER BOUND)      {agg['t2']:5d} = {100*agg['t2']/er:5.1f}%")
    print(f"  T3  provenance-only (honest null)  {agg['t3']:5d} = {100*agg['t3']/er:5.1f}%")
    rec = agg["t1"] + agg["t15"]
    print(f"  --> reconstructible without git    {rec:5d} = {100*rec/er:5.1f}%")
    print(f"  --> + git upper bound              {rec+agg['t2']:5d} = "
          f"{100*(rec+agg['t2'])/er:5.1f}%")
    print(f"  file-history refs resolved on disk {fh_hits}/{fh_total_refs}"
          f" ({100*fh_hits/(fh_total_refs or 1):.1f}%) — retention-limited")

    print("\n=== G3  STRUCTURAL INCOMPLETENESS ===")
    nc, oc = agg["naming_calls"], agg["opaque_calls"]
    print(f"  naming calls {nc}   opaque calls {oc}   ratio {oc/(nc or 1):.2f}:1")
    print(f"  => files_complete:false is mandatory; {100*oc/((nc+oc) or 1):.0f}%"
          f" of tool calls can mutate files while naming none")

    print("\n=== G4  PARSE ROBUSTNESS ===")
    print(f"  lines {agg['lines']}  unparseable {agg['unparseable']} "
          f"({100*agg['unparseable']/(agg['lines'] or 1):.3f}%)")

    if failures:
        print(f"\n=== FAILING TOP-LEVEL FILES ({len(failures)}) ===")
        for p, why in failures[:12]:
            print(f"  {why:42s} {p}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
