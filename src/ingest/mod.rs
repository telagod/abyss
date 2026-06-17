//! SCIP ingest scaffold (v0.5.15 prototype).
//!
//! The abyss heuristic resolver tops out around 75–99 % gated precision —
//! great for repos without a working language server, but always a
//! pessimistic floor when an LSP has compiler-grade ground truth in hand.
//! SCIP indexers (scip-go, scip-typescript, scip-python, rust-analyzer,
//! scip-clang) emit that ground truth as a binary protobuf. Ingesting it
//! lets us promote every covered ref to `confidence = 1.0` without
//! touching the eval contract for heuristic-only runs.
//!
//! ## Scope of this prototype
//!
//! v0.5.15 ships the **wiring** and a **mock JSON parser** so the
//! subcommand surface, the CLI flag set, and the dry-run summary stat
//! shape are pinned by tests:
//!
//! * `abyss ingest scip <file>` — accepts a `.scip` or `.json` file path
//! * `--dry-run --print-summary` — parses the input (JSON only for now)
//!   and prints document / occurrence / ref-candidate counts. No DB
//!   writes; safe to run against any input.
//! * Detects when `scip` is needed (i.e. the input is `.scip` binary
//!   protobuf) and emits an actionable error pointing at
//!   `eval/setup-indexers.sh`.
//!
//! What is **not** here yet (tracked for v0.5.16+):
//!
//! * Shelling out to `scip print --json <file>.scip` to convert binary
//!   protobuf into a JSON document set we can parse.
//! * Writing the SCIP occurrences back into `refs` at confidence 1.0 as
//!   the new L-1 tier (the design lives in pipeline.rs's tier table).
//! * Per-language symbol-string parsing for the `scheme local` /
//!   `package` / `descriptor` shape SCIP uses.
//!
//! The mock parser exists so we can pin two contracts under `cargo test`
//! today: the JSON shape we expect from `scip print --json` and the
//! counts our summary prints. Both stay byte-identical when the real
//! binary-protobuf path comes online.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Subset of the SCIP JSON shape (`scip print --json`) we actually need.
/// Keeping the model intentionally minimal: only the fields the prototype
/// reads. Extra fields are silently ignored thanks to serde's default
/// behaviour, so a real `scip print` blob will deserialise without
/// schema churn.
#[derive(Debug, Deserialize)]
pub struct ScipIndex {
    /// `metadata` — tool version, project root. Optional in our mock so
    /// hand-written test fixtures don't need to fake the whole header.
    #[serde(default)]
    pub metadata: Option<ScipMetadata>,
    /// One document per source file the SCIP indexer processed.
    #[serde(default)]
    pub documents: Vec<ScipDocument>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ScipMetadata {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub project_root: String,
    #[serde(default)]
    pub tool_info: Option<ScipToolInfo>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ScipToolInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct ScipDocument {
    /// Path relative to `metadata.project_root` (or absolute when the
    /// indexer couldn't anchor).
    #[serde(default)]
    pub relative_path: String,
    /// Language hint from the indexer (`go`, `typescript`, …). We pass
    /// it through unchanged; the call site decides what to do.
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub occurrences: Vec<ScipOccurrence>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ScipOccurrence {
    /// `[start_line, start_col, end_col]` or
    /// `[start_line, start_col, end_line, end_col]` — SCIP packs the
    /// range as a flat int array. The summary only needs the start
    /// line so we don't try to canonicalise the shape here.
    #[serde(default)]
    pub range: Vec<i64>,
    /// SCIP symbol string — opaque package/module/descriptor path. The
    /// resolver maps this back to `refs.target_symbol_id` once the real
    /// L-1 tier is wired.
    #[serde(default)]
    pub symbol: String,
    /// Bitfield. Bit 0 = definition; absence (== 0) means reference.
    /// The prototype only distinguishes "is a reference candidate" vs
    /// "is a definition", which is enough to drive the summary.
    #[serde(default)]
    pub symbol_roles: i64,
    /// Syntax kind hint — used by the call/type/inherit classifier
    /// when we finally wire L-1. Optional in the mock so hand-written
    /// fixtures stay tiny.
    #[serde(default)]
    pub syntax_kind: i64,
}

/// SCIP `SymbolRole` bit positions, mirroring the upstream protobuf
/// enum so we can decode `symbol_roles` without pulling in the full
/// SCIP schema crate.
pub mod symbol_role {
    /// The occurrence introduces a new symbol (vs referring to one).
    pub const DEFINITION: i64 = 0x1;
    /// Stable across SCIP versions; ImportRole is bit 1 (we ignore it
    /// here — imports aren't refs in abyss's model).
    pub const IMPORT: i64 = 0x2;
}

/// Aggregated counts printed by `--print-summary`. Public so tests can
/// pin the shape and a future MCP `ingest_scip` tool can return them
/// verbatim without re-counting.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct IngestSummary {
    pub documents: usize,
    pub occurrences: usize,
    /// Occurrences with `symbol_roles & DEFINITION == 0` — the set the
    /// real L-1 tier will promote to `confidence = 1.0`.
    pub ref_candidates: usize,
    /// Occurrences with `DEFINITION` set — useful for sanity-checking
    /// that the indexer actually emitted definitions and not just refs.
    pub definitions: usize,
    /// Distinct languages observed across documents.
    pub languages: Vec<String>,
}

impl IngestSummary {
    /// Build a summary from a parsed [`ScipIndex`] without touching the
    /// DB. Pure function — exposed so the CLI and a future MCP tool
    /// share one count implementation.
    pub fn from_index(index: &ScipIndex) -> Self {
        let mut summary = IngestSummary {
            documents: index.documents.len(),
            ..Default::default()
        };
        let mut langs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for doc in &index.documents {
            if !doc.language.is_empty() {
                langs.insert(doc.language.clone());
            }
            for occ in &doc.occurrences {
                summary.occurrences += 1;
                if occ.symbol_roles & symbol_role::DEFINITION != 0 {
                    summary.definitions += 1;
                } else {
                    summary.ref_candidates += 1;
                }
            }
        }
        summary.languages = langs.into_iter().collect();
        summary
    }
}

/// Read a SCIP input file. Today we only accept JSON (`scip print
/// --json` output, or a hand-rolled fixture). A `.scip` binary protobuf
/// path emits an actionable error pointing at the indexer setup script
/// so operators know exactly what's missing.
///
/// The mime-style detection is deliberately simple: extension wins. A
/// future patch will shell out to `scip print --json <input>` when
/// `input.extension() == "scip"` is detected and `scip` is on PATH.
pub fn parse_scip_input(path: &Path) -> Result<ScipIndex> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "json" => parse_scip_json_file(path),
        "scip" => anyhow::bail!(
            "abyss ingest scip: binary `.scip` ingest is not implemented yet (v0.5.15 \
             prototype). Convert with `scip print --json {} > /tmp/index.json` \
             then re-run `abyss ingest scip /tmp/index.json`. The `scip` CLI ships \
             via eval/setup-indexers.sh.",
            path.display()
        ),
        "" => anyhow::bail!(
            "abyss ingest scip: cannot infer format for {} — pass a .json (from `scip \
             print --json`) or a .scip file",
            path.display()
        ),
        other => anyhow::bail!(
            "abyss ingest scip: unsupported extension `.{other}` on {} — expected .json or .scip",
            path.display()
        ),
    }
}

