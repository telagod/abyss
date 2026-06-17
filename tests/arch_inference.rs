//! Integration tests for the L0 arch inference pipeline.
//!
//! These build a small synthetic Go workspace with the file shapes we want
//! to classify, run the full indexer (so the call-graph is real and Louvain
//! has edges to chew on), then verify that arch_facts emerges with the
//! expected layer / role assignments.

mod common;

use common::index_fixture;

#[test]
fn handler_file_classified_as_api() {
    let fx = index_fixture(&[
        (
            "internal/handler/user_handler.go",
            r#"package handler

func HandleUser() string { return "ok" }
"#,
        ),
        (
            "cmd/server/main.go",
            r#"package main

func main() {}
"#,
        ),
    ]);

    let conn = fx.repo.conn();
    let (layer, _conf): (String, f64) = conn
        .query_row(
            "SELECT af.layer, af.layer_conf
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             WHERE f.path LIKE '%user_handler.go'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("arch_facts row for handler should exist");
    assert_eq!(layer, "api", "handler file should classify as api");
}

#[test]
fn underscore_test_file_classified_as_test() {
    let fx = index_fixture(&[(
        "pkg/foo_test.go",
        r#"package foo

import "testing"

func TestFoo(t *testing.T) {}
"#,
    )]);

    let conn = fx.repo.conn();
    let layer: String = conn
        .query_row(
            "SELECT af.layer
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             WHERE f.path LIKE '%foo_test.go'",
            [],
            |r| r.get(0),
        )
        .expect("arch_facts row for test file should exist");
    assert_eq!(layer, "test", "_test.go suffix should classify as test");
}

#[test]
fn cmd_main_classified_as_entry_layer() {
    let fx = index_fixture(&[
        (
            "cmd/server/main.go",
            r#"package main

func main() {}
"#,
        ),
        (
            "internal/foo/foo.go",
            r#"package foo

func Foo() {}
"#,
        ),
    ]);

    let conn = fx.repo.conn();
    let layer: String = conn
        .query_row(
            "SELECT af.layer
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             WHERE f.path LIKE '%cmd/server/main.go'",
            [],
            |r| r.get(0),
        )
        .expect("arch_facts row for main.go should exist");
    // Either the dictionary's cmd/main.go rule or the entry-point detector
    // should win — both put this file in the "entry" bucket. We don't
    // assert role here because is_entry_point reads the file from disk,
    // which our tempdir-cwd setup may not let it find — but the layer
    // signal still resolves through the dictionary.
    assert_eq!(
        layer, "entry",
        "cmd/server/main.go should classify as entry"
    );
}

#[test]
fn arch_facts_populated_for_every_file() {
    let fx = index_fixture(&[
        ("a.go", "package a\nfunc A() {}\n"),
        ("b.go", "package b\nfunc B() {}\n"),
        ("c.go", "package c\nfunc C() {}\n"),
    ]);

    let conn = fx.repo.conn();
    let file_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
        .unwrap();
    let fact_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM arch_facts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        file_count, fact_count,
        "every indexed file should get an arch_facts row"
    );
}

#[test]
fn where_summary_returns_struct_for_indexed_file() {
    let fx = index_fixture(&[(
        "pkg/util/strings.go",
        r#"package util
func Trim(s string) string { return s }
"#,
    )]);

    let view = code_abyss::context::where_summary(&fx.repo, "pkg/util/strings.go")
        .expect("where_summary should not error")
        .expect("file should be indexed");
    assert_eq!(view["layer"].as_str(), Some("util"));
    assert!(view["module_label"].is_string());
    // signals JSON should round-trip
    assert!(view["signals"].is_object());
}

#[test]
fn where_summary_returns_none_for_unindexed_file() {
    let fx = index_fixture(&[("a.go", "package a\n")]);
    let result = code_abyss::context::where_summary(&fx.repo, "does_not_exist.go").unwrap();
    assert!(result.is_none());
}

// ── arch.toml user override ────────────────────────────────────────────────

