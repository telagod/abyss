//! Hook latency regression gate.
//!
//! The agent fires `abyss hook pre-edit` on every save. If the hook ever
//! starts blocking the editor for >200ms p95 on a trivial fixture, this
//! test fails and we know before shipping.
//!
//! NOTE: we run the *in-process* hook path (Repository + build_file_context)
//! rather than `Command::new(env!("CARGO_BIN_EXE_abyss"))`. Reasoning:
//!   * fork+exec dominates wall time on CI runners and makes the threshold
//!     noisy; we already have hook_cli.rs covering the binary surface.
//!   * The real cost we want to police is hash-incremental refresh + the
//!     context query — that's what runs on every save once the binary is
//!     warm in page cache.
//!
//! Burst is serial, but represents the same blocking work; if a single call
//! crosses 200ms, the agent's keyboard does too.

mod common;

use std::time::Instant;

use code_abyss::context::build_file_context;
use code_abyss::indexer::IndexPipeline;
use common::*;

fn hub_fixture() -> Fixture {
    // A "hub" file: one target many callers fan into. Mirrors the worst-case
    // structure hostile.sh hunts for (high symbol density + heavy fan-in).
    let mut files: Vec<(String, String)> = Vec::new();
    files.push((
        "app/core.go".to_string(),
        "package app\n\nfunc Hub() int { return 1 }\nfunc Sidekick() int { return 2 }\n"
            .to_string(),
    ));
    for i in 0..20 {
        files.push((
            format!("app/caller{i}.go"),
            format!("package app\n\nfunc Caller{i}() int {{ return Hub() + Sidekick() }}\n"),
        ));
    }
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    index_fixture(&refs)
}

fn percentile(sorted: &[u128], pct: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * pct).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[test]
fn pre_edit_p95_under_200ms() {
    let fx = hub_fixture();
    let pipeline = IndexPipeline::new(fx.config.clone());
    let rel = "app/core.go";

    // Warm-up: prime page cache, prepared statements, parser pool.
    for _ in 0..5 {
        let _ = pipeline.run_structural(&fx.repo);
        let _ = build_file_context(&fx.repo, rel).unwrap();
    }

    let iterations = 100usize;
    let mut samples_us = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        // Match hook_pre_edit shape: refresh + context build.
        let _ = pipeline.run_structural(&fx.repo);
        let ctx = build_file_context(&fx.repo, rel).unwrap();
        let elapsed = start.elapsed();
        samples_us.push(elapsed.as_micros());
        assert!(ctx.is_some(), "context should resolve for indexed hub file");
    }

    samples_us.sort_unstable();
    let p50 = percentile(&samples_us, 0.50);
    let p95 = percentile(&samples_us, 0.95);
    let p99 = percentile(&samples_us, 0.99);
    let p50_ms = p50 as f64 / 1000.0;
    let p95_ms = p95 as f64 / 1000.0;
    let p99_ms = p99 as f64 / 1000.0;
    eprintln!(
        "hook latency (in-process, {iterations} iters): p50={p50_ms:.2}ms p95={p95_ms:.2}ms p99={p99_ms:.2}ms"
    );

    // 200ms is the agreed perceptible-blocking threshold. On every CI runner
    // we've measured this comes in well under 50ms; the headroom catches the
    // regression that would actually annoy a human typing.
    assert!(
        p95_ms < 200.0,
        "hook p95 regressed: {p95_ms:.2}ms > 200ms (p50={p50_ms:.2}ms, p99={p99_ms:.2}ms)"
    );
}