/// Read + parse a JSON SCIP fixture from disk. Separated out so unit
/// tests can call `parse_scip_json_str` against an in-memory string
/// without touching the filesystem.
fn parse_scip_json_file(path: &Path) -> Result<ScipIndex> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read SCIP JSON {}", path.display()))?;
    parse_scip_json_str(&raw).with_context(|| format!("parse SCIP JSON {}", path.display()))
}

/// Parse a SCIP JSON document from an in-memory string. Used by both
/// the file loader above and the unit tests below.
pub fn parse_scip_json_str(raw: &str) -> Result<ScipIndex> {
    let index: ScipIndex = serde_json::from_str(raw).context("decode SCIP JSON")?;
    Ok(index)
}

#[cfg(test)]
mod tests {
    //! Pins the SCIP JSON shape and the summary-count contract.
    //!
    //! These tests intentionally exercise the smallest plausible
    //! fixture so future shape evolution surfaces here before it
    //! reaches the CLI / MCP layer.

    use super::*;

    const MOCK_JSON: &str = r#"{
        "metadata": {
            "version": "0.6.0",
            "project_root": "file:///tmp/proj",
            "tool_info": { "name": "scip-go", "version": "0.1.0" }
        },
        "documents": [
            {
                "relative_path": "src/a.go",
                "language": "go",
                "occurrences": [
                    {
                        "range": [10, 4, 14],
                        "symbol": "scip-go pkg/a/Foo.",
                        "symbol_roles": 1,
                        "syntax_kind": 1
                    },
                    {
                        "range": [20, 8, 18],
                        "symbol": "scip-go pkg/b/Bar#",
                        "symbol_roles": 0,
                        "syntax_kind": 2
                    },
                    {
                        "range": [30, 8, 18],
                        "symbol": "scip-go pkg/b/Bar#",
                        "symbol_roles": 0,
                        "syntax_kind": 2
                    }
                ]
            },
            {
                "relative_path": "src/b.go",
                "language": "go",
                "occurrences": [
                    {
                        "range": [5, 0, 10],
                        "symbol": "scip-go pkg/b/Bar.",
                        "symbol_roles": 1,
                        "syntax_kind": 1
                    }
                ]
            }
        ]
    }"#;

    #[test]
    fn json_parses_into_expected_shape() {
        let idx = parse_scip_json_str(MOCK_JSON).expect("parse mock");
        assert_eq!(idx.documents.len(), 2);
        assert_eq!(idx.documents[0].language, "go");
        assert_eq!(idx.documents[0].occurrences.len(), 3);
        assert_eq!(
            idx.metadata
                .as_ref()
                .and_then(|m| m.tool_info.as_ref())
                .map(|t| t.name.as_str()),
            Some("scip-go"),
            "tool_info should round-trip"
        );
    }

    #[test]
    fn summary_counts_match_mock_fixture() {
        let idx = parse_scip_json_str(MOCK_JSON).expect("parse mock");
        let summary = IngestSummary::from_index(&idx);
        assert_eq!(summary.documents, 2);
        assert_eq!(summary.occurrences, 4);
        // Two DEFINITION occurrences (one per file) + two ref candidates
        // on src/a.go pointing at the symbol defined in src/b.go.
        assert_eq!(summary.definitions, 2);
        assert_eq!(summary.ref_candidates, 2);
        assert_eq!(summary.languages, vec!["go".to_string()]);
    }

    #[test]
    fn empty_documents_yield_zero_counts() {
        let idx = parse_scip_json_str(r#"{"documents": []}"#).expect("parse");
        let summary = IngestSummary::from_index(&idx);
        assert_eq!(summary, IngestSummary::default());
    }

    #[test]
    fn binary_scip_extension_emits_actionable_error() {
        // No file write needed — parse_scip_input bails before touching
        // the filesystem when it sees a `.scip` extension.
        let err = parse_scip_input(Path::new("/tmp/nonexistent.scip"))
            .expect_err("binary .scip should be unsupported in v0.5.15");
        let msg = format!("{err}");
        assert!(
            msg.contains("scip print --json"),
            "error should mention the conversion command, got: {msg}",
        );
        assert!(
            msg.contains("setup-indexers.sh"),
            "error should point at the setup script, got: {msg}",
        );
    }

    #[test]
    fn unknown_extension_errors_cleanly() {
        let err = parse_scip_input(Path::new("/tmp/index.bin"))
            .expect_err("unknown extensions must error");
        let msg = format!("{err}");
        assert!(msg.contains("unsupported extension"), "got: {msg}");
    }
}
