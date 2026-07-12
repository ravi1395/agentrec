//! H++ adversarial torture harness (D36, IMPLEMENTATION.md:52/:175).
//!
//! Drives the REAL `agentrec` binary through randomized interleavings of
//! agent bursts, human edits, git checkouts, daemon `kill -9`, undo/redo,
//! and (Task 12) memory ops (`remember`/`recall`/`forget`/`candidate` +
//! pinned-file mutation), asserting invariants after EVERY executed undo
//! and after every op batch:
//!
//!   INV1 — undo never writes to a file whose on-disk hash differs from the
//!          target turn's recorded `after` (the D30 modified-since rail):
//!          undo must REFUSE modified-since files, never clobber them.
//!   INV2 — every executed undo is itself undoable back to the exact
//!          pre-undo state, byte-for-byte (undo-is-a-turn, re-revertible).
//!   INV-M1 — every `memory.jsonl` line parses as a `MemoryRecord` and
//!            carries >=1 pin (checked against the RAW file, not
//!            `load_effective`'s tolerant fold, so a torn kill-9 line can't
//!            hide behind that fold's silent skip — see
//!            `assert_memory_invariants`).
//!   INV-M2 — every memory `recall` returns re-hashes Fresh right now (no
//!            drifted pin ever surfaces) — see `op_recall`.
//!
//! `torture_survives_chaos` (the heavy run, `#[ignore]`) reads
//! `AGENTREC_TORTURE_OPS` (default 1200) and `AGENTREC_TORTURE_SEED` (default
//! a fixed constant) so a failure is reproducible: re-run with the printed
//! SEED. `torture_smoke` runs a small fixed op count in every `cargo test` so
//! the harness itself can never silently bitrot.
//!
//! PRNG: a ~10-line xorshift64* (no `rand` dep — not a dev-dependency of
//! this crate, and this harness doesn't need a "real" RNG, only a
//! deterministic, printable one).
//!
//! Any invariant violation panics with the full op trace and SEED for
//! reproduction — this harness never green-washes a real failure.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use agentrec_core::memory::{self, Freshness, MemoryRecord, Pin};
use agentrec_core::record::{LogRecord, TurnRecord};
use agentrec_core::store::hash_bytes;

// ---- binary-driving helpers (mirrors cli/tests/integration.rs) ------------

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentrec")
}

fn agentrec(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .args(["--root", root.to_str().unwrap()])
        .output()
        .expect("run agentrec")
}

