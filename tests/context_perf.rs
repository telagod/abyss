//! Performance smoke for build_file_context: synthesizes a hub file (50 funcs)
//! plus 30 caller files (each calling all 50 funcs), then asserts that median
//! latency over 10 runs stays under the agent-blocking threshold.
//!
//! The old N+1 loop ran ~1500 SQLite round-trips per call on this shape
//! (50 syms × 30 callers each, plus per-caller is_test_file). The flattened
//! single-JOIN path should land far below the wall clock budget.
mod common;

use std::time::Instant;

use code_abyss::context::build_file_context;
use common::*;

fn hub_fixture() -> Fixture {
    let n_syms = 50;
    let n_callers = 30;

    let mut hub = String::from("package hub\n");
    for i in 0..n_syms {
        hub.push_str(&format!("func Target{i}() int {{ return {i} }}\n"));
    }

    let mut files: Vec<(String, String)> = vec![("hub/core.go".to_string(), hub)];
    for c in 0..n_callers {
        let mut body = String::from("package hub\n\nfunc Caller_dummy() int {\n");
        body.push_str("    n := 0\n");
        for i in 0..n_syms {
            body.push_str(&format!("    n += Target{i}()\n"));
        }
        body.push_str("    return n\n}\n");
        files.push((format!("hub/caller_{c}.go"), body));
    }

    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    index_fixture(&refs)
}

#[test]
fn build_file_context_on_hub_is_under_budget() {
    let fx = hub_fixture();

    // Warm-up — first call pays for prepared-statement caching.
    let warm = build_file_context(&fx.repo, "hub/core.go").unwrap();
    assert!(warm.is_some(), "fixture file must be indexed");
    let warm_v = warm.unwrap();
    let sym_callers = warm_v["symbols_with_external_callers"]
        .as_array()
        .expect("symbols_with_external_callers must be an array");
    assert_eq!(
        sym_callers.len(),
        50,
        "all 50 hub funcs should report external callers"
    );
    // Every hub symbol should see 30 external callers (one per caller file).
    for entry in sym_callers {
        let ext = entry["external_callers"].as_array().unwrap();
        assert_eq!(
            ext.len(),
            30,
            "symbol {} expected 30 external callers, got {}",
            entry["symbol"],
            ext.len()
        );
    }

    let mut samples: Vec<u128> = Vec::with_capacity(10);
    for _ in 0..10 {
        let start = Instant::now();
        let _ = build_file_context(&fx.repo, "hub/core.go").unwrap();
        samples.push(start.elapsed().as_micros());
    }
    samples.sort_unstable();
    let median_us = samples[samples.len() / 2];
    let median_ms = median_us as f64 / 1000.0;

    eprintln!(
        "build_file_context hub-shape: median={median_ms:.2}ms over 10 runs (samples_us={samples:?})"
    );

    // Budget: 30ms in release, generous 100ms in debug (test binaries are debug
    // by default — SQLite + serde_json without optimizations are 3-5× slower).
    let budget_ms: f64 = if cfg!(debug_assertions) { 100.0 } else { 30.0 };
    assert!(
        median_ms < budget_ms,
        "build_file_context too slow: median {median_ms:.2}ms >= budget {budget_ms}ms"
    );
}
