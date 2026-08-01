#!/usr/bin/env python3
"""D6 spike gate: measure transcript-declared-write coverage of observed fs
mutations, on the real dogfood corpus. Read-only. Phase 1 of
docs/superpowers/plans/2026-08-01-d6-transcript-correlation.md.

Usage:
    python3 scripts/d6_spike.py --root ~/Projects/agentrec

Joins rich `tool:"claude"` turns in <root>/.agentrec/log.jsonl to
~/.claude/projects/*/<session>.jsonl transcripts.

Two metrics, deliberately both:
  - SESSION-level: union(declared paths in transcript) vs union(observed
    files across that session's rich turns). Ceiling for file-level
    attribution — the production design correlates at bracket close over the
    whole prompt->stop span, which the log does not record, so session union
    is the closest measurable upper bound.
  - TURN-level: declared writes with timestamp in [started - lead, ended +
    late] vs that turn's files. Lead slack is required because turn.started
    is the FIRST OBSERVED MUTATION (post-debounce), which trails the
    tool_use timestamp (measured ~8s on real pairs); this is a spike
    approximation, not the production window.

Observed files are bucketed: hook-artifact churn (.remember/, .claims/,
.superpowers/, _rsrc_probe/, .code-review-graph/) vs working files. Hook
writes are same-session but not agent tool_use — under D6 correlation they
would stay unattributed, which is the desired outcome, so coverage is
reported per bucket rather than blended.

Declared-write shapes harvested (union):
  - assistant lines: message.content[].type=="tool_use", name in WRITE_TOOLS,
    input.file_path / input.notebook_path
  - any line: toolUseResult.filePath (presence-based, not type-literal —
    the file-history-snapshot lesson)

Deliberately NOT harvested: Bash tool invocations (bash-side writes are
undeclared by design — that gap is part of what this spike measures).

Caveats printed with results:
  - True false-attribution (declared AND observed but human-authored) is NOT
    measurable without ground truth; coverage and phantom rate bound it.
  - `ended` is the last observed mutation, not Stop-signal arrival.
"""

import argparse
import collections
import glob
import json
import os
import sys
from datetime import datetime, timedelta

WRITE_TOOLS = {"Write", "Edit", "MultiEdit", "NotebookEdit"}
HOOK_ARTIFACT_PREFIXES = (
    ".remember/",
    ".claims/",
    ".superpowers/",
    "_rsrc_probe/",
    ".code-review-graph/",
)


def parse_ts(s):
    try:
        return datetime.fromisoformat(s.replace("Z", "+00:00"))
    except (ValueError, AttributeError):
        return None


def is_hook_artifact(rel):
    return any(rel.startswith(p) for p in HOOK_ARTIFACT_PREFIXES)


def load_turns(log_path):
    turns = []
    bad = 0
    with open(log_path, encoding="utf-8") as f:
        for line in f:
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                bad += 1
                continue
            if r.get("grade") != "rich" or r.get("tool") != "claude":
                continue
            if r.get("imported"):
                continue  # imported turns came FROM transcripts; circular
            if not r.get("session"):
                continue
            s, e = parse_ts(r.get("started", "")), parse_ts(r.get("ended", ""))
            if not s or not e:
                continue
            turns.append(r | {"_started": s, "_ended": e})
    return turns, bad


def find_transcript(session):
    hits = glob.glob(os.path.expanduser(f"~/.claude/projects/*/{session}.jsonl"))
    return hits[0] if hits else None


def declared_writes(transcript_path):
    """[(ts, abs_path, shape)] for every declared write in the transcript."""
    out = []
    with open(transcript_path, encoding="utf-8") as f:
        for line in f:
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue
            ts = parse_ts(r.get("timestamp", ""))
            msg = r.get("message") or {}
            content = msg.get("content")
            if isinstance(content, list):
                for c in content:
                    if (
                        isinstance(c, dict)
                        and c.get("type") == "tool_use"
                        and c.get("name") in WRITE_TOOLS
                    ):
                        inp = c.get("input") or {}
                        p = inp.get("file_path") or inp.get("notebook_path")
                        if p:
                            out.append((ts, p, "tool_use"))
            tur = r.get("toolUseResult")
            if isinstance(tur, dict) and tur.get("filePath"):
                out.append((ts, tur["filePath"], "toolUseResult"))
    return out


def normalize(abs_path, root):
    """abs -> root-relative, or None + reason if out of root."""
    ap = os.path.normpath(abs_path)
    rt = os.path.normpath(root)
    if ap == rt:
        return None, "is_root"
    if not ap.startswith(rt + os.sep):
        return None, "out_of_root"
    return ap[len(rt) + 1 :], None


