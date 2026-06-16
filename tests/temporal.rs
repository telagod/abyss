//! Temporal intelligence tests: git history mining, file metrics,
//! change coupling, and graceful degradation outside git.

mod common;
use common::*;

const X_V: [&str; 4] = [
    "package app\n\nfunc X() int { return 0 }\n",
    "package app\n\nfunc X() int { return 1 }\n",
    "package app\n\nfunc X() int { return 2 }\n",
    "package app\n\nfunc X() int { return 3 }\n",
];
const Y_V: [&str; 4] = [
    "package app\n\nfunc Y() int { return 0 }\n",
    "package app\n\nfunc Y() int { return 1 }\n",
    "package app\n\nfunc Y() int { return 2 }\n",
    "package app\n\nfunc Y() int { return 3 }\n",
];

#[test]
fn git_history_populates_metrics_and_coupling() {
    // x.go and y.go change together in 4 commits → coupled pair + hotspot data.
    let fx = index_git_fixture(&[
        &[
            ("app/x.go", X_V[0]),
            ("app/y.go", Y_V[0]),
            (
                "app/solo.go",
                "package app\n\nfunc Solo() int { return 0 }\n",
            ),
        ],
        &[("app/x.go", X_V[1]), ("app/y.go", Y_V[1])],
        &[("app/x.go", X_V[2]), ("app/y.go", Y_V[2])],
        &[("app/x.go", X_V[3]), ("app/y.go", Y_V[3])],
    ]);
    let conn = fx.repo.conn();

    let commits: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(commits, 4);

    let changes: i64 = conn
        .query_row(
            "SELECT change_count_30d FROM file_metrics fm JOIN files f ON fm.file_id = f.id WHERE f.path = 'app/x.go'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(changes, 4);

    let co_changes: i64 = conn
        .query_row(
            "SELECT co_changes FROM change_coupling
             WHERE (file_a = 'app/x.go' AND file_b = 'app/y.go')
                OR (file_a = 'app/y.go' AND file_b = 'app/x.go')
             LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("coupled pair must exist");
    assert_eq!(co_changes, 4);

    // solo.go only appears in one commit → below the coupling threshold (3).
    let solo_pairs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM change_coupling WHERE file_a = 'app/solo.go' OR file_b = 'app/solo.go'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(solo_pairs, 0);
}

#[test]
fn hotspots_rank_changed_complex_files() {
    let fx = index_git_fixture(&[
        &[
            ("app/hot.go", X_V[0]),
            (
                "app/cold.go",
                "package app\n\nfunc Cold() int { return 0 }\n",
            ),
        ],
        &[("app/hot.go", X_V[1])],
        &[("app/hot.go", X_V[2])],
        &[("app/hot.go", X_V[3])],
    ]);
    let hotspots = code_abyss::temporal::hotspot::top_hotspots(&fx.repo, 10).unwrap();
    assert!(!hotspots.is_empty());
    assert_eq!(hotspots[0].file_path, "app/hot.go", "{hotspots:?}");
    assert!(hotspots[0].change_count >= 4);
}

#[test]
fn git_history_skips_unindexed_paths() {
    // Lock files, notes, deleted/vendored paths flood git history but are never
    // indexed. They must not land in commit_files — every temporal consumer
    // filters by an indexed path, so they'd be pure dead weight + coupling-N².
    let fx = index_git_fixture(&[
        &[
            ("app/x.go", X_V[0]),
            ("notes.txt", "just a note\n"),
            ("deps.lock", "lockfile contents\n"),
        ],
        &[("app/x.go", X_V[1]), ("notes.txt", "updated note\n")],
    ]);
    let conn = fx.repo.conn();

    let indexed_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commit_files WHERE file_path = 'app/x.go'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        indexed_rows, 2,
        "indexed go file tracked across both commits"
    );

    let dead_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM commit_files WHERE file_path IN ('notes.txt', 'deps.lock')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dead_rows, 0, "unindexed paths must not enter commit_files");
}

#[test]
fn coupling_excludes_bulk_commits() {
    // A reformat / dep-bump commit touching >50 files couples every pair —
    // false signal and O(N²) blowup. Such commits must not generate coupling.
    let n = 60;
    let mk = |round: usize| -> Vec<(String, String)> {
        (0..n)
            .map(|i| {
                (
                    format!("app/f{i}.go"),
                    format!("package app\n\nfunc F{i}() int {{ return {round} }}\n"),
                )
            })
            .collect()
    };
    let v0 = mk(0);
    let v1 = mk(1);
    let v2 = mk(2);
    fn to_refs(v: &[(String, String)]) -> Vec<(&str, &str)> {
        v.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect()
    }
    // Three commits each touching 60 files together — would be 60*59/2 = 1770
    // coupling pairs each at co_changes=3 without the bulk guard.
    let c0 = to_refs(&v0);
    let c1 = to_refs(&v1);
    let c2 = to_refs(&v2);
    let fx = index_git_fixture(&[&c0, &c1, &c2]);
    let conn = fx.repo.conn();

    let pairs: i64 = conn
        .query_row("SELECT COUNT(*) FROM change_coupling", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        pairs, 0,
        "bulk commits (>50 files) must not generate coupling"
    );
}

#[test]
fn non_git_workspace_indexes_without_temporal_data() {
    // Must not fail — temporal data is best-effort.
    let fx = index_fixture(&[("a.go", "package app\n\nfunc A() int { return 1 }\n")]);
    assert_eq!(fx.repo.file_count().unwrap(), 1);
    let conn = fx.repo.conn();
    let commits: i64 = conn
        .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(commits, 0);
}

#[test]
fn evolution_traces_file_history() {
    let fx = index_git_fixture(&[&[("app/x.go", X_V[0])], &[("app/x.go", X_V[1])]]);
    let result = code_abyss::temporal::evolution::trace_evolution(
        &fx.config.workspace,
        &fx.repo,
        "app/x.go",
        None,
    )
    .unwrap();
    assert_eq!(result.total_changes, 2);
    assert_eq!(result.unique_authors, 1);
    assert_eq!(result.commits.len(), 2);
}