fn spawn_record(root: &Path) -> Child {
    Command::new(bin())
        .args(["record", "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn record")
}

fn send_hook(root: &Path, payload: &str) {
    use std::io::Write;
    let mut child = Command::new(bin())
        .args(["hook", "claude", "--root", root.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait().unwrap();
}

fn sigkill(child: &Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
    }
}

fn poll_until<T>(timeout: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn load_turns(root: &Path) -> Vec<TurnRecord> {
    agentrec_core::record::load_log(&root.join(".agentrec/log.jsonl"))
        .into_iter()
        .filter_map(|r| match r {
            LogRecord::Turn(t) => Some(t),
            LogRecord::Epoch(_) => None,
        })
        .collect()
}

/// Turn ids absorbed by a later retroactive merge — excluded from checkpoint
/// undo target selection (mirrors `cmds::merged_ids`, reimplemented here
/// since the `agentrec` package is bin-only and exposes no lib to link
/// against from an integration test).
fn superseded_ids(root: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for t in load_turns(root) {
        for id in t.merges {
            out.insert(id);
        }
    }
    out
}

fn agentrec_tool_turns(root: &Path) -> Vec<TurnRecord> {
    load_turns(root)
        .into_iter()
        .filter(|t| t.tool.as_deref() == Some("agentrec"))
        .collect()
}

fn disk_bytes(root: &Path, rel: &str) -> Option<Vec<u8>> {
    std::fs::read(root.join(rel)).ok()
}

// ---- deterministic PRNG (xorshift64*) --------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1) // never all-zero state
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn gen_range(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

// ---- daemon lifecycle guard -------------------------------------------------

/// SIGKILL every process whose command line matches `agentrec record --root
/// <root>` — a belt-and-suspenders sweep for a daemon this harness lost the
/// `Child` handle to (a SIGKILL/respawn race can leave a superseded or
/// mid-spawn daemon reparented away from us, which `Child::wait` can then
/// never reap). Confined to THIS harness's own tempdir root, so it can never
/// touch an unrelated `agentrec record` running elsewhere on the machine.
fn pkill_by_root(root: &Path) {
    let pattern = format!("agentrec record --root {}", root.display());
    let _ = Command::new("pkill")
        .args(["-9", "-f", &pattern])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Owns EVERY `agentrec record` child this harness spawns for one world —
/// not just the current one. `Drop` SIGKILLs and reaps all of them, then
/// sweeps by root-path match for any handle-less survivor, so a torture
/// failure (or a normal finish) never leaves an orphan daemon or a zombie on
/// the machine. Drop fires during a panic unwind too.
struct DaemonGuard {
    root: PathBuf,
    /// All spawned children, live-or-dead, in spawn order. A killed+reaped
    /// child is removed; everything still here on Drop is force-killed.
    children: Vec<Child>,
}

impl DaemonGuard {
    fn spawn(root: &Path) -> Self {
        let child = spawn_record(root);
        std::thread::sleep(Duration::from_millis(800));
        DaemonGuard {
            root: root.to_path_buf(),
            children: vec![child],
        }
    }

    /// Kill+reap the live daemon, then spawn a fresh one. Any earlier child
    /// handles are also swept (they should already be reaped, but a prior
    /// respawn race may have left one) so `children` never accumulates
    /// unreaped handles across 45+ cycles.
    fn kill_and_respawn(&mut self, root: &Path) {
        for mut c in self.children.drain(..) {
            sigkill(&c);
            let _ = c.wait();
        }
        // Backstop: reparented/handle-less survivors from a prior race.
        pkill_by_root(&self.root);
        let child = spawn_record(root);
        std::thread::sleep(Duration::from_millis(800));
        self.children.push(child);
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        for mut c in self.children.drain(..) {
            sigkill(&c);
            let _ = c.wait();
        }
        // Catch anything we lost the handle to (reparented mid-race), so
        // `pgrep -fl "agentrec record"` is clean after every run.
        pkill_by_root(&self.root);
    }
}

// ---- world -------------------------------------------------------------------

/// Every op mutates real state on disk; the "expected content model" is
/// simply reading the real file back at the moment it matters (pre-undo
/// snapshot, post-undo/redo comparison) — the harness never trusts a
/// simulated copy over the real bytes.
struct World {
    // Field order is drop order: `daemon` must drop (kill every child +
    // root-match sweep) BEFORE `_tmp` removes the tempdir, otherwise a
    // still-live daemon would race the directory teardown.
    daemon: DaemonGuard,
    _tmp: tempfile::TempDir,
    root: PathBuf,
    rng: Rng,
    seed: u64,
    iter: usize,
    files: Vec<String>,
    /// Fixture pool for memory pins — deliberately disjoint from `files` (the
    /// undo-targeted pool): sharing a pool would make every recalled memory
    /// churn on unrelated undo/redo/git-dance activity, leaving `op_recall`
    /// almost nothing Fresh to check (INV-M2 would go under-exercised).
    /// `mem_files[0]` is the ANCHOR file — `op_mutate_pinned_file` and
    /// `op_forget` both deliberately never touch it (see their doc
    /// comments), so a Fresh, never-retracted memory is guaranteed to exist
    /// for the whole run, making `op_recall`'s empty-query fallback always
    /// have something real to verify.
    mem_files: Vec<String>,
    trace: Vec<String>,
    branch_a: String,
    on_branch_b: bool,
    stat_kills: u32,
    stat_checkpoints: u32,
    stat_undos_executed: u32,
    stat_undos_skipped: u32,
    stat_reverted_nothing: u32,
    stat_remembers: u32,
    stat_mutations: u32,
    stat_recalls: u32,
    stat_recall_hits: u32,
    stat_forgets: u32,
    stat_candidates: u32,
}

/// Shared vocabulary term every torture-authored fact carries, so
/// `op_recall`'s topic-query exercises real BM25 ranking (not just the
/// empty-query freshest-fallback path) — mirrors `memory.rs`'s own
/// `recall_never_returns_stale` fixture style.
const MEM_TOPIC: &str = "gremlin";
/// Marker substring on the anchor memory's fact — `op_forget` filters any
/// candidate carrying this marker out of its forget-target pool, so the
/// anchor memory (see `mem_files` doc comment) can never be retracted by
/// chaos.
const ANCHOR_MARKER: &str = "ANCHOR-DO-NOT-FORGET";

impl World {
    fn new(seed: u64) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();

        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg(&root)
            .status()
            .expect("git init");
        let out = agentrec(&root, &["init", "--no-hook", "--no-service"]);
        assert!(out.status.success(), "agentrec init failed: {out:?}");

        let branch_out = Command::new("git")
            .args([
                "-C",
                root.to_str().unwrap(),
                "symbolic-ref",
                "--short",
                "HEAD",
            ])
            .output()
            .expect("git symbolic-ref");
        let branch_a = String::from_utf8_lossy(&branch_out.stdout)
            .trim()
            .to_string();
        let branch_a = if branch_a.is_empty() {
            "main".to_string()
        } else {
            branch_a
        };

        // Seed the memory fixture pool BEFORE the daemon spawns — `remember`
        // is a direct, daemon-independent CLI write, so this doesn't race
        // daemon startup. mem_files[0] ("mem_anchor.rs") gets the one memory
        // `op_mutate_pinned_file`/`op_forget` are barred from ever touching
        // (see the `World::mem_files` doc comment) — the run-long Fresh
        // guarantee `op_recall`'s final `stat_recall_hits > 0` check relies
        // on.
        let mem_files: Vec<String> = vec![
            "mem_anchor.rs".into(),
            "mem0.rs".into(),
            "mem1.rs".into(),
            "mem2.rs".into(),
            "mem3.rs".into(),
        ];
        for f in &mem_files {
            std::fs::write(root.join(f), b"seed\n").expect("seed mem fixture");
        }
        let anchor_fact = format!("{ANCHOR_MARKER} torture baseline {MEM_TOPIC} fact");
        let out = agentrec(&root, &["remember", &anchor_fact, "--from", &mem_files[0]]);
        assert!(out.status.success(), "seed anchor remember failed: {out:?}");

        let daemon = DaemonGuard::spawn(&root);

        World {
            daemon,
            _tmp: tmp,
            root,
            rng: Rng::new(seed),
            seed,
            iter: 0,
            files: vec![
                "a.rs".into(),
                "b.rs".into(),
                "c.rs".into(),
                "d.rs".into(),
                "sub/e.rs".into(),
                "f.txt".into(),
                "g.md".into(),
                "h.json".into(),
            ],
            mem_files,
            trace: Vec::new(),
            branch_a,
            on_branch_b: false,
            stat_kills: 0,
            stat_checkpoints: 0,
            stat_undos_executed: 0,
            stat_undos_skipped: 0,
            stat_reverted_nothing: 0,
            stat_remembers: 1, // the anchor remember above
            stat_mutations: 0,
            stat_recalls: 0,
            stat_recall_hits: 0,
            stat_forgets: 0,
            stat_candidates: 0,
        }
    }
}

/// Raw chaos ops per burst before a settle attempt (see `settle_burst`).
const BURST_SIZE: usize = 15;
/// Raw ops between full undo/redo checkpoints — a multiple of `BURST_SIZE`
/// so a checkpoint always lands right after a burst has had a chance to
/// settle.
const CHECKPOINT_EVERY: usize = 60;

/// One randomized op: write, delete, git checkout dance, or daemon kill -9 +
/// respawn — the {agent bursts, human edits, git checkouts, daemon kill -9}
/// half of D36's op menu. Deliberately no per-op Stop-hook here: the
/// daemon's debounce (1.5s) and quiet-window (10s) are wall-clock
/// mechanisms, and firing a Stop hook on every op (faster than debounce can
/// ever settle) starves every burst into an empty "nothing pending"
/// attribution instead of a real one. Stop-hook signalling — and the
/// {undo/redo} half of the op menu — happens at the coarser `BURST_SIZE`
/// and `CHECKPOINT_EVERY` cadences instead (see `settle_burst`/`checkpoint`),
/// since every `agentrec undo --confirm` invocation also costs a mandatory
/// ~3s guard-linger in the real binary (H7) — interleaving either on every
/// single raw op would make a 1000+ op run take hours for no extra coverage.
fn step(world: &mut World) {
    match world.rng.gen_range(100) {
        // git-dance is deliberately rare (3%, not ~15%): PROTOCOL semantics
        // attribute an ENTIRE overlapping burst to tool="git" the moment a
        // ref transition lands anywhere in it (by design — see REVIEW.md),
        // so at 15% nearly every `BURST_SIZE`-sized window contained one and
        // over 80% of turns came out git-tagged (excluded from undo
        // targeting) — starving undo/redo coverage, not exercising it.
        //
        // write/delete are proportionally trimmed (was 70%/15%) to make room
        // for the Task 12 memory op mix below; git-dance and kill-daemon
        // keep their original percentages unchanged — this harness's
        // pre-existing INV1/INV2 coverage must not be diluted by adding
        // memory ops.
        0..=54 => op_write(world),
        55..=66 => op_delete(world),
        67..=69 => op_git_dance(world),
        70..=73 => op_kill_daemon(world),
        74..=78 => op_remember(world),
        79..=82 => op_mutate_pinned_file(world),
        83..=86 => op_recall(world),
        87..=88 => op_forget(world),
        89..=90 => op_candidate(world),
        _ => op_pause(world),
    }
}

fn op_write(world: &mut World) {
    let idx = world.rng.gen_range(world.files.len());
    let path = world.files[idx].clone();
    let content = format!("iter{}-{:x}\n", world.iter, world.rng.next_u64());
    let full = world.root.join(&path);
    if let Some(parent) = full.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&full, content).expect("write op");
    world.trace.push(format!("[{}] write {path}", world.iter));
}

fn op_delete(world: &mut World) {
    let existing: Vec<String> = world
        .files
        .iter()
        .filter(|p| world.root.join(p).exists())
        .cloned()
        .collect();
    if existing.is_empty() {
        return op_write(world);
    }
    let path = existing[world.rng.gen_range(existing.len())].clone();
    std::fs::remove_file(world.root.join(&path)).expect("delete op");
    world.trace.push(format!("[{}] delete {path}", world.iter));
}

fn op_git_dance(world: &mut World) {
    let root_str = world.root.to_str().unwrap();
    std::fs::write(
        world.root.join("git_marker.txt"),
        format!("commit {}\n", world.iter),
    )
    .expect("write git marker");
    let _ = Command::new("git")
        .args(["-C", root_str, "add", "-A"])
        .output();
    let _ = Command::new("git")
        .args([
            "-C",
            root_str,
            "-c",
            "user.name=torture",
            "-c",
            "user.email=torture@test.local",
            "commit",
            "-q",
            "-m",
            &format!("torture {}", world.iter),
        ])
        .output();
    if world.on_branch_b {
        let _ = Command::new("git")
            .args(["-C", root_str, "checkout", "-q", &world.branch_a])
            .output();
    } else {
        let _ = Command::new("git")
            .args(["-C", root_str, "checkout", "-q", "-B", "branch_b"])
            .output();
    }
    world.on_branch_b = !world.on_branch_b;
    world.trace.push(format!(
        "[{}] git-checkout (now on_branch_b={})",
        world.iter, world.on_branch_b
    ));
}

fn op_kill_daemon(world: &mut World) {
    world.daemon.kill_and_respawn(&world.root);
    world.stat_kills += 1;
    world
        .trace
        .push(format!("[{}] kill-9 + respawn daemon", world.iter));
}

fn op_pause(world: &mut World) {
    let ms = 20 + (world.rng.next_u64() % 80);
    std::thread::sleep(Duration::from_millis(ms));
    world.trace.push(format!("[{}] pause {ms}ms", world.iter));
}

// ---- Task 12: memory ops -----------------------------------------------

/// Writes fresh content to a randomly-chosen `mem_files` fixture, then
/// `agentrec remember`s a fact pinned to it. `remember` is a synchronous,
/// daemon-independent write — the content is fresh at the moment of the
/// call, so this must always succeed (no `.env`/`.pem`-shaped names in
/// `mem_files`, no secret-shaped fact text) — a failure here is a real bug,
/// not expected chaos, hence the hard `assert!`.
fn op_remember(world: &mut World) {
    let idx = world.rng.gen_range(world.mem_files.len());
    let path = world.mem_files[idx].clone();
    let content = format!("mem-iter{}-{:x}\n", world.iter, world.rng.next_u64());
    std::fs::write(world.root.join(&path), content).expect("write mem fixture");
    let fact = format!("torture fact {} about {MEM_TOPIC} ({path})", world.iter);
    let out = agentrec(&world.root, &["remember", &fact, "--from", &path]);
    assert!(
        out.status.success(),
        "remember failed unexpectedly: {out:?}\nSEED={}\n--- trace ---\n{}",
        world.seed,
        world.trace.join("\n"),
    );
    world.stat_remembers += 1;
    world
        .trace
        .push(format!("[{}] remember: {path}", world.iter));
}

/// Overwrites a pinned fixture's content — the mechanism that drifts any
/// memory pinned to it from Fresh to Stale. Never targets `mem_files[0]`
/// (the ANCHOR file — see `World::mem_files` doc comment), so the
/// run-long "at least one Fresh memory always exists" guarantee holds.
fn op_mutate_pinned_file(world: &mut World) {
    if world.mem_files.len() < 2 {
        return;
    }
    let idx = 1 + world.rng.gen_range(world.mem_files.len() - 1);
    let path = world.mem_files[idx].clone();
    let content = format!("mutated-iter{}-{:x}\n", world.iter, world.rng.next_u64());
    std::fs::write(world.root.join(&path), content).expect("mutate mem fixture");
    world.stat_mutations += 1;
    world
        .trace
        .push(format!("[{}] mutate-pinned-file: {path}", world.iter));
}

/// `agentrec recall <query> -k <k> --json`, parsed into `(id, pins)` pairs.
/// Empty stdout, a non-zero exit, or unparseable JSON all fold to "no hits"
/// — this helper never panics on its own (the caller decides what an empty
/// result means).
fn recall_json(root: &Path, query: &str, k: usize) -> Vec<(String, Vec<Pin>)> {
    let out = agentrec(root, &["recall", query, "-k", &k.to_string(), "--json"]);
    if !out.status.success() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return Vec::new();
    };
    let Some(arr) = v.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let pins_v = m.get("pins")?.as_array()?;
            let pins: Vec<Pin> = pins_v
                .iter()
                .filter_map(|p| {
                    Some(Pin {
                        path: p.get("path")?.as_str()?.to_string(),
                        hash: p.get("hash")?.as_str()?.to_string(),
                    })
                })
                .collect();
            Some((id, pins))
        })
        .collect()
}

