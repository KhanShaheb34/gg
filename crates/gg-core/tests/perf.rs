//! Speed targets for the core on large synthetic repositories (spec §7, plan task 10).
//!
//! Ignored by default and meaningful only in a release build:
//!
//! ```text
//! scripts/gen-synthetic-repo.sh ~/.cache/gitgraph-perf/r20k 20000 200
//! scripts/gen-synthetic-repo.sh ~/.cache/gitgraph-perf/r100k 100000 400
//! GG_PERF_R20K="$HOME/.cache/gitgraph-perf/r20k" GG_PERF_R100K="$HOME/.cache/gitgraph-perf/r100k" \
//!   cargo test -p gg-core --release --test perf -- --ignored --nocapture
//! ```
//!
//! A repository whose variable is unset is skipped with a printed notice, as is everything
//! in a debug build. The two tests run one at a time (they would disturb each other's
//! timings). Each opens its repository once untimed (printed as "first", the file cache may
//! be cold), then three times timed; the targets apply to the median of the three.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gg_core::EventSink;
use gg_core::git::runner::{CommandLog, GitRunner};
use gg_core::repo::RepoManager;
use serde_json::Value;

const TIMEOUT: Duration = Duration::from_secs(60);

/// Held by each test for its whole run, so the tests never overlap.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Records when each `repo-changed` event arrived and its row count and loading flag.
#[derive(Default)]
struct TimedSink {
    events: Mutex<Vec<(Instant, u64, bool)>>,
}

impl EventSink for TimedSink {
    fn emit(&self, name: &str, payload: Value) {
        if name == "repo-changed" {
            let rows = payload["totalRows"].as_u64().unwrap_or(0);
            let loading = payload["loading"].as_bool().unwrap_or(true);
            self.events
                .lock()
                .unwrap()
                .push((Instant::now(), rows, loading));
        }
    }
}

impl TimedSink {
    fn first(&self, pred: impl Fn(u64, bool) -> bool) -> Option<Instant> {
        let events = self.events.lock().unwrap();
        events.iter().find(|e| pred(e.1, e.2)).map(|e| e.0)
    }
}

/// The repository named by `var`, or `None` (with a notice) when it is unset or this is
/// a debug build.
fn repo_path(var: &str) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        println!("perf: skipped, run with --release");
        return None;
    }
    match std::env::var_os(var) {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => {
            println!("perf: {var} is not set, skipped");
            None
        }
    }
}

struct Opened {
    m: Arc<RepoManager>,
    sink: Arc<TimedSink>,
    id: u32,
    generation: u64,
    total: u32,
    started: Instant,
    /// When `open` returned (refs, HEAD, worktrees and status read; the log is loading).
    opened: Instant,
    loaded: Instant,
}

