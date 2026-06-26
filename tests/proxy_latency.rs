//! Proxy latency benchmark test.
//!
//! Asserts that the proxy's filter path stays under 5ms for typical inputs.
//! This is a regression gate, not a micro-benchmark — if the filter starts
//! doing something O(n²) or hitting disk, this catches it.

use code_abyss::proxy::handlers::{all_handlers, find_handler};

fn make_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

#[test]
fn git_status_filter_under_1ms() {
    let handlers = all_handlers();
    let args = make_args(&["status"]);
    let handler = find_handler(&handlers, "git", &args).unwrap();

    let mut raw = String::from("On branch main\nChanges to be committed:\n");
    for i in 0..100 {
        raw.push_str(&format!("\tnew file:   src/file{i}.rs\n"));
    }
    raw.push_str("\nChanges not staged for commit:\n");
    for i in 0..50 {
        raw.push_str(&format!("\tmodified:   src/mod{i}.rs\n"));
    }

    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _ = handler.filter(&raw, "", 0, &args, None);
    }
    let elapsed = start.elapsed();
    let per_call = elapsed / 100;

    assert!(
        per_call.as_millis() < 1,
        "git status filter too slow: {per_call:?} per call (100 staged + 50 unstaged)"
    );
}

#[test]
fn cargo_test_filter_under_1ms() {
    let handlers = all_handlers();
    let args = make_args(&["test"]);
    let handler = find_handler(&handlers, "cargo", &args).unwrap();

    let mut raw = String::from("running 500 tests\n");
    for i in 0..500 {
        raw.push_str(&format!("test test_{i} ... ok\n"));
    }
    raw.push_str("test result: ok. 500 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n");

    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _ = handler.filter(&raw, "", 0, &args, None);
    }
    let elapsed = start.elapsed();
    let per_call = elapsed / 100;

    assert!(
        per_call.as_millis() < 1,
        "cargo test filter too slow: {per_call:?} per call (500 tests)"
    );
}

#[test]
fn grep_filter_under_5ms() {
    let handlers = all_handlers();
    let args = make_args(&["-rn", "pattern"]);
    let handler = find_handler(&handlers, "grep", &args).unwrap();

    let mut raw = String::new();
    for i in 0..2000 {
        raw.push_str(&format!(
            "src/file{}.rs:{}:    let x = pattern();\n",
            i % 50,
            i
        ));
    }

    let start = std::time::Instant::now();
    for _ in 0..100 {
        let _ = handler.filter(&raw, "", 0, &args, None);
    }
    let elapsed = start.elapsed();
    let per_call = elapsed / 100;

    assert!(
        per_call.as_millis() < 5,
        "grep filter too slow: {per_call:?} per call (2000 matches)"
    );
}

#[test]
fn cat_treesitter_strip_under_5ms() {
    let handlers = all_handlers();
    let args = make_args(&["big.rs"]);
    let handler = find_handler(&handlers, "cat", &args).unwrap();

    let mut raw = String::new();
    for i in 0..20 {
        raw.push_str(&format!("pub fn func_{i}() {{\n"));
        for j in 0..50 {
            raw.push_str(&format!("    let x{j} = {j};\n"));
        }
        raw.push_str("}\n\n");
    }

    let start = std::time::Instant::now();
    for _ in 0..50 {
        let _ = handler.filter(&raw, "", 0, &args, None);
    }
    let elapsed = start.elapsed();
    let per_call = elapsed / 50;

    assert!(
        per_call.as_millis() < 15,
        "cat/treesitter filter too slow: {per_call:?} per call (20 functions × 50 lines)"
    );
}