/// INV-M2, load-bearing: every memory `recall` returns must re-hash Fresh
/// RIGHT NOW. Two queries per call: an empty query (the freshest-fallback
/// path — no BM25 score floor involved, so with the ANCHOR memory always
/// live this deterministically returns >=1 hit) and a topic-word query
/// (exercises real BM25 ranking; may legitimately floor to zero early in a
/// run before enough on-topic memories exist — never asserted nonempty on
/// its own).
fn op_recall(world: &mut World) {
    world.stat_recalls += 1;
    let empty_hits = recall_json(&world.root, "", 10);
    let topic_hits = recall_json(&world.root, MEM_TOPIC, 10);

    for (id, pins) in empty_hits.iter().chain(topic_hits.iter()) {
        world.stat_recall_hits += 1;
        let fresh = memory::pin_freshness(&world.root, pins);
        assert_eq!(
            fresh,
            Freshness::Fresh,
            "INV-M2 VIOLATED: recall returned memory {id} with a drifted \
             pin (freshness={fresh:?})\nSEED={}\n--- trace ---\n{}",
            world.seed,
            world.trace.join("\n"),
        );
    }
    world.trace.push(format!(
        "[{}] recall: {} empty-query hit(s), {} topic hit(s), all re-verified Fresh",
        world.iter,
        empty_hits.len(),
        topic_hits.len(),
    ));
}

