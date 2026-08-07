#!/usr/bin/env python3
"""T0 spike: rework-rate viability on a real log.jsonl (spec 3.0.1, plan T0).

Log-only fold. excluded_unknown_mtime requires working-tree hashing and is
NOT computed by this spike (printed as such, never 0). Clause (c) cannot fire
pre-3.1 (no `reverts` field exists); undo_unevaluable_c is reported.

Usage: python3 scripts/p30_rework_spike.py <path-to-log.jsonl> [window_days]
"""
import json
import sys
from datetime import datetime, timedelta, timezone

WINDOW_DAYS = int(sys.argv[2]) if len(sys.argv) > 2 else 7
NOW = datetime.now(timezone.utc)


def ts(s):
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


def main(path):
    turns, epochs = [], []
    with open(path, "rb") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                r = json.loads(line)
            except json.JSONDecodeError:
                continue  # torn tail tolerated, like real readers
            if r.get("type") == "turn":
                turns.append(r)
            elif r.get("type") == "epoch":
                epochs.append(r)

    # Covered intervals from epoch records: start..stop pairs; a start with no
    # stop before the next start is covered up to that epoch's last activity
    # (crash); trailing start with no stop = covered to NOW (live daemon).
    covered = []
    cur_start = None
    for e in epochs:
        t = ts(e["ts"])
        if e.get("event") == "start":
            if cur_start is not None:
                covered.append((cur_start, t))  # crash epoch: approximate to next start
            cur_start = t
        else:  # stop
            if cur_start is not None:
                covered.append((cur_start, t))
                cur_start = None
    if cur_start is not None:
        covered.append((cur_start, NOW))

    def gap_overlaps(a, b):
        """True if [a,b] is not fully inside covered intervals."""
        cursor = a
        for (s, e) in covered:
            if e <= cursor:
                continue
            if s > cursor:
                return True  # hole before next coverage
            cursor = max(cursor, e)
            if cursor >= b:
                return False
        return cursor < b

    window = timedelta(days=WINDOW_DAYS)
    imported = lambda t: t.get("imported") is True

    # per-file later-touch index: (path -> list of (ended, grade, tool, op))
    touches = {}
    for t in turns:
        for fe in t.get("files") or []:
            touches.setdefault(fe["path"], []).append(
                (ts(t["ended"]), t.get("grade"), t.get("tool"), fe.get("op"))
            )
    for v in touches.values():
        v.sort(key=lambda x: x[0])

    denom = 0
    reworked = 0
    censored_recent = 0
    excluded_imported = 0
    excluded_gap = 0
    undo_unevaluable_c = 0
    denom_windows = []

    for t in turns:
        grade, tool = t.get("grade"), t.get("tool")
        ended = ts(t["ended"])
        write_events = [fe for fe in (t.get("files") or [])
                        if fe.get("op") in ("create", "write")]
        if not write_events:
            continue
        if imported(t):
            excluded_imported += len(write_events)
            continue
        if grade != "rich" or tool in ("git", "agentrec"):
            continue  # bare/git/undo turns: out of denominator entirely
        for fe in write_events:
            if NOW - ended < window:
                censored_recent += 1
                continue
            w_end = ended + window
            if gap_overlaps(ended, w_end):
                excluded_gap += 1
                continue
            denom += 1
            denom_windows.append((ended, w_end))
            # rework (a)+(b): later touch within window not covered by a rich turn
            hit = False
            for (te, tg, ttool, top) in touches.get(fe["path"], []):
                if te <= ended or te > w_end:
                    continue
                if tg == "rich" and ttool != "agentrec":
                    continue  # agent-covered change, not rework
                # bare touch (a), bare/any delete (b)
                hit = True
                break
            if hit:
                reworked += 1

    for t in turns:
        if t.get("tool") == "agentrec":
            e = ts(t["ended"])
            if any(a < e <= b for (a, b) in denom_windows):
                undo_unevaluable_c += 1

    measurable = denom
    rate = (reworked / measurable) if measurable else None
    print(f"turns={len(turns)} epochs={len(epochs)} covered_intervals={len(covered)}")
    print(f"window_days={WINDOW_DAYS}")
    print(f"measurable={measurable}")
    print(f"reworked={reworked}")
    print(f"rate={'n/a (no measurable events)' if rate is None else f'{rate:.1%}'}")
    print(f"censored_recent={censored_recent}")
    print(f"excluded_imported={excluded_imported}")
    print(f"excluded_gap={excluded_gap}")
    print("excluded_unknown_mtime=not computed by this spike")
    print(f"undo_unevaluable_c={undo_unevaluable_c}")


if __name__ == "__main__":
    main(sys.argv[1])
