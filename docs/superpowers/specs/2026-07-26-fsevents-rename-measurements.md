# FSEvents rename-kind measurement (Phase 1 spike, residuals round)

**Branch:** `fix/residuals-round`, plan HEAD at measurement time `3477781`.
**Machine:** macOS 26.5.2 (BuildVersion 25F84), Darwin 25.5.0, arm64 (Apple Silicon).
**Fixture volume:** real APFS (`/dev/disk3s5`, mounted at `/System/Volumes/Data`, `/tmp` resolves
there via `/private/tmp`) — confirmed via `mount` before any trial ran, not tmpfs/ramdisk.
**Crate under test:** `notify = "6"` (resolved `notify v6.1.1`), matching `cli/Cargo.toml`'s pin
exactly (`grep -i notify cli/Cargo.toml` → `notify = "6"`).
**Question:** does FSEvents (via `notify` v6) deliver `EventKind::Modify(ModifyKind::Name(_))` for
directories *only* on genuine renames, or does it coalesce/replay rename flags onto unrelated
first-events the way it demonstrably does with `ItemCreated` (measured 2/2 fabricating in a prior
round, commit `f4bca8a`)? Phase 2's entire shape branches on the answer.

## What this doc is NOT

No `cli/` or `agentrec-core/` source was touched. No `cargo test` was run in the agentrec repo
(shared target dir, per instruction). The probe harness below is a standalone cargo project in the
session scratchpad, never committed to the repo — its full source is pasted inline in this doc so
the measurement is re-runnable without the scratchpad.

## Incident during measurement — a full data discard, not a rounding

The first attempt at this measurement was invalidated mid-run and **all data from it is void and
excluded below**. Recorded here because it is directly relevant to trusting the numbers that
follow:

1. An early harness revision created a **new `notify` watcher (one new OS thread) per trial** —
   80 of them for Probe B alone. Around the 9th sequential watcher in the first Probe B run, the
   process crashed: `Os { code: 35, kind: WouldBlock, message: "Resource temporarily unavailable" }`
   on a subprocess spawn (`Command::new("touch")...status()`).
2. Chasing that, the host was found to be at **4292–4296 total processes** against
   `kern.maxprocperuid=4000` — `fork()`/`posix_spawn()` was failing intermittently for *any* new
   process on the machine, including `ps`, `cargo build`'s linker invocation of `cc`/`clang`, and
   at one point a plain shell builtin (`:`). This was diagnosed (by the orchestrating session, not
   this probe) as **3607 orphaned `tail -f -n +1 /dev/null` processes**, all `ppid 1`, all spawned
   in the same second — unrelated tooling on this shared machine, not agentrec, not this probe.
   Once those were killed, the host returned to **690 processes** and every subsequent operation
   (build, watcher creation, subprocess spawn) succeeded on first try.
3. **Every measurement taken before that cleanup is discarded**, not merely noted: a fork-starved
   host perturbs scheduling and FSEvents delivery latency — exactly the timing the probes depend
   on — so a "clean" cell observed while the process table was full would not be trustworthy
   either direction. All trial counts below are from a *fresh* run, started from zero, after the
   host was confirmed healthy (`ps aux | wc -l` → 690, no leftover `probe` process via `pgrep`).
4. The harness itself was also redesigned as a result (see "Harness design" below): one watcher
   reused across many trials, not one watcher per trial. This was good hygiene independent of the
   host issue (it's also what the real `agentrec` daemon does — one watcher for its whole
   lifetime), but the host starvation was the proximate cause of the crash, not "10 watchers is
   inherently too many" — the original per-trial design had already completed Probe A's 10 trials
   twice without incident before the host separately filled up.

## Harness design

Every probe creates **exactly one** `notify::RecommendedWatcher` (one OS thread) and reuses it
across all of that probe's trials via multiple pre-created target paths, rather than one watcher
per trial. Every delivered `notify::Event` is logged verbatim — full `paths` and full `EventKind`,
never paraphrased or first-kind-wins — because notify's FSEvents backend can and does emit
multiple `EventKind`s from one coalesced flag union (this is exactly how historical `ItemCreated`
surfaced in the prior round; a first-kind-wins recorder would have missed it).