/// Picks a random live (non-retracted, non-ANCHOR) memory via `agentrec
/// memories --json` and `agentrec forget`s it. No live eligible memory
/// (possible early in a run, or after a run of bad luck) is a skip, not a
/// failure — mirrors `checkpoint`'s "no eligible turn" skip style.
fn op_forget(world: &mut World) {
    let out = agentrec(&world.root, &["memories", "--json"]);
    if !out.status.success() {
        return;
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return;
    };
    let Some(arr) = v.as_array() else {
        return;
    };
    let ids: Vec<String> = arr
        .iter()
        .filter_map(|m| {
            let fact = m.get("fact").and_then(|f| f.as_str()).unwrap_or("");
            if fact.contains(ANCHOR_MARKER) {
                return None;
            }
            m.get("id").and_then(|i| i.as_str()).map(|s| s.to_string())
        })
        .collect();
    if ids.is_empty() {
        world.trace.push(format!(
            "[{}] forget: no eligible (non-anchor) memory, skipped",
            world.iter
        ));
        return;
    }
    let id = ids[world.rng.gen_range(ids.len())].clone();
    let out = agentrec(
        &world.root,
        &["forget", &id, "--reason", "torture chaos retraction"],
    );
    if out.status.success() {
        world.stat_forgets += 1;
        world.trace.push(format!("[{}] forget: {id}", world.iter));
    }
}

