# T0 spike — rework-rate viability on the real dogfood corpus

Date: 2026-08-07/08 (re-run after delta-gate D1–D3). Plan:
`docs/superpowers/plans/2026-08-07-phase-3-0-read-wedge.md` Task 0.
Corpus: `~/Projects/agentrec/.agentrec/log.jsonl` — live daemon appending;
absolutes drift between runs (4126→4134 turns within the session) and are
NOT gradeable figures; ratios and structural findings are the deliverables.

## Figures of record (corrected script, committed, both runs 2026-08-08)

```bash
python3 scripts/p30_rework_spike.py .agentrec/log.jsonl
```
```
turns=4133 epochs=21 covered_intervals=11
window_days=7 gap_rule=tolerate (amended)
measurable=2420
reworked=1335
rate=55.2%
censored_recent=1854
excluded_imported=0
gap_overlapped=2420 (disclosure only; rate is approximate lower bound)
excluded_unknown_mtime=not computed by this spike
undo_unevaluable_c=0
```

```bash
python3 scripts/p30_rework_spike.py .agentrec/log.jsonl 7 --exclude-gaps
```
```
measurable=0 ... gap_overlapped=2420 (excluded under spec v1 rule)
```

## Correction history (both caught by the scoped delta gate, not by the author)

1. **D1 — op enum was wrong in spec AND spike AND this doc's first version.**
   Spec draft said write events = "create or write op"; the wire enum is
   `create|modify|delete` (PROTOCOL §5; measured census: create 4088 /
   modify 5326 / delete 2473 / "write" 0). The first spike silently dropped
   every `modify` — the majority of agent writes. First-version figures
   (measurable=1331, rate 31.5%) are RETIRED; never quote them. This doc's
   first Finding 5 certified the field contract "confirmed" with
   "`create`/`write` observed" — false, the signature defect class, sitting
   inside the exit criterion built to catch it. Spec amended to
   `create|modify`; script filter fixed.
2. **D3 — the 31.5% headline had come from an uncommitted inline variant**
   while the committed script still implemented the pre-amendment exclusion.
   The committed script now implements the amended rule by default and
   `--exclude-gaps` reproduces the spec-v1 measurable=0 finding.
3. **D2 — "never overcounted" was falsifiable.** Gaps also strip bracket
   coverage (dropped `start` → post-restart activity mints bare turns →
   clause (a) counts them). Spec label amended to "approximate lower bound"
   with both channels named.

## Findings (as corrected)

1. **Gap rule:** spec-as-written any-gap-in-window exclusion → measurable=0
   (restart gaps are seconds–minutes: 10 holes, 0.00h–6.23h, 26.9d covered of
   a 27d span; every 7-day window overlaps one). **Founder ruling: tolerate
   gaps** — `gap_overlapped` is disclosure, rate is an approximate lower
   bound. On this corpus gap_overlapped == measurable (2420): every window
   overlaps some sliver, which is exactly why the v1 rule was inoperative.
2. **T0 exit decision: rework stays headline** — 2420 measurable ≥ 20.
   Dogfood rate 55.2% (approximate lower bound, window 7d).
3. **Field contract: CONFIRMED only after correction** — `type`, `grade`,
   `tool`, `model`, `ended`, `files[].op ∈ {create,modify,delete}`,
   `files[].before/after`, epoch `event`/`ts`. `imported` absent on all
   dogfood turns (flag appears only on imported turns). The op-enum mismatch
   was the exact "mismatch = spec amendment before T1" case; amendment done
   and re-gated.
4. `excluded_imported=0` and `undo_unevaluable_c=0` on this corpus (no
   imported turns; zero `tool:"agentrec"` turns — tool census rich/claude
   2613, bare 1390, rich/git 125 at gate time). Both buckets are exercised by
   T1 fixtures, not this corpus.
5. **Spike approximations, disclosed:** crash epochs are approximated as
   covered up to the next epoch start (overstates coverage — safe direction
   for finding 1, which excluded everything even so); T1 must use
   `view::recording_gaps`, which tags Crash properly. Numerator treats ANY
   rich turn as coverage (including hypothetical undo turns — none exist
   here); clauses (a)+(b) only, (c) structurally unevaluable pre-3.1.
   Log-only: `excluded_unknown_mtime` not computed.

## Baselines recorded (plan global-constraints row)

- Plan-start commit: `e1efbe5`.
- `rg -c 'load_log' cli/src`: cmds.rs 10, daemon.rs 5, importcmd.rs 5,
  memorycmds.rs 1, purgecmd.rs 13, readcmds.rs 2 (total 36).
- Suite baseline: **1035 passed / 0 failed / 4 ignored** (19 test-binary
  result lines summed; `cargo test --workspace -- --test-threads=3` at
  `aadefc5`). A first capture attempt piped through `tail -30` and saw only
  the last binary (230) — discarded, not a suite figure.