/// Spin up a tempdir, drop an arch.toml + source files into it, then run the
/// full structural index so the pipeline picks up the override config from
/// `config.workspace` (not cwd).
fn index_fixture_with_override(files: &[(&str, &str)], arch_toml: &str) -> common::Fixture {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();

    // Write the override config FIRST so the pipeline sees it on its first
    // pass.
    let cfg_dir = workspace.join(".code-abyss");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("arch.toml"), arch_toml).unwrap();

    common::write_files(workspace, files);

    let ws_canon = std::fs::canonicalize(workspace).unwrap();
    let config = code_abyss::config::Config::new(&ws_canon);
    let repo =
        code_abyss::storage::Repository::open(&config.db_path, config.model.dimensions).unwrap();
    let pipeline = code_abyss::indexer::IndexPipeline::new(config.clone());
    pipeline.run_structural(&repo).unwrap();
    common::Fixture {
        _dir: dir,
        repo,
        config,
    }
}

#[test]
fn arch_toml_override_promotes_custom_segment_to_infra() {
    // Without the override "graph" is just a directory name with no
    // dictionary hit, so the layer would fall through to "unknown". With the
    // override it should land in "infra".
    let fx = index_fixture_with_override(
        &[(
            "src/graph/languages/go.rs",
            r#"pub fn extract() -> u32 { 42 }
"#,
        )],
        r#"
[layers]
graph = { layer = "infra", weight = 0.6 }
"#,
    );

    let conn = fx.repo.conn();
    let layer: String = conn
        .query_row(
            "SELECT af.layer
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             WHERE f.path LIKE '%graph/languages/go.rs'",
            [],
            |r| r.get(0),
        )
        .expect("arch_facts row should exist");
    assert_eq!(
        layer, "infra",
        "user override should flip graph/* into infra"
    );
}

#[test]
fn arch_toml_override_higher_weight_beats_default_dictionary() {
    // "service" is now in the default dictionary as `domain` at weight 0.4.
    // Override it to `infra` at weight 0.9 and verify the higher-weight rule
    // wins the fusion.
    let fx = index_fixture_with_override(
        &[(
            "src/service/billing.rs",
            r#"pub fn charge() -> u32 { 1 }
"#,
        )],
        r#"
[layers]
service = { layer = "infra", weight = 0.9 }
"#,
    );

    let conn = fx.repo.conn();
    let layer: String = conn
        .query_row(
            "SELECT af.layer
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             WHERE f.path LIKE '%service/billing.rs'",
            [],
            |r| r.get(0),
        )
        .expect("arch_facts row should exist");
    assert_eq!(
        layer, "infra",
        "higher-weight override should beat the default service→domain rule"
    );
}

#[test]
fn arch_toml_ignore_patterns_skip_file_entirely() {
    let fx = index_fixture_with_override(
        &[
            ("src/main.rs", "fn main() {}\n"),
            ("vendor/foo/bar.rs", "pub fn bar() {}\n"),
        ],
        r#"
[ignore]
patterns = ["^vendor/"]
"#,
    );

    let conn = fx.repo.conn();
    let vendor_count: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             WHERE f.path LIKE '%vendor/foo/bar.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        vendor_count, 0,
        "vendor/ files should be skipped by [ignore].patterns"
    );

    // Sanity: non-ignored file still gets a row.
    let main_count: i64 = conn
        .query_row(
            "SELECT COUNT(*)
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             WHERE f.path LIKE '%src/main.rs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(main_count, 1, "non-ignored files still get arch_facts");
}

#[test]
fn arch_toml_absent_does_not_change_default_behavior() {
    // Without arch.toml, a "graph" segment has no dictionary entry, so the
    // layer falls back to "unknown".
    let fx = index_fixture(&[(
        "src/graph/languages/go.rs",
        r#"pub fn extract() -> u32 { 42 }
"#,
    )]);

    let conn = fx.repo.conn();
    let layer: String = conn
        .query_row(
            "SELECT af.layer
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             WHERE f.path LIKE '%graph/languages/go.rs'",
            [],
            |r| r.get(0),
        )
        .expect("arch_facts row should exist");
    assert_eq!(
        layer, "unknown",
        "without arch.toml the default dictionary should not classify 'graph'"
    );
}