/// Writes fresh content to a random `mem_files` fixture, then `agentrec
/// candidate`s a fact pinned to it — the agent-authored write path, routed
/// through `signal.jsonl` for the daemon to validate/hash/ingest
/// (asynchronous: unlike `remember`, there's no guarantee the daemon has
/// ingested it by the time this returns — `assert_memory_invariants` at the
/// next burst boundary checks whatever has actually landed). The CLI call
/// itself only appends to the signal inbox and does not depend on daemon
/// liveness, but `DaemonGuard` (see its doc comment) guarantees a live
/// daemon child exists by the time `step` ever runs, so ingestion always
/// eventually happens — satisfying the brief's "only when daemon alive"
/// framing by construction.
fn op_candidate(world: &mut World) {
    let idx = world.rng.gen_range(world.mem_files.len());
    let path = world.mem_files[idx].clone();
    let content = format!("cand-iter{}-{:x}\n", world.iter, world.rng.next_u64());
    std::fs::write(world.root.join(&path), content).expect("write mem fixture (candidate)");
    let fact = format!(
        "agent-observed fact {} about {MEM_TOPIC} ({path})",
        world.iter
    );
    let out = agentrec(
        &world.root,
        &["candidate", &fact, "--from", &path, "--tool", "torture"],
    );
    assert!(
        out.status.success(),
        "candidate emit failed: {out:?}\nSEED={}\n--- trace ---\n{}",
        world.seed,
        world.trace.join("\n"),
    );
    world.stat_candidates += 1;
    world
        .trace
        .push(format!("[{}] candidate: {path}", world.iter));
}