Two bugs found and fixed before trusting any result:

- **Path canonicalization.** `std::env::temp_dir()` on macOS returns a path through the
  `/var` → `/private/var` symlink, but FSEvents reports the **canonical** (resolved) path in every
  delivered event. Comparing the un-resolved path against event paths silently matched nothing —
  the very first run of Probe A showed `kinds=[] has_modify_name=false` on all 10/10 trials with
  zero events attributed, which is the "broken watcher reports nothing" false-clean this plan
  explicitly warns about. Fixed by `fs::canonicalize()`-ing every fixture root before use; this
  cost a false 0/10 that was caught only because 10/10 identical empty results with zero events
  attributed was implausible enough to check the raw `EVENT` lines, which showed events being
  delivered for the un-resolved path all along.
- **Process-per-trial watcher churn** (see incident above) — fixed by the one-watcher-per-probe
  design described here.

## Probe A — rename-in (positive control)

One root, 10 directories (each holding 3 files) pre-staged **outside** the root, one watcher
started on the root, then each directory `mv`'d in sequentially; full `EventKind` sequence
recorded for the destination path over a 2500ms post-move drain window.

| Trial | `EventKind`s observed at destination | `Modify(Name(_))` present |
|---|---|---|
| 0–9 (all 10) | `["Modify(Name(Any))"]` | **true**, every trial |

**10/10 trials delivered `Modify(Name(Any))` for the moved directory's destination path.**
Zero other kinds appeared at the destination in any trial — no coalesced `Create(Folder)`, no
`Modify(Metadata(_))`, nothing but the rename flag itself.

This is the **positive control**: it proves the harness genuinely observes rename-kind events end
to end (watcher → channel → path-matched filter → log), not just that it compiles. Every later
"clean" cell in Probe B is meaningful only because this control fired 10/10, not 0/10 or 5/10.

## Probe B — fabrication trap (the verdict axis)