def cov_line(label, num, den):
    p = f"{100.0 * num / den:.1f}%" if den else "n/a"
    return f"{label}: {num}/{den} = {p}"


def global_declared_index(root):
    """Session-agnostic: every declared write into root from EVERY transcript
    in every ~/.claude/projects dir. Needed because F4 (session-unaware
    bracket matching) makes turn.session unreliable in this corpus — stub
    background sessions (hooks-only, zero tool_use) close other sessions'
    brackets and get recorded as the turn's session."""
    idx = collections.defaultdict(list)  # rel -> [ts]
    n_declaring = 0
    for tp in glob.glob(os.path.expanduser("~/.claude/projects/*/*.jsonl")):
        found = False
        for ts, p, _shape in declared_writes(tp):
            rel, why = normalize(p, root)
            if why is None:
                idx[rel].append(ts)
                found = True
        if found:
            n_declaring += 1
    for v in idx.values():
        v.sort(key=lambda t: t or datetime.min)
    return idx, n_declaring


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--lead-slack", type=int, default=120,
                    help="seconds before turn.started a declare may precede "
                         "observation (debounce + fs latency)")
    ap.add_argument("--late-slack", type=int, default=60)
    ap.add_argument("--any-lead", type=int, default=300,
                    help="lead slack for the session-agnostic pass (wider: "
                         "no session pairing to anchor on)")
    args = ap.parse_args()

    root = os.path.realpath(os.path.expanduser(args.root))
    log_path = os.path.join(root, ".agentrec", "log.jsonl")
    turns, bad_lines = load_turns(log_path)

    by_session = collections.defaultdict(list)
    for t in turns:
        by_session[t["session"]].append(t)

    declared_cache = {}
    missing_transcript = 0
    for sess in by_session:
        p = find_transcript(sess)
        declared_cache[sess] = declared_writes(p) if p else None
        if p is None:
            missing_transcript += 1

    # ---- session-level ceiling ----
    s_num = {"hook": 0, "work": 0}
    s_den = {"hook": 0, "work": 0}
    sessions_with_writes = 0
    sessions_with_observed = 0
    out_of_root_declares = 0
    total_declares = 0
    per_session_work = []  # (hit, den, sess) for work bucket

    # ---- turn-level ----
    lead = timedelta(seconds=args.lead_slack)
    late = timedelta(seconds=args.late_slack)
    t_num = {"hook": 0, "work": 0}
    t_den = {"hook": 0, "work": 0}
    turns_with_work = 0
    turns_work_fully_covered = 0
    shape_counts = collections.Counter()
    phantom = collections.Counter()

    for sess, ts_list in by_session.items():
        writes = declared_cache[sess]
        if writes is None:
            continue
        norm = []
        for w_ts, w_p, shape in writes:
            total_declares += 1
            rel, why = normalize(w_p, root)
            if why:
                out_of_root_declares += 1
                continue
            norm.append((w_ts, rel, shape))
        declared_set = {rel for _, rel, _ in norm}
        if declared_set:
            sessions_with_writes += 1

        observed_all = set()
        for t in ts_list:
            obs = {f["path"] for f in t.get("files", []) if not f.get("withheld")}
            observed_all |= obs

            # turn-level
            in_win = {rel for w_ts, rel, shape in norm
                      if w_ts and t["_started"] - lead <= w_ts <= t["_ended"] + late}
            for w_ts, rel, shape in norm:
                if w_ts and t["_started"] - lead <= w_ts <= t["_ended"] + late:
                    shape_counts[shape] += 1
            work_obs = {p for p in obs if not is_hook_artifact(p)}
            hook_obs = obs - work_obs
            t_num["work"] += len(in_win & work_obs)
            t_den["work"] += len(work_obs)
            t_num["hook"] += len(in_win & hook_obs)
            t_den["hook"] += len(hook_obs)
            if work_obs:
                turns_with_work += 1
                if work_obs <= in_win:
                    turns_work_fully_covered += 1

        if observed_all:
            sessions_with_observed += 1
        work_all = {p for p in observed_all if not is_hook_artifact(p)}
        hook_all = observed_all - work_all
        s_num["work"] += len(declared_set & work_all)
        s_den["work"] += len(work_all)
        s_num["hook"] += len(declared_set & hook_all)
        s_den["hook"] += len(hook_all)
        if work_all:
            per_session_work.append(
                (len(declared_set & work_all), len(work_all), sess)
            )

        for p in declared_set - observed_all:
            if p.startswith(".claude/"):
                phantom["dot_claude_watchfiltered"] += 1
            elif not os.path.exists(os.path.join(root, p)):
                phantom["gone_now"] += 1
            elif is_hook_artifact(p):
                phantom["hook_artifact"] += 1
            else:
                phantom["exists_never_observed"] += 1

    print(f"log: {log_path} (unparseable lines: {bad_lines})")
    print(f"population: rich claude turns with session: {len(turns)} "
          f"across {len(by_session)} sessions")
    print(f"join: transcripts missing {missing_transcript}/{len(by_session)} "
          f"sessions (30-day retention bound)")
    print(f"declared writes total {total_declares}, out-of-root "
          f"{out_of_root_declares} (worktrees/other repos)")
    print(f"sessions with >=1 in-root declared write: {sessions_with_writes}; "
          f"with >=1 observed file: {sessions_with_observed}")
    print()
    print("== SESSION-level ceiling (union declared vs union observed) ==")
    print(cov_line("  work-file coverage", s_num["work"], s_den["work"]))
    print(cov_line("  hook-artifact coverage (should be ~0)",
                   s_num["hook"], s_den["hook"]))
    full = sum(1 for h, d, _ in per_session_work if h == d)
    zero = sum(1 for h, d, _ in per_session_work if h == 0)
    print(f"  sessions w/ work files: {len(per_session_work)}; fully covered "
          f"{full}; zero-covered {zero}")
    print()
    print(f"== TURN-level (lead {args.lead_slack}s / late {args.late_slack}s) ==")
    print(cov_line("  work-file coverage", t_num["work"], t_den["work"]))
    print(cov_line("  hook-artifact coverage (should be ~0)",
                   t_num["hook"], t_den["hook"]))
    print(cov_line("  turns w/ work files fully covered",
                   turns_work_fully_covered, turns_with_work))
    print(f"  declared shapes matched in-window: {dict(shape_counts)}")
    print()
    print(f"PHANTOM (declared in-root, never observed anywhere in session): "
          f"{dict(phantom)}")

    # ---- stub-session classification (F4 evidence) ----
    stub = with_tools = 0
    for sess in by_session:
        writes = declared_cache[sess]
        if writes is None:
            continue
        p = find_transcript(sess)
        any_tool = False
        with open(p, encoding="utf-8") as f:
            for line in f:
                if '"tool_use"' in line:
                    any_tool = True
                    break
        if any_tool:
            with_tools += 1
        else:
            stub += 1
    print(f"\n== F4 evidence: joined sessions with ZERO tool_use lines "
          f"(hook-only stubs recorded as the turn's session): "
          f"{stub}/{stub + with_tools}")

    # ---- session-agnostic (any-transcript) turn-level coverage ----
    idx, n_declaring = global_declared_index(root)
    any_lead = timedelta(seconds=args.any_lead)
    a_num = a_den = 0
    a_turns = a_full = a_zero = 0
    uncovered_dirs = collections.Counter()
    for sess, ts_list in by_session.items():
        for t in ts_list:
            work = {
                f["path"] for f in t.get("files", [])
                if not f.get("withheld") and not is_hook_artifact(f["path"])
            }
            if not work:
                continue
            a_turns += 1
            hit = {
                p for p in work
                if any(
                    ts and t["_started"] - any_lead <= ts <= t["_ended"] + late
                    for ts in idx.get(p, [])
                )
            }
            a_num += len(hit)
            a_den += len(work)
            if hit == work:
                a_full += 1
            if not hit:
                a_zero += 1
            for p in work - hit:
                uncovered_dirs[p.split("/")[0]] += 1
    print(f"\n== ANY-SESSION turn-level (lead {args.any_lead}s / late "
          f"{args.late_slack}s; {n_declaring} transcripts declare into root) ==")
    print(cov_line("  work-file coverage ceiling", a_num, a_den))
    print(f"  turns w/ work files: {a_turns}; fully covered {a_full}; "
          f"zero {a_zero}")
    print(f"  uncovered top dirs: {uncovered_dirs.most_common(10)}")
    print("\nCAVEATS: session-level is a CEILING (whole-session union, not "
          "bracket windows); turn-level lead slack is a spike approximation "
          "of the prompt->stop bracket the log does not record. Coverage and "
          "phantom bound false-attribution; no ground truth for "
          "human-authored-but-declared exists in this corpus.")


if __name__ == "__main__":
    sys.exit(main())