/// INV-M1, load-bearing: reads `memory.jsonl` as RAW TEXT (not via
/// `memory::load_effective`, which silently skips malformed lines — correct
/// production tolerance, but it would make this check vacuously pass over a
/// torn kill-9 line). Every non-empty line must parse as a `MemoryRecord`
/// and carry >=1 pin. Called after every burst (including right after a
/// `kill_and_respawn`), so this is also the kill-9-recovery proof: fsynced
/// appends (`append_line_synced`) mean a SIGKILL mid-write can never leave a
/// torn line, and this assertion is what actually checks that promise.
fn assert_memory_invariants(world: &mut World) {
    let text = std::fs::read_to_string(memory::memory_path(&world.root)).unwrap_or_default();
    let mut n_lines = 0usize;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        n_lines += 1;
        let rec: MemoryRecord = serde_json::from_str(line).unwrap_or_else(|e| {
            panic!(
                "INV-M1 VIOLATED: memory.jsonl line {i} does not parse \
                 (torn/invalid line after kill-9?): {e}\nline: {line}\n\
                 SEED={}\n--- trace ---\n{}",
                world.seed,
                world.trace.join("\n"),
            )
        });
        assert!(
            !rec.pins.is_empty(),
            "INV-M1 VIOLATED: memory record {} has zero pins\nSEED={}\n--- trace ---\n{}",
            rec.id,
            world.seed,
            world.trace.join("\n"),
        );
    }
    world.trace.push(format!(
        "[{}] mem-invariants: {n_lines} memory.jsonl line(s) parsed OK, all with >=1 pin",
        world.iter
    ));
}

/// After a burst of raw ops, give the daemon's debounce (1.5s, plus
/// possible FSEvents coalescing lag) a bounded chance to stage the burst
/// into an open turn, then usually (not always — an unattributed burst is
/// realistic chaos too, left to the quiet-window bare-turn fallback) send a
/// Stop signal to close it as rich. Firing Stop before anything is staged
/// just produces an empty "nothing pending" turn (a real, documented
/// product behavior — not a bug) with none of `target.files`, so waiting
/// for `open.json` first is load-bearing here, not decorative.
fn settle_burst(world: &mut World) {
    let opened = poll_until(Duration::from_secs(3), || {
        world
            .root
            .join(".agentrec/open.json")
            .exists()
            .then_some(())
    });
    if opened.is_none() {
        world
            .trace
            .push(format!("[{}] burst-settle: nothing pending", world.iter));
        return;
    }
    if world.rng.gen_range(10) < 7 {
        send_hook(
            &world.root,
            &format!(
                r#"{{"hook_event_name":"Stop","session_id":"b_{}"}}"#,
                world.iter
            ),
        );
        world
            .trace
            .push(format!("[{}] burst-settle: stop-hook sent", world.iter));
    } else {
        world.trace.push(format!(
            "[{}] burst-settle: left open for quiet-window",
            world.iter
        ));
    }
}