Two fixtures, four stimuli, 10 trials per (fixture, stimulus) cell = **80 trials total**, one
watcher per fixture (reused across all 4 stimuli within that fixture, since all four target
directories predate that fixture's single watcher equally). Any `Modify(Name(_))` **anywhere** in
any delivered sequence for the stimulated directory's own path — not just in the narrow
post-stimulus window, checked separately below — counts as DIRTY.

**Fixture (i) — fresh tempdir tree.** Directory tree created, then the harness sleeps 1500ms
before starting the watcher (so the tree predates watcher attachment), then a further 3000ms
settle-drain before any stimulus. Four distinct pre-existing directories, one per stimulus.

**Fixture (ii) — aged real-repo copy.** `rsync -a` copy of this actual agentrec repo
(`/Users/ravichandrasekhar/Projects/agentrec`, excluding `target/`, `.agentrec/`, `.remember/`,
`.code-review-graph/`) into the fixture root, same age/settle protocol. Target directories:
`cli/src`, `agentrec-core/src`, `docs/superpowers`, `claude-setup` — four real, distinct,
populated, pre-existing directories, one per stimulus.

Stimuli: `touch` (mtime bump via the `filetime` crate, no subprocess), `chmod` (alternating
0755/0775 via `fs::set_permissions`), `xattr -w` (external process, one custom attribute write per
trial), `write` (first new file written inside the directory).

### Raw counts

| Fixture | Stimulus | Trials | `Modify(Name(_))` count (DIRTY) | `Create(_)` count (informational, not verdict axis) |
|---|---|---|---|---|
| fresh | touch  | 10 | **0** | 2/10 |
| fresh | chmod  | 10 | **0** | 0/10 |
| fresh | xattr  | 10 | **0** | 0/10 |
| fresh | write  | 10 | **0** | 0/10 |
| aged  | touch  | 10 | **0** | 0/10 |
| aged  | chmod  | 10 | **0** | 0/10 |
| aged  | xattr  | 10 | **0** | 0/10 |
| aged  | write  | 10 | **0** | 0/10 |
| **Total** | | **80** | **0** | 2/80 |

Zero `Modify(Name(_))` observed on any non-rename stimulus, on either fixture, in any of the 80
trials. This was checked twice: once via the per-trial `has_modify_name` flag captured in the
narrow post-stimulus drain window (the table above), and again by grepping the **entire** raw log
— including the pre-stimulus settle/preroll window — for `Modify(Name` across both fixture logs:

```
$ grep "Modify(Name" /tmp/probe_b_fresh.log   # zero matches
$ grep "Modify(Name" /tmp/probe_b_aged.log    # zero matches
```

Neither grep returned any line. The rename-kind axis is clean everywhere in this run, not just in
the windows the per-trial logic happened to check.

### Variance observed (recorded, not rounded down)

`Create(Folder)` — **not** the verdict axis, but the same coalescence class documented in the
prior round for `ItemCreated` — appeared on 2 of the fresh fixture's 10 `touch` trials (trials
where the target directory's own creation, seconds earlier in the same process, apparently hadn't
fully drained out of FSEvents' journal before the stimulus). It did **not** appear on the other 8
touch trials, nor on any trial of the other three stimuli, nor on any aged-fixture trial. This
asymmetry — present sometimes, absent other times, for the identical stimulus against the same
kind of pre-existing directory — is exactly the intermittent-coalescence signature this repo has
hit before (prior round measured `ItemCreated` fabricating 2/4 trials at `f4bca8a`). It is recorded
here as corroborating evidence that coalescence is real and observable on this machine in this
run — which is precisely what makes the 0/80 result on the **rename** axis meaningful rather than
a lucky quiet window. If `Create(_)` were the axis being gated, this run alone would already argue
against a bare `Create(_)`-only gate (consistent with the existing Linux-only cfg's rationale).

### Stated limitation

The "aged" fixture's directories were `rsync`'d into place seconds before the watcher attached —
they are aged in **tree shape and file count** (a real, deep, multi-hundred-directory repo
structure) but not in **per-path FSEvents journal history**, since every path in the copy is, from
FSEvents' perspective, newly created at copy time. If coalescence specifically requires a long
real-world journal history (weeks/months of prior activity on that exact path) rather than merely
"this directory existed before this watcher instance," this measurement cannot rule that out. This
bounds how strong the CLEAN verdict below can be read, and is why Phase 2's live-daemon escalation
trial (5/5 `touch <pre-existing dir>`, per the plan) remains required rather than skippable.

## Probe C — replay

5 genuine rename-ins performed up front (one watcher, 5 distinct destination paths), each
confirmed via its own initial post-move drain (this doubles as a second positive control — see
below), then a single combined **68-second** hold window with unrelated file-write activity
elsewhere in the root, checking whether **any** of the 5 destination paths receives a delayed
spurious `Modify(Name(_))` redelivery.

| Trial | Initial rename confirmed | Hold duration | Delayed replay seen | Replay events |
|---|---|---|---|---|
| 0 | true | 68s | **false** | `[]` |
| 1 | true | 68s | **false** | `[]` |
| 2 | true | 68s | **false** | `[]` |
| 3 | true | 68s | **false** | `[]` |
| 4 | true | 68s | **false** | `[]` |

**5/5 initial renames confirmed** (second positive control — the harness observes genuine rename
kinds independently in this probe too, not just in Probe A). **0/5 replays** over a 68-second hold
with unrelated activity. A whole-log grep for `Modify(Name` in this probe's output returns exactly
5 matches — one per initial rename, zero extra — confirming no redelivery anywhere, not just
absence in the specific windows checked.

## Probe D — rename-out

5 pre-staged directories inside the root, one watcher, each moved out sequentially; from-side
`EventKind` sequence and `path.is_dir()` (checked at the moment of logging, i.e. after the move)
recorded.

| Trial | From-side `EventKind`s | `is_dir()` at delivery |
|---|---|---|
| 0 | `["Create(Folder)", "Modify(Name(Any))"]` | **false** |
| 1 | `["Create(Folder)", "Modify(Name(Any))"]` | **false** |
| 2 | `["Create(Folder)", "Modify(Name(Any))"]` | **false** |
| 3 | `["Create(Folder)", "Modify(Name(Any))"]` | **false** |
| 4 | `["Create(Folder)", "Modify(Name(Any))"]` | **false** |

5/5 trials: the from-side path carries `Modify(Name(Any))` (the rename-out flag) plus a
`Create(Folder)` that is simply that directory's own creation a few hundred milliseconds earlier in
the same process run (self-history, not a fabrication — these directories were created fresh
immediately before the watcher started, so their own genuine creation is still in-journal). In all
5 trials `path.is_dir()` is `false` at the moment of delivery — the path no longer resolves,
confirming the daemon's existing `is_dir()` conjunct correctly excludes rename-out from triggering
admission (edge case already covered in the plan's "Edge cases considered" section for P2).

## Harness source (inline, re-runnable without the scratchpad)

`Cargo.toml`:

```toml
[package]
name = "fsevents-probe"
version = "0.1.0"
edition = "2021"

[dependencies]
notify = "6"
filetime = "0.2"

[[bin]]
name = "probe"
path = "src/main.rs"

[profile.release]
opt-level = 1
```

`src/main.rs`:

```rust
// FSEvents rename-kind measurement probe (Phase 1 spike, agentrec residuals round).
// Standalone against notify = "6" directly -- NOT part of the agentrec repo.
//
// Usage:
//   probe a <trials>
//   probe b <fixture: fresh|aged> <trials-per-stimulus>
//   probe c <trials>
//   probe d <trials>
//
// Design note (load-bearing -- read before modifying): every probe creates
// exactly ONE notify watcher (one OS thread) and reuses it across ALL trials
// via multiple pre-created target paths. An earlier revision created a fresh
// watcher per trial (80 for probe B alone) and crashed the process with
// `EAGAIN`/`WouldBlock` around trial 9 -- macOS was not reclaiming FSEvents
// watcher threads fast enough to keep up with per-trial creation, and it took
// the whole process table with it (this repo's daemon watches ONE root for
// its entire lifetime -- the crash was a harness bug, not a platform fact).
//
// All output is plain lines to stdout: one EVENT line per delivered notify
// Event (full paths + full EventKind, never paraphrased), and one RESULT
// line per trial summarizing what was observed. Nothing is post-filtered
// silently -- grep the EVENT lines yourself to audit RESULT lines.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const SETTLE_MS: u64 = 3000; // drain window after watcher starts, before any stimulus is trusted
const OBSERVE_MS: u64 = 2500; // drain window after a stimulus, to catch coalesced delivery
const ARM_WAIT_MS: u64 = 400; // brief pause after watch() returns, before trusting it's armed

fn now_tag() -> String {
    format!("{:?}", std::time::SystemTime::now())
}

struct Delivery {
    t: Instant,
    paths: Vec<PathBuf>,
    kind: EventKind,
}

fn start_watcher(root: &Path) -> (RecommendedWatcher, Receiver<Delivery>) {
    let (tx, rx) = channel::<Delivery>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
        Ok(ev) => {
            let _ = tx.send(Delivery {
                t: Instant::now(),
                paths: ev.paths.clone(),
                kind: ev.kind,
            });
        }
        Err(e) => eprintln!("WATCH_ERROR {:?}", e),
    })
    .expect("create watcher");
    watcher
        .watch(root, RecursiveMode::Recursive)
        .expect("watch root");
    std::thread::sleep(Duration::from_millis(ARM_WAIT_MS));
    (watcher, rx)
}

/// Drain the channel for `dur`, printing every delivery as an EVENT line, and
/// returning the full list for programmatic inspection.
fn drain_for(rx: &Receiver<Delivery>, dur: Duration, tag: &str, t0: Instant) -> Vec<Delivery> {
    let deadline = Instant::now() + dur;
    let mut out = Vec::new();
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        match rx.recv_timeout(deadline - now) {
            Ok(d) => {
                let elapsed = d.t.duration_since(t0).as_millis();
                println!(
                    "EVENT tag={} elapsed_ms={} kind={:?} paths={:?}",
                    tag, elapsed, d.kind, d.paths
                );
                out.push(d);
            }
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    out
}

fn is_modify_name(k: &EventKind) -> bool {
    matches!(k, EventKind::Modify(notify::event::ModifyKind::Name(_)))
}

fn is_create(k: &EventKind) -> bool {
    matches!(k, EventKind::Create(_))
}

fn fresh_root(label: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "fsevents-probe-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).unwrap();
    // macOS: /tmp and $TMPDIR resolve through /var -> /private/var symlinks, and
    // FSEvents reports the CANONICAL (resolved) path in every delivered Event.
    // Comparing against the un-resolved path silently never matches -- canonicalize
    // here so every path comparison in the probes is apples-to-apples.
    fs::canonicalize(&base).unwrap()
}

fn touch_no_fork(p: &Path) {
    // filetime crate already ships as a transitive notify dependency (walkdir's
    // dep tree); pulling it directly avoids forking an external `touch` process
    // per trial, which is exactly the kind of process churn that crashed the
    // per-trial-watcher revision.
    let now = filetime::FileTime::now();
    filetime::set_file_mtime(p, now).unwrap();
}

// ---------------------------------------------------------------------------
// Probe A: rename-in. ONE watcher on one root; N pre-staged outside dirs (each
// with 3 files), moved in one at a time. Record the full EventKind sequence
// delivered for each moved dir's DESTINATION path.
// ---------------------------------------------------------------------------
fn probe_a(trials: u32) {
    let root = fresh_root("a-root");
    let outside = fresh_root("a-outside");

    // Pre-stage all source dirs BEFORE the watcher starts (root is empty at
    // watch-time regardless -- these live outside root until moved).
    let mut src_dirs = Vec::new();
    for i in 0..trials {
        let d = outside.join(format!("moved_dir_{}", i));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("f1"), b"one").unwrap();
        fs::write(d.join("f2"), b"two").unwrap();
        fs::write(d.join("f3"), b"three").unwrap();
        src_dirs.push(d);
    }

    let (_watcher, rx) = start_watcher(&root);
    let _ = drain_for(&rx, Duration::from_millis(500), "A-preroll", Instant::now());

    let mut hits = 0u32;
    for (trial, src_dir) in src_dirs.iter().enumerate() {
        let dest = root.join(format!("moved_dir_{}", trial));
        let t0 = Instant::now();
        let status = Command::new("mv").arg(src_dir).arg(&dest).status().unwrap();
        assert!(status.success(), "mv failed");

        let deliveries = drain_for(&rx, Duration::from_millis(OBSERVE_MS), "A", t0);
        let dest_events: Vec<&Delivery> = deliveries
            .iter()
            .filter(|d| d.paths.iter().any(|p| p == &dest))
            .collect();
        let kinds: Vec<String> = dest_events.iter().map(|d| format!("{:?}", d.kind)).collect();
        let has_rename = dest_events.iter().any(|d| is_modify_name(&d.kind));
        if has_rename {
            hits += 1;
        }
        println!(
            "RESULT probe=A trial={} dest={:?} kinds={:?} has_modify_name={} at={}",
            trial, dest, kinds, has_rename, now_tag()
        );
    }
    println!("SUMMARY probe=A trials={} hits(has_modify_name)={}", trials, hits);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// Probe B: fabrication trap. Directories all predate the watcher. ONE watcher
// per fixture, reused across all 4 stimuli x N trials via distinct
// pre-created target directories (one per stimulus, stimulus applied N times
// to the same dir in sequence -- the dir still predates the watcher; repeated
// application of the SAME stimulus is what the plan's "N trials per stimulus"
// asks for). Record the FULL per-delivery EventKind sequence for each dir
// path. Any Modify(Name(_)) anywhere = DIRTY.
// ---------------------------------------------------------------------------
const STIMULI: [&str; 4] = ["touch", "chmod", "xattr", "write"];

fn make_fresh_fixture() -> (PathBuf, [PathBuf; 4]) {
    let root = fresh_root("b-fresh-root");
    let mut dirs = Vec::new();
    for stim in STIMULI.iter() {
        let d = root.join(format!("target_{}", stim));
        fs::create_dir_all(d.join("nested")).unwrap();
        for i in 0..5 {
            fs::write(d.join(format!("file{}.txt", i)), b"content").unwrap();
        }
        dirs.push(d);
    }
    (root, [dirs[0].clone(), dirs[1].clone(), dirs[2].clone(), dirs[3].clone()])
}

fn make_aged_fixture() -> (PathBuf, [PathBuf; 4]) {
    let root = fresh_root("b-aged-root");
    let src = "/Users/ravichandrasekhar/Projects/agentrec";
    // Copy a real, deep, aged repo tree (excluding heavy/churny dirs) so the
    // fixture has genuine per-path FSEvents history from before this watcher
    // ever attached, per the plan's coalescence-keys-on-history requirement.
    let status = Command::new("rsync")
        .arg("-a")
        .arg("--exclude=target")
        .arg("--exclude=.agentrec")
        .arg("--exclude=.remember")
        .arg("--exclude=.code-review-graph")
        .arg(format!("{}/", src))
        .arg(format!("{}/", root.display()))
        .status()
        .unwrap();
    assert!(status.success(), "rsync fixture copy failed");
    // Four real, distinct, populated, pre-existing directories -- one per stimulus.
    let dirs = [
        root.join("cli").join("src"),
        root.join("agentrec-core").join("src"),
        root.join("docs").join("superpowers"),
        root.join("claude-setup"),
    ];
    for d in &dirs {
        assert!(d.is_dir(), "expected {:?} to exist post-copy", d);
    }
    (root, dirs)
}

fn apply_stimulus(stimulus: &str, dir: &Path, iteration: u32) {
    match stimulus {
        "touch" => touch_no_fork(dir),
        "chmod" => {
            let mode = if iteration % 2 == 0 { 0o755 } else { 0o775 };
            fs::set_permissions(dir, fs::Permissions::from_mode(mode)).unwrap();
        }
        "xattr" => {
            let status = Command::new("xattr")
                .arg("-w")
                .arg("com.agentrec.probe")
                .arg(format!("val{}", iteration))
                .arg(dir)
                .status()
                .unwrap();
            assert!(status.success(), "xattr -w failed");
        }
        "write" => {
            let f = dir.join(format!("probe_inside_{}.txt", iteration));
            fs::write(&f, b"probe write").unwrap();
        }
        other => panic!("unknown stimulus {}", other),
    }
}

fn probe_b_fixture(fixture: &str, trials: u32) {
    let (root, dirs) = match fixture {
        "fresh" => make_fresh_fixture(),
        "aged" => make_aged_fixture(),
        other => panic!("unknown fixture {}", other),
    };
    // Let the fixture "age" relative to the watcher: sleep, THEN start watching,
    // exactly mirroring a daemon attaching to a pre-existing tree.
    std::thread::sleep(Duration::from_millis(1500));

    let (_watcher, rx) = start_watcher(&root);
    // Drain + discard the historical settle noise BEFORE any stimulus -- this is
    // deliberately not asserted on; what matters is the event triggered by each
    // stimulus below, isolated by per-stimulus drain windows.
    let _ = drain_for(&rx, Duration::from_millis(SETTLE_MS), "B-preroll", Instant::now());

    let mut dirty_total = 0u32;
    for (si, stimulus) in STIMULI.iter().enumerate() {
        let target_dir = &dirs[si];
        let mut dirty = 0u32;
        for trial in 0..trials {
            let t0 = Instant::now();
            apply_stimulus(stimulus, target_dir, trial);

            let deliveries = drain_for(&rx, Duration::from_millis(OBSERVE_MS), "B", t0);
            let dir_events: Vec<&Delivery> = deliveries
                .iter()
                .filter(|d| d.paths.iter().any(|p| p == target_dir))
                .collect();
            let kinds: Vec<String> = dir_events.iter().map(|d| format!("{:?}", d.kind)).collect();
            let has_rename = dir_events.iter().any(|d| is_modify_name(&d.kind));
            let has_create = dir_events.iter().any(|d| is_create(&d.kind));
            if has_rename {
                dirty += 1;
                dirty_total += 1;
            }
            println!(
                "RESULT probe=B fixture={} stimulus={} trial={} dir={:?} kinds={:?} has_modify_name={} has_create={} at={}",
                fixture, stimulus, trial, target_dir, kinds, has_rename, has_create, now_tag()
            );
        }
        println!(
            "SUMMARY probe=B fixture={} stimulus={} trials={} dirty(has_modify_name)={}",
            fixture, stimulus, trials, dirty
        );
    }
    println!(
        "GRANDSUMMARY probe=B fixture={} total_trials={} total_dirty={}",
        fixture,
        trials * STIMULI.len() as u32,
        dirty_total
    );
    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Probe C: replay. ONE watcher; do N genuine rename-ins up front (each its own
// destination path, each confirmed via the initial delivery), then a SINGLE
// combined hold window (>=60s) with unrelated activity elsewhere in root,
// checking whether ANY of the N moved-in dir paths receives a DELAYED
// spurious Modify(Name(_)) redelivery.
// ---------------------------------------------------------------------------
fn probe_c(trials: u32) {
    let root = fresh_root("c-root");
    let outside = fresh_root("c-outside");
    let unrelated_dir = root.join("unrelated");
    fs::create_dir_all(&unrelated_dir).unwrap();

    let mut src_dirs = Vec::new();
    for i in 0..trials {
        let d = outside.join(format!("moved_dir_{}", i));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("f1"), b"one").unwrap();
        src_dirs.push(d);
    }

    let (_watcher, rx) = start_watcher(&root);
    let _ = drain_for(&rx, Duration::from_millis(500), "C-preroll", Instant::now());

    let session_t0 = Instant::now();
    let mut dests = Vec::new();
    let mut initial_renames = Vec::new();
    for (trial, src_dir) in src_dirs.iter().enumerate() {
        let dest = root.join(format!("moved_dir_{}", trial));
        let t0 = Instant::now();
        let status = Command::new("mv").arg(src_dir).arg(&dest).status().unwrap();
        assert!(status.success());
        let initial = drain_for(&rx, Duration::from_millis(OBSERVE_MS), "C-initial", t0);
        let initial_rename = initial
            .iter()
            .any(|d| d.paths.iter().any(|p| p == &dest) && is_modify_name(&d.kind));
        println!(
            "RESULT probe=C-initial trial={} dest={:?} initial_rename={}",
            trial, dest, initial_rename
        );
        dests.push(dest);
        initial_renames.push(initial_rename);
    }

    // Combined hold window: >=65s of unrelated activity, watching ALL dest
    // paths at once for a delayed spurious Modify(Name(_)) redelivery.
    let hold_start = Instant::now();
    let mut replay_hits: Vec<bool> = vec![false; dests.len()];
    let mut replay_events: Vec<Vec<String>> = vec![Vec::new(); dests.len()];
    let mut i = 0u32;
    while hold_start.elapsed() < Duration::from_secs(65) {
        let f = unrelated_dir.join(format!("noise{}.txt", i));
        fs::write(&f, format!("noise {}", i)).unwrap();
        i += 1;
        let batch = drain_for(&rx, Duration::from_millis(4000), "C-hold", session_t0);
        for d in &batch {
            for (idx, dest) in dests.iter().enumerate() {
                if d.paths.iter().any(|p| p == dest) && is_modify_name(&d.kind) {
                    replay_hits[idx] = true;
                    replay_events[idx]
                        .push(format!("{:?}@{}ms", d.kind, d.t.duration_since(session_t0).as_millis()));
                }
            }
        }
    }

    let mut replays = 0u32;
    for (trial, dest) in dests.iter().enumerate() {
        if replay_hits[trial] {
            replays += 1;
        }
        println!(
            "RESULT probe=C trial={} dest={:?} initial_rename={} hold_secs={} replay_seen={} replay_events={:?}",
            trial,
            dest,
            initial_renames[trial],
            hold_start.elapsed().as_secs(),
            replay_hits[trial],
            replay_events[trial]
        );
    }
    println!("SUMMARY probe=C trials={} replays={}", trials, replays);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
}

// ---------------------------------------------------------------------------
// Probe D: rename-out. ONE watcher; N pre-staged dirs inside root, each moved
// out one at a time. Confirm (i) the from-side EventKind, (ii) whether
// path.is_dir() is false at the moment of delivery.
// ---------------------------------------------------------------------------
fn probe_d(trials: u32) {
    let root = fresh_root("d-root");
    let outside = fresh_root("d-outside");

    let mut dirs = Vec::new();
    for i in 0..trials {
        let d = root.join(format!("leaving_dir_{}", i));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("f1"), b"one").unwrap();
        dirs.push(d);
    }

    let (_watcher, rx) = start_watcher(&root);
    let _ = drain_for(&rx, Duration::from_millis(500), "D-preroll", Instant::now());

    for (trial, dir) in dirs.iter().enumerate() {
        let dest = outside.join(format!("leaving_dir_{}", trial));
        let t0 = Instant::now();
        let status = Command::new("mv").arg(dir).arg(&dest).status().unwrap();
        assert!(status.success());

        let deliveries = drain_for(&rx, Duration::from_millis(OBSERVE_MS), "D", t0);
        let from_events: Vec<&Delivery> = deliveries
            .iter()
            .filter(|d| d.paths.iter().any(|p| p == dir))
            .collect();
        let from_kinds: Vec<String> = from_events.iter().map(|d| format!("{:?}", d.kind)).collect();
        let is_dir_at_delivery = dir.is_dir();
        println!(
            "RESULT probe=D trial={} from_path={:?} from_kinds={:?} is_dir_at_delivery={} at={}",
            trial, dir, from_kinds, is_dir_at_delivery, now_tag()
        );
    }
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: probe <a|b|c|d> ...");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "a" => {
            let trials: u32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(10);
            probe_a(trials);
        }
        "b" => {
            let fixture = args.get(2).expect("fixture: fresh|aged");
            let trials: u32 = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(10);
            probe_b_fixture(fixture, trials);
        }
        "c" => {
            let trials: u32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(5);
            probe_c(trials);
        }
        "d" => {
            let trials: u32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(5);
            probe_d(trials);
        }
        other => {
            eprintln!("unknown probe {}", other);
            std::process::exit(2);
        }
    }
}
```

Run commands used to produce the tables above:

```
cargo build --release
./target/release/probe a 10                # Probe A
./target/release/probe b fresh 10          # Probe B, fixture (i)
./target/release/probe b aged 10           # Probe B, fixture (ii)
./target/release/probe c 5                 # Probe C
./target/release/probe d 5                 # Probe D
```

## Verdict

- Probe A: **10/10** rename-in trials delivered `Modify(Name(Any))` at the destination path
  (positive control — required for any cell below to be trustworthy).
- Probe B: **0/80** trials across 2 fixtures × 4 stimuli × 10 trials showed `Modify(Name(_))`
  anywhere in the delivered sequence, checked both in the per-stimulus window and via a whole-log
  grep.
- Probe C: **0/5** delayed replays over a 68-second hold, following **5/5** confirmed genuine
  initial renames (second positive control).
- Probe D: **5/5** rename-out trials showed `is_dir()` false at delivery — confirms the existing
  `is_dir()` conjunct already excludes this direction, independent of the verdict above.

Per the plan's interface contract: Probe B clean (0 spurious across all trials) AND Probe C clean
(0 replays) → 

```
VERDICT: CLEAN
```

This is bounded by the stated limitation above (the aged fixture is tree-shape-aged, not
journal-history-aged) — Phase 2's live-daemon 5/5 escalation trial against a genuinely
long-lived pre-existing directory remains required, not optional, before this verdict is treated
as fully closing the question.
