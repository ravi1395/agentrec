# T0 spike — rework-rate viability on the real dogfood corpus

Date: 2026-08-07. Plan: `docs/superpowers/plans/2026-08-07-phase-3-0-read-wedge.md` Task 0.
Corpus: `~/Projects/agentrec/.agentrec/log.jsonl`, 4147 lines / 4126 turn records /
21 epoch records at run time (live daemon appending — absolutes are not gradeable
figures; ratios and the structural finding are the deliverables).

## Commands

```bash
python3 scripts/p30_rework_spike.py .agentrec/log.jsonl
```

Spec-as-written (any-gap-in-window exclusion):

```
measurable=0
reworked=0
rate=n/a (no measurable events)
censored_recent=622
excluded_imported=0
excluded_gap=1331
excluded_unknown_mtime=not computed by this spike
undo_unevaluable_c=0
```

Gaps-tolerated variant (inline fold, same denominator rules otherwise):

```
measurable=1331 reworked=419 rate=31.5% censored_recent=622 undo_unevaluable_c=0
```

Gap diagnosis (epoch-interval fold): ledger span 2026-07-11 → 2026-08-07,
covered 26.9d over 11 intervals; the 10 holes are 0.00h–6.23h, mostly
seconds–minutes (daemon restarts). Every 7-day window therefore contains a
gap → spec-as-written excludes ALL 1331 candidate events.

## Findings

1. **Spec §3.0.1's gap exclusion was a definition flaw** — same class as the
   bisect gap-fatality the spec gate killed at round 2 (NB2): any-gap-fatal
   semantics are inoperative on any repo whose daemon ever restarts.
   **Founder ruling (2026-08-07, in-session): tolerate gaps** — window stays
   measurable, `gap_overlapped` disclosure count, rate labeled lower bound
   (gap-hidden edits undercount rework, never overcount). Spec + plan amended
   in this commit; delta re-gated by the skeptic (scoped).
2. **T0 exit decision: rework stays headline** — 1331 measurable events ≥ 20
   under the ruled definition. Rate on dogfood: 31.5% (lower bound).
3. `excluded_imported=0` on this corpus (dogfood has no imported turns) — the
   bucket is exercised by fixtures, not this corpus; corpus-shape claims about
   imported repos remain unmeasured here (disclosed, not blocking: the bucket's
   spec role is disclosure).
4. `undo_unevaluable_c=0` here (no `tool:"agentrec"` undo turns inside
   denominator windows on this corpus); nonzero-on-real-corpora remains
   possible elsewhere — the T1 fixture covers it regardless.
5. Downstream field contract CONFIRMED: every field the fold needed exists as
   spec assumes — `type`, `grade`, `tool`, `model`, `ended`, `files[].op`
   (`create`/`write` observed), `files[].before/after`, epoch `event`/`ts`.
   `imported` absent on all dogfood turns (absent = not imported; the flag is
   PROTOCOL `imported: true` on imported turns only). No spec amendment needed
   on fields.
6. Spike limits, stated: log-only — `excluded_unknown_mtime` not computed
   (requires working-tree hashing, T1 scope); clause (c) structurally
   unevaluable pre-3.1; the gaps-tolerated variant's numerator counts bare
   touches and deletes within window (clauses a+b) exactly as T1 must.

## Baselines recorded (plan global-constraints row)

- Plan-start commit: `e1efbe5`.
- `rg -c 'load_log' cli/src`: cmds.rs 10, daemon.rs 5, importcmd.rs 5,
  memorycmds.rs 1, purgecmd.rs 13, readcmds.rs 2 (total 36).
- Suite baseline: see appended figure below (run completed after this doc's
  first draft; command `cargo test --workspace -- --test-threads=3`).