/// Settle pending activity into a turn, pick a random eligible recorded turn
/// (excluding git turns and merge-superseded ids — scoping choice, not an
/// invariant weakening), undo it, assert INV1, then — if anything was
/// actually reverted — undo the undo (redo) and assert INV2.
fn checkpoint(world: &mut World) {
    world.stat_checkpoints += 1;
    // Give a still-forming burst a real chance to stage before forcing
    // attribution — same rationale as `settle_burst`, with a longer budget
    // since checkpoints are far rarer and need an actual closed turn to work
    // with, not just a best-effort nudge.
    let _ = poll_until(Duration::from_secs(10), || {
        world
            .root
            .join(".agentrec/open.json")
            .exists()
            .then_some(())
    });
    send_hook(
        &world.root,
        &format!(
            r#"{{"hook_event_name":"Stop","session_id":"cp_{}"}}"#,
            world.iter
        ),
    );
    world
        .trace
        .push(format!("[{}] checkpoint: settle", world.iter));

    let Some(turns) = poll_until(Duration::from_secs(8), || {
        let turns = load_turns(&world.root);
        (!turns.is_empty()).then_some(turns)
    }) else {
        world.stat_undos_skipped += 1;
        world.trace.push(format!(
            "[{}] checkpoint: no turns yet, skipped",
            world.iter
        ));
        return;
    };

    let superseded = superseded_ids(&world.root);
    let eligible: Vec<&TurnRecord> = turns
        .iter()
        .filter(|t| {
            t.tool.as_deref() != Some("git") && !superseded.contains(&t.id) && !t.files.is_empty()
        })
        .collect();
    if eligible.is_empty() {
        world.stat_undos_skipped += 1;
        world.trace.push(format!(
            "[{}] checkpoint: no eligible turn, skipped",
            world.iter
        ));
        return;
    }

    // Bias toward recent turns: with only 8 candidate paths shared across
    // every burst, an old turn's files are very likely rewritten many times
    // since (a legitimate, frequent INV1 "must refuse" exercise) — but
    // undo/redo (INV2) needs turns that are STILL current sometimes too, so
    // most picks favor the last few eligible turns and the rest sample
    // uniformly across all history.
    let target = if world.rng.gen_range(10) < 8 {
        let recent_n = eligible.len().min(3);
        eligible[eligible.len() - recent_n + world.rng.gen_range(recent_n)]
    } else {
        eligible[world.rng.gen_range(eligible.len())]
    };
    let target_id = target.id.clone();
    world.trace.push(format!(
        "[{}] checkpoint: undo target={target_id}",
        world.iter
    ));

    // Pre-undo snapshot of every path this turn touches — the source of
    // truth INV1/INV2 are checked against, not a simulated model.
    let pre: Vec<(String, Option<Vec<u8>>)> = target
        .files
        .iter()
        .map(|f| (f.path.clone(), disk_bytes(&world.root, &f.path)))
        .collect();

    let agentrec_before = agentrec_tool_turns(&world.root).len();
    let out = agentrec(&world.root, &["undo", &target_id, "--confirm"]);
    assert!(
        out.status.success(),
        "undo {target_id} failed\nSEED={}\nstdout: {}\nstderr: {}\n--- trace ---\n{}",
        world.seed,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        world.trace.join("\n"),
    );

    // INV1: any path modified-since (pre-hash != this turn's recorded
    // `after`) must be byte-identical before and after — the D30 rail must
    // refuse it, never clobber it.
    for f in &target.files {
        let pre_bytes = pre
            .iter()
            .find(|(p, _)| p == &f.path)
            .map(|(_, b)| b)
            .unwrap();
        let pre_hash = pre_bytes.as_ref().map(|b| hash_bytes(b));
        let modified_since = pre_hash != f.after;
        if modified_since {
            let post_bytes = disk_bytes(&world.root, &f.path);
            assert_eq!(
                &post_bytes,
                pre_bytes,
                "INV1 VIOLATED: undo wrote to modified-since file {} (turn {target_id})\nSEED={}\n--- trace ---\n{}",
                f.path,
                world.seed,
                world.trace.join("\n"),
            );
        }
    }

    // Which paths did this undo actually touch?
    let touched: Vec<String> = target
        .files
        .iter()
        .filter(|f| {
            let pre_bytes = pre
                .iter()
                .find(|(p, _)| p == &f.path)
                .map(|(_, b)| b)
                .unwrap();
            disk_bytes(&world.root, &f.path).as_ref() != pre_bytes.as_ref()
        })
        .map(|f| f.path.clone())
        .collect();

    if touched.is_empty() {
        world.stat_reverted_nothing += 1;
        world.trace.push(format!(
            "[{}] checkpoint: undo reverted nothing (refused/excluded)",
            world.iter
        ));
        return;
    }
    world.stat_undos_executed += 1;

    let agentrec_after = agentrec_tool_turns(&world.root);
    assert!(
        agentrec_after.len() > agentrec_before,
        "undo touched files but recorded no new turn (turn {target_id})\nSEED={}\n--- trace ---\n{}",
        world.seed,
        world.trace.join("\n"),
    );
    let new_undo_id = agentrec_after.last().unwrap().id.clone();
    world.trace.push(format!(
        "[{}] checkpoint: redo (undo-of-undo) target={new_undo_id}",
        world.iter
    ));

    let out2 = agentrec(&world.root, &["undo", &new_undo_id, "--confirm"]);
    assert!(
        out2.status.success(),
        "redo (undo of {new_undo_id}) failed\nSEED={}\nstdout: {}\nstderr: {}\n--- trace ---\n{}",
        world.seed,
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr),
        world.trace.join("\n"),
    );

    // INV2: undo-of-undo restores byte-for-byte the pre-undo state.
    for path in &touched {
        let pre_bytes = pre.iter().find(|(p, _)| p == path).map(|(_, b)| b).unwrap();
        let post_redo = disk_bytes(&world.root, path);
        assert_eq!(
            &post_redo,
            pre_bytes,
            "INV2 VIOLATED: undo-of-undo did not restore {path} byte-exact (turn {target_id}, undo {new_undo_id})\nSEED={}\n--- trace ---\n{}",
            world.seed,
            world.trace.join("\n"),
        );
    }
    world.trace.push(format!(
        "[{}] checkpoint: INV1+INV2 held for {} file(s)",
        world.iter,
        touched.len()
    ));
}