/// Opens `path` in a fresh manager and waits until its log is fully loaded.
async fn open_loaded(path: &Path) -> Opened {
    let runner = GitRunner::detect(CommandLog::new())
        .await
        .expect("git is installed");
    let sink = Arc::new(TimedSink::default());
    let m = RepoManager::new(runner, sink.clone());
    let started = Instant::now();
    let snap = m.open(path).await.expect("open");
    let opened = Instant::now();
    tokio::time::timeout(TIMEOUT, m.wait_loaded(snap.repo_id))
        .await
        .expect("load finishes");
    // The final event can be emitted just after `wait_loaded` returns.
    let deadline = Instant::now() + TIMEOUT;
    let loaded = loop {
        if let Some(at) = sink.first(|_, loading| !loading) {
            break at;
        }
        assert!(Instant::now() < deadline, "no final repo-changed event");
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    let snap = m.snapshot(snap.repo_id).await.unwrap();
    Opened {
        m,
        sink,
        id: snap.repo_id,
        generation: snap.generation,
        total: snap.total_rows,
        started,
        opened,
        loaded,
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Times of one open, ms from just before `open`.
#[derive(Clone, Copy)]
struct Timing {
    /// `open` returned (refs, HEAD, worktrees and status read).
    opened: f64,
    /// First `repo-changed` with at least 500 rows.
    first_rows: f64,
    /// Final `repo-changed` (`loading: false`).
    loaded: f64,
}

impl Opened {
    fn timing(&self) -> Timing {
        let first = self
            .sink
            .first(|rows, _| rows >= 500)
            .unwrap_or(self.loaded);
        Timing {
            opened: ms(self.opened - self.started),
            first_rows: ms(first - self.started),
            loaded: ms(self.loaded - self.started),
        }
    }
}

/// Opens `path` once untimed and three times timed, each in a fresh manager. Prints every
/// timing; returns the median of the timed ones (per measure) and the last open.
async fn measure(name: &str, path: &Path) -> (Timing, Opened) {
    let first = open_loaded(path).await.timing();
    println!(
        "perf {name}: first open (not asserted): open {:.1} ms, first >=500 rows {:.1} ms, \
         fully loaded {:.1} ms",
        first.opened, first.first_rows, first.loaded
    );
    let mut timings = Vec::new();
    let mut last = None;
    for run in 1..=3 {
        let o = open_loaded(path).await;
        let t = o.timing();
        println!(
            "perf {name}: timed open {run}: open {:.1} ms, first >=500 rows {:.1} ms, \
             fully loaded {:.1} ms, {} rows",
            t.opened, t.first_rows, t.loaded, o.total
        );
        timings.push(t);
        last = Some(o);
    }
    let median = |f: fn(&Timing) -> f64| {
        let mut v: Vec<f64> = timings.iter().map(f).collect();
        v.sort_by(f64::total_cmp);
        v[1]
    };
    let m = Timing {
        opened: median(|t| t.opened),
        first_rows: median(|t| t.first_rows),
        loaded: median(|t| t.loaded),
    };
    println!(
        "perf {name}: median of 3: open {:.1} ms, first >=500 rows {:.1} ms, fully loaded {:.1} ms",
        m.opened, m.first_rows, m.loaded
    );
    (m, last.expect("three opens"))
}

/// Deterministic pseudo-random numbers (no extra crate for one test).
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % u64::from(n)) as u32
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "release-only perf test; needs GG_PERF_R20K / GG_PERF_R100K"]
async fn r20k_first_rows_and_full_load() {
    let _serial = SERIAL.lock().await;
    let Some(path) = repo_path("GG_PERF_R20K") else {
        return;
    };
    let (m, o) = measure("r20k", &path).await;
    assert!(o.total >= 20_000);
    assert!(
        m.first_rows < 300.0,
        "first 500 rows took {:.1} ms",
        m.first_rows
    );
    assert!(m.loaded < 1_500.0, "full load took {:.1} ms", m.loaded);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "release-only perf test; needs GG_PERF_R20K / GG_PERF_R100K"]
async fn r100k_full_load_rows_and_search() {
    let _serial = SERIAL.lock().await;
    let Some(path) = repo_path("GG_PERF_R100K") else {
        return;
    };
    let (m, o) = measure("r100k", &path).await;
    let loaded_ms = m.loaded;
    assert!(o.total >= 100_000);

    let mut rng = Lcg(0x5eed);
    let calls = 1_000;
    let mut bytes = 0usize;
    let mut serialize = Duration::ZERO;
    let t = Instant::now();
    for _ in 0..calls {
        let start = rng.below(o.total);
        let page = o.m.rows(o.id, o.generation, start, 200).await.unwrap();
        assert!(!page.rows.is_empty());
        // Not part of the target, but it is what the transports do with every page.
        let s = Instant::now();
        bytes += serde_json::to_vec(&page).unwrap().len();
        serialize += s.elapsed();
    }
    let rows_ms = (ms(t.elapsed()) - ms(serialize)) / f64::from(calls);
    println!(
        "perf r100k: rows(.., 200) average {rows_ms:.3} ms; serializing a page to JSON \
         averages {:.3} ms and {} bytes",
        ms(serialize) / f64::from(calls),
        bytes / calls as usize
    );

    let t = Instant::now();
    let hits = o.m.search(o.id, "feature").await.unwrap();
    let search_ms = ms(t.elapsed());
    println!(
        "perf r100k: search(\"feature\") {search_ms:.1} ms, {} hits",
        hits.len()
    );

    assert!(loaded_ms < 6_000.0, "full load took {loaded_ms:.1} ms");
    assert!(rows_ms < 2.0, "rows(.., 200) averaged {rows_ms:.3} ms");
    assert!(!hits.is_empty());
    assert!(search_ms < 150.0, "search took {search_ms:.1} ms");
}