/// Runs `ops` randomized interleavings with periodic undo/redo checkpoints,
/// printing SEED at start and (with the full ordered op trace) on any panic.
fn run_torture(ops: usize, seed: u64) {
    println!("SEED={seed}");
    let mut world = World::new(seed);

    // Baseline: the anchor memory `World::new` just seeded must itself be a
    // valid, single-pin record before any chaos runs.
    assert_memory_invariants(&mut world);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for i in 0..ops {
            world.iter = i;
            step(&mut world);
            if (i + 1) % BURST_SIZE == 0 {
                settle_burst(&mut world);
                // INV-M1 (+ kill-9-recovery: no torn line survives a
                // `kill_and_respawn` that may have landed mid-batch), checked
                // after every op batch, per the brief.
                assert_memory_invariants(&mut world);
            }
            if (i + 1) % CHECKPOINT_EVERY == 0 {
                checkpoint(&mut world);
            }
        }
        world.iter = ops;
        checkpoint(&mut world); // final checkpoint over any trailing activity
        assert_memory_invariants(&mut world);
        // Unconditional final recall check — the randomized `op_recall`
        // (4% per op) may simply never fire on a short run (e.g.
        // `torture_smoke`'s 40 ops), and INV-M2 must still be genuinely
        // exercised every single run, not left to RNG luck.
        op_recall(&mut world);
        // The ANCHOR memory (see `World::mem_files`) guarantees at least one
        // Fresh hit always exists, so this must never be 0 — a violation
        // here means that guarantee itself broke, not that recall found
        // nothing to check.
        assert!(
            world.stat_recall_hits > 0,
            "INV-M2 was never exercised: every `op_recall` call returned \
             zero hits across the whole run (the ANCHOR-memory guarantee is \
             broken)\nSEED={}\n--- trace ---\n{}",
            world.seed,
            world.trace.join("\n"),
        );
    }));

    if let Err(payload) = result {
        eprintln!("SEED={seed}");
        eprintln!("--- op trace ({} entries) ---", world.trace.len());
        for line in &world.trace {
            eprintln!("{line}");
        }
        // `world` (and its DaemonGuard) is dropped as this unwind continues,
        // so the daemon child is always reaped even on failure.
        std::panic::resume_unwind(payload);
    }

    println!(
        "torture: {ops} ops completed, SEED={seed}, INV1+INV2 held on every undo, \
         INV-M1+INV-M2 held on every memory batch \
         (checkpoints={}, undos_executed(INV1+INV2 both checked)={}, \
         undos_reverted_nothing(INV1-only)={}, checkpoints_skipped={}, daemon_kills={}, \
         mem_remembers={}, mem_mutations={}, mem_recalls={}, mem_recall_hits(INV-M2 checked)={}, \
         mem_forgets={}, mem_candidates={})",
        world.stat_checkpoints,
        world.stat_undos_executed,
        world.stat_reverted_nothing,
        world.stat_undos_skipped,
        world.stat_kills,
        world.stat_remembers,
        world.stat_mutations,
        world.stat_recalls,
        world.stat_recall_hits,
        world.stat_forgets,
        world.stat_candidates,
    );
}

const DEFAULT_SEED: u64 = 20260710;

/// Resolve the run seed. An explicit `AGENTREC_TORTURE_SEED` is honored verbatim
/// (reproducible failure debugging). When UNSET — the nightly D36 launch-gate
/// case — derive from the wall clock so each of the 7 consecutive nights drives
/// a DIFFERENT interleaving; a fixed default would repeat one run 7× and never
/// broaden INV2 coverage. The resolved seed is printed by `run_torture`, so any
/// failing nightly is still reproducible by re-exporting it.
fn env_seed() -> u64 {
    if let Some(seed) = std::env::var("AGENTREC_TORTURE_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        return seed;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(DEFAULT_SEED);
    // Fold through the mixer so a low-entropy clock still spreads bits; never 0
    // (xorshift64* fixed point).
    (nanos ^ DEFAULT_SEED) | 1
}

fn env_ops() -> usize {
    std::env::var("AGENTREC_TORTURE_OPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1200)
}

/// Small, always-on smoke test: same engine, ~40 ops, fixed seed — proves the
/// harness compiles and runs on every `cargo test` without the heavy cost of
/// the full torture run.
#[test]
fn torture_smoke() {
    run_torture(40, DEFAULT_SEED ^ 0xC0FFEE);
}

/// The real torture run (D36 / H++): ≥1000 randomized ops by default,
/// asserting INV1+INV2 after every undo. `#[ignore]`d — run explicitly via
/// `cargo test --test torture -- --ignored --nocapture`.
#[test]
#[ignore]
fn torture_survives_chaos() {
    run_torture(env_ops(), env_seed());
}
