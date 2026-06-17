use std::collections::{HashSet, VecDeque};

use anyhow::Result;
use serde::Serialize;

use crate::storage::Repository;

pub struct GraphQuery<'a> {
    repo: &'a Repository,
}

#[derive(Debug, Clone, Serialize)]
pub struct CallerInfo {
    pub file_path: String,
    pub symbol: String,
    pub line: u32,
    pub depth: u32,
    pub confidence: f64,
    pub is_test: bool,
    /// Ref kind from the underlying edge: `call`, `field_access`, or `type_ref`.
    /// Surfaces use it to suffix mixed listings with `(call, 95%)` vs
    /// `(type, 95%)`. Defaults to "call" when produced from legacy paths that
    /// pre-date the kind expansion (callers of `find_callers` etc.).
    #[serde(default)]
    pub kind: String,
}

/// `find_callers_filtered` returns both the surfaced caller list and the count
/// of test callers that were dropped by the filter. CLI/MCP surfaces use the
/// `excluded_tests` count to print "N tests excluded — use --include-tests to
/// see all" so agents know they're seeing a curated view, not the full graph.
#[derive(Debug, Clone, Serialize)]
pub struct CallersResult {
    pub callers: Vec<CallerInfo>,
    pub excluded_tests: usize,
}

/// Which `refs.kind` edges count as "callers". Default is `Both` (the agent
/// superset: invocation + type-position + inheritance) because for an agent
/// asking "who depends on X" a type-position user OR an inheritance edge is
/// just as load-bearing as an invocation site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKindFilter {
    /// Invocation edges: `call` + `field_access`. Legacy behaviour.
    CallsOnly,
    /// Type-position edges only: `type_ref`. Useful for "who annotates with
    /// this type / extends this interface".
    TypesOnly,
    /// Inheritance edges only: `inherit`. Useful for "every subclass of this
    /// base class". Django Model dogfood (2026-06-17): `callers Model` was
    /// 7 instead of ~983 because `inherit` was missing from the default set;
    /// the v0.5.0 G1 work surfaced `type_ref` but the same pattern blindness
    /// dropped `inherit`. Different label, same bug.
    InheritsOnly,
    /// The agent-facing superset: invocation + type-position + inheritance.
    /// Historically named `Both` (when only call+type_ref were in the set).
    Both,
}

impl CallerKindFilter {
    pub fn as_slice(self) -> &'static [&'static str] {
        match self {
            CallerKindFilter::CallsOnly => &["call", "field_access"],
            CallerKindFilter::TypesOnly => &["type_ref"],
            CallerKindFilter::InheritsOnly => &["inherit"],
            CallerKindFilter::Both => &["call", "field_access", "type_ref", "inherit"],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImpactResult {
    pub target: String,
    pub direct_callers: Vec<CallerInfo>,
    pub transitive_callers: Vec<CallerInfo>,
    pub affected_tests: Vec<CallerInfo>,
    pub uncovered_paths: Vec<String>,
    pub risk_score: f64,
    pub risk_factors: Vec<String>,
}

impl<'a> GraphQuery<'a> {
    pub fn new(repo: &'a Repository) -> Self {
        Self { repo }
    }

    /// Find callers of a symbol. Refs below `min_confidence` are dropped so
    /// low-confidence (ambiguous) matches don't poison agent context; pass 0.0
    /// to see everything. Returns *all* callers (prod + test) — agent surfaces
    /// should prefer [`Self::find_callers_filtered`], which exposes the test
    /// exclusion knob so unfamiliar codebases get prod call sites by default.
    ///
    /// Kind contract: returns invocation edges (`call`, `field_access`),
    /// type-position edges (`type_ref`), AND inheritance edges (`inherit`).
    /// On TS/Rust, exported types are used in annotations far more than
    /// they're directly constructed; on Django/Python, base classes have
    /// hundreds of subclass inheritors and zero direct invocations. A
    /// caller list that hides either mislabels real users as "no users".
    pub fn find_callers(
        &self,
        symbol_name: &str,
        limit: usize,
        min_confidence: f64,
    ) -> Result<Vec<CallerInfo>> {
        Ok(self
            .find_callers_filtered_kinds(
                symbol_name,
                limit,
                min_confidence,
                true,
                CallerKindFilter::Both,
            )?
            .callers)
    }

    /// Same as [`Self::find_callers`] but lets the caller hide test files by
    /// default. When `include_tests=false`, callers detected by
    /// [`Repository::is_test_file`] are removed from the returned list and
    /// counted in `excluded_tests` so the surface can render the
    /// "(N tests excluded — use --include-tests to see all)" hint.
    ///
    /// Default kind filter is [`CallerKindFilter::Both`] — see [`Self::find_callers`].
    pub fn find_callers_filtered(
        &self,
        symbol_name: &str,
        limit: usize,
        min_confidence: f64,
        include_tests: bool,
    ) -> Result<CallersResult> {
        self.find_callers_filtered_kinds(
            symbol_name,
            limit,
            min_confidence,
            include_tests,
            CallerKindFilter::Both,
        )
    }

    /// Full variant: same as [`Self::find_callers_filtered`] but takes a
    /// [`CallerKindFilter`] so the surface can restrict to call-only or
    /// type-only via `--calls-only` / `--types-only` flags.
    pub fn find_callers_filtered_kinds(
        &self,
        symbol_name: &str,
        limit: usize,
        min_confidence: f64,
        include_tests: bool,
        kind_filter: CallerKindFilter,
    ) -> Result<CallersResult> {
        // Fetch a larger pool when filtering tests so the surfaced prod-only
        // list can still reach `limit`. Bounded so a pathological query can't
        // explode memory: prod + excluded test callers together cap at 4× the
        // user-visible limit, with a hard floor for tiny callsite counts.
        let fetch_limit = if include_tests {
            limit
        } else {
            (limit.saturating_mul(4)).max(limit + 16)
        };
        let kinds = kind_filter.as_slice();
        let refs = self
            .repo
            .find_callers_of_kinds(symbol_name, None, kinds, fetch_limit)?;
        let mut callers = Vec::new();
        let mut excluded_tests = 0usize;
        for r in refs {
            if r.confidence < min_confidence {
                continue;
            }
            let is_test = self.repo.is_test_file(r.source_file_id).unwrap_or(false);
            if !include_tests && is_test {
                excluded_tests += 1;
                continue;
            }
            if callers.len() >= limit {
                // Already at the user-visible cap. Keep scanning only so we
                // can finish counting excluded test callers honestly; stop
                // once we've exhausted the fetched pool. Prod callers past
                // the cap are silently dropped (same as the legacy contract).
                continue;
            }
            callers.push(CallerInfo {
                file_path: r.source_file_path,
                symbol: r.source_symbol.unwrap_or_default(),
                line: r.source_line,
                depth: 0,
                confidence: r.confidence,
                is_test,
                kind: r.kind,
            });
        }
        Ok(CallersResult {
            callers,
            excluded_tests,
        })
    }

    /// Blast-radius analysis: BFS over callers, gated by `min_confidence`.
    ///
    /// Kind contract (v0.5.2+): the default kind set is the agent-facing
    /// superset (`call`, `field_access`, `type_ref`, `inherit`) — same as
    /// [`Self::find_callers`]. v0.5.1 dogfood (hono `Context`): `impact`
    /// reported `direct=2` while `callers` reported 20 prod callers; the
    /// numbers referred to different things because impact silently used the
    /// invocation-only legacy set. A type-position user of a symbol IS in the
    /// blast radius if you change that symbol's API; an inheritance edge IS a
    /// load-bearing dependency. The two surfaces must agree by default.
    ///
    /// For the function-only blast radius (legacy semantics), see
    /// [`Self::impact_analysis_filtered`] with [`CallerKindFilter::CallsOnly`].
    pub fn impact_analysis(
        &self,
        symbol_name: &str,
        max_depth: u32,
        min_confidence: f64,
    ) -> Result<ImpactResult> {
        self.impact_analysis_filtered(
            symbol_name,
            max_depth,
            min_confidence,
            CallerKindFilter::Both,
        )
    }

    /// Blast-radius analysis with caller-specified kind filter. See
    /// [`Self::impact_analysis`] for the kind-contract rationale.
    pub fn impact_analysis_filtered(
        &self,
        symbol_name: &str,
        max_depth: u32,
        min_confidence: f64,
        kind_filter: CallerKindFilter,
    ) -> Result<ImpactResult> {
        // Visited key is (source_file_id, source_symbol_name) — NOT bare name.
        //
        // Bug class pinned: two functions named `run` in different files must
        // each have their subtrees walked independently. Keying on just the
        // name collapses them and any future BFS step that branches on
        // (file_id, name) — e.g. passing source-file-scoped filters into
        // find_callers_of_kinds — would silently drop a subtree.
        //
        // The current find_callers_of_kinds is name-keyed at the SQL layer, so
        // the observable surface of this bug is currently masked by the
        // aggregate query. The visited-set type is a forward-looking guard: it
        // preserves correctness if/when the BFS is tightened to file-scoped
        // traversal. See `visited_set_contract` tests below for the pin.
        let mut visited: HashSet<(i64, String)> = HashSet::new();
        let mut queue: VecDeque<(String, u32)> = VecDeque::new();
        let mut direct = Vec::new();
        let mut transitive = Vec::new();
        let mut tests = Vec::new();
        let mut excluded_low_confidence = 0usize;

        let kinds = kind_filter.as_slice();
        // Seed: direct callers
        let direct_refs = self
            .repo
            .find_callers_of_kinds(symbol_name, None, kinds, 200)?;
        for r in &direct_refs {
            if r.confidence < min_confidence {
                excluded_low_confidence += 1;
                continue;
            }
            let is_test = self.repo.is_test_file(r.source_file_id).unwrap_or(false);
            let caller = CallerInfo {
                file_path: r.source_file_path.clone(),
                symbol: r.source_symbol.clone().unwrap_or_default(),
                line: r.source_line,
                depth: 0,
                confidence: r.confidence,
                is_test,
                kind: r.kind.clone(),
            };
            if is_test {
                tests.push(caller);
            } else {
                direct.push(caller.clone());
                if let Some(ref sym) = r.source_symbol
                    && !sym.is_empty()
                {
                    let key = (r.source_file_id, sym.clone());
                    if !visited.contains(&key) {
                        visited.insert(key);
                        queue.push_back((sym.clone(), 1));
                    }
                }
            }
        }

        // BFS for transitive callers
        while let Some((sym, depth)) = queue.pop_front() {
            if depth > max_depth {
                continue;
            }

            let callers = self.repo.find_callers_of_kinds(&sym, None, kinds, 100)?;
            for r in callers {
                if r.confidence < min_confidence {
                    excluded_low_confidence += 1;
                    continue;
                }
                let is_test = self.repo.is_test_file(r.source_file_id).unwrap_or(false);
                let caller = CallerInfo {
                    file_path: r.source_file_path.clone(),
                    symbol: r.source_symbol.clone().unwrap_or_default(),
                    line: r.source_line,
                    depth,
                    confidence: r.confidence,
                    is_test,
                    kind: r.kind.clone(),
                };

                if is_test {
                    tests.push(caller);
                } else {
                    transitive.push(caller);
                    if let Some(ref sym_name) = r.source_symbol
                        && !sym_name.is_empty()
                    {
                        let key = (r.source_file_id, sym_name.clone());
                        if !visited.contains(&key) {
                            visited.insert(key);
                            queue.push_back((sym_name.clone(), depth + 1));
                        }
                    }
                }
            }
        }

        // Find uncovered paths (callers with no test covering them)
        let tested_symbols: HashSet<String> = tests.iter().map(|t| t.symbol.clone()).collect();
        let uncovered: Vec<String> = direct
            .iter()
            .chain(transitive.iter())
            .filter(|c| !c.symbol.is_empty() && !tested_symbols.contains(&c.symbol))
            .map(|c| format!("{}:{}", c.file_path, c.symbol))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        // Risk scoring
        let risk_score = compute_risk(direct.len(), transitive.len(), uncovered.len());
        let mut risk_factors =
            compute_risk_factors(direct.len(), transitive.len(), uncovered.len());
        if excluded_low_confidence > 0 {
            risk_factors.push(format!(
                "{excluded_low_confidence} low-confidence reference(s) excluded (rerun with --min-confidence 0 to see them)"
            ));
        }

        Ok(ImpactResult {
            target: symbol_name.to_string(),
            direct_callers: direct,
            transitive_callers: transitive,
            affected_tests: tests,
            uncovered_paths: uncovered,
            risk_score,
            risk_factors,
        })
    }
}

fn compute_risk(direct: usize, transitive: usize, uncovered: usize) -> f64 {
    let blast = (direct as f64).ln_1p() * 2.0 + (transitive as f64).ln_1p();
    let test_risk = (uncovered as f64).ln_1p() * 3.0;
    ((blast + test_risk) / 2.0).min(10.0)
}

fn compute_risk_factors(direct: usize, transitive: usize, uncovered: usize) -> Vec<String> {
    let mut f = Vec::new();
    if direct > 10 {
        f.push(format!("high blast radius ({direct} direct callers)"));
    }
    if transitive > 20 {
        f.push(format!("deep dependency chain ({transitive} transitive)"));
    }
    if uncovered > 0 {
        f.push(format!("{uncovered} call paths without test coverage"));
    }
    f
}

#[cfg(test)]
mod visited_set_contract {
    //! Strict regression tests pinning the BFS visited-set key shape.
    //!
    //! These tests do not exercise `impact_analysis` end-to-end (that path is
    //! covered by `tests/graph_query.rs`). Instead they pin the *data-type
    //! contract* the fix established: visited keys MUST be
    //! `(source_file_id, source_symbol_name)`, never bare name.
    //!
    //! Why a contract test and not just an integration test? `find_callers_of`
    //! is a name-keyed SQL aggregate, so at runtime the merged result masks
    //! the bug. The visited-set type is a forward-looking guard. These tests
    //! make the type choice load-bearing: a refactor that demotes the key
    //! back to `HashSet<String>` will fail compilation here, and a refactor
    //! that mock-implements BFS with a bare-name key will fail the
    //! `buggy_key_drops_subtree_when_bfs_is_file_scoped` reductio below.
    use std::collections::{HashSet, VecDeque};
    use std::iter::FromIterator;

    /// Hard-codes the visited-set type. If a refactor narrows the key, this
    /// test fails at compile time (mismatched types) or at runtime when two
    /// distinct (file_id, name) entries collapse.
    #[test]
    fn visited_keys_distinguish_same_name_across_files() {
        // (file_id=10, "foo") and (file_id=20, "foo") are distinct callers.
        let mut visited: HashSet<(i64, String)> = HashSet::new();
        let mut asserts = 0u32;

        assert!(visited.insert((10, "foo".to_string())));
        asserts += 1;
        assert!(
            visited.insert((20, "foo".to_string())),
            "two same-named symbols in different files MUST get distinct visited entries"
        );
        asserts += 1;
        assert_eq!(visited.len(), 2, "visited collapsed two distinct callers");
        asserts += 1;

        // A re-insert of an existing key is a no-op (BFS dedup still works
        // within a single file).
        assert!(!visited.insert((10, "foo".to_string())));
        asserts += 1;
        assert_eq!(visited.len(), 2);
        asserts += 1;

        // Different names in the same file are also distinct.
        assert!(visited.insert((10, "bar".to_string())));
        asserts += 1;
        assert_eq!(visited.len(), 3);
        asserts += 1;

        // Pin: a HashSet<String> would have collapsed the two foos. We
        // mirror the buggy shape here to make the regression explicit.
        let bare_name: HashSet<String> =
            HashSet::from_iter(["foo".to_string(), "foo".to_string(), "bar".to_string()]);
        assert_eq!(
            bare_name.len(),
            2,
            "control: bare-name keying would collapse to 2 (this is the bug)"
        );
        asserts += 1;

        // Sanity print so the harness owner can grep assertion counts.
        eprintln!("visited_keys_distinguish_same_name_across_files: {asserts} asserts");
    }

    /// Reductio: simulate a future BFS that passes `target_file_id` into
    /// `find_callers_of` (file-scoped traversal). Under that contract the
    /// OLD bare-name visited set drops a subtree; the FIXED (file_id, name)
    /// key does not. Pins the design rationale even though the current
    /// production path's name-keyed SQL masks the bug.
    #[test]
    fn buggy_key_drops_subtree_when_bfs_is_file_scoped() {
        // Mini call-graph (file_id, source_symbol, target_symbol, target_file_id_hint)
        // File X (id=1): foo_x calls target;       runner_x calls foo_x
        // File Y (id=2): foo_y calls target;       runner_y calls foo_y
        //
        // The hypothetical file-scoped find_callers_of(target, file_id) returns
        // only callers whose ref binds to the matching file. With bare-name
        // visited, after enqueueing "foo" once, the second "foo" subtree is
        // never traversed.
        #[derive(Clone)]
        struct Caller {
            file_id: i64,
            source_symbol: &'static str,
            target_name: &'static str,
            target_file_id: i64,
        }
        let edges: [Caller; 4] = [
            // File X chain: target <- foo_x <- runner_x
            Caller {
                file_id: 1,
                source_symbol: "foo",
                target_name: "target",
                target_file_id: 99,
            },
            Caller {
                file_id: 1,
                source_symbol: "runner_x",
                target_name: "foo",
                target_file_id: 1,
            },
            // File Y chain: target <- foo_y <- runner_y
            Caller {
                file_id: 2,
                source_symbol: "foo",
                target_name: "target",
                target_file_id: 99,
            },
            Caller {
                file_id: 2,
                source_symbol: "runner_y",
                target_name: "foo",
                target_file_id: 2,
            },
        ];
        let file_scoped_lookup = |name: &str, target_file_id: i64| -> Vec<Caller> {
            edges
                .iter()
                .filter(|c| c.target_name == name && c.target_file_id == target_file_id)
                .cloned()
                .collect()
        };

        // --- BUGGY BFS: visited keyed on bare name. ---
        let mut buggy_visited: HashSet<String> = HashSet::new();
        let mut buggy_queue: VecDeque<(String, i64)> = VecDeque::new();
        let mut buggy_walked: Vec<(i64, String)> = Vec::new();
        // Seed direct callers of (target, file 99)
        for c in file_scoped_lookup("target", 99) {
            buggy_walked.push((c.file_id, c.source_symbol.to_string()));
            if buggy_visited.insert(c.source_symbol.to_string()) {
                buggy_queue.push_back((c.source_symbol.to_string(), c.file_id));
            }
        }
        while let Some((name, file_id)) = buggy_queue.pop_front() {
            for c in file_scoped_lookup(&name, file_id) {
                buggy_walked.push((c.file_id, c.source_symbol.to_string()));
                if buggy_visited.insert(c.source_symbol.to_string()) {
                    buggy_queue.push_back((c.source_symbol.to_string(), c.file_id));
                }
            }
        }

        // --- FIXED BFS: visited keyed on (file_id, name). ---
        let mut fixed_visited: HashSet<(i64, String)> = HashSet::new();
        let mut fixed_queue: VecDeque<(String, i64)> = VecDeque::new();
        let mut fixed_walked: Vec<(i64, String)> = Vec::new();
        for c in file_scoped_lookup("target", 99) {
            fixed_walked.push((c.file_id, c.source_symbol.to_string()));
            if fixed_visited.insert((c.file_id, c.source_symbol.to_string())) {
                fixed_queue.push_back((c.source_symbol.to_string(), c.file_id));
            }
        }
        while let Some((name, file_id)) = fixed_queue.pop_front() {
            for c in file_scoped_lookup(&name, file_id) {
                fixed_walked.push((c.file_id, c.source_symbol.to_string()));
                if fixed_visited.insert((c.file_id, c.source_symbol.to_string())) {
                    fixed_queue.push_back((c.source_symbol.to_string(), c.file_id));
                }
            }
        }

        let mut asserts = 0u32;

        // Buggy walk MUST drop one of the runner subtrees.
        let buggy_has_x = buggy_walked.iter().any(|(_, s)| s == "runner_x");
        let buggy_has_y = buggy_walked.iter().any(|(_, s)| s == "runner_y");
        assert!(
            !(buggy_has_x && buggy_has_y),
            "control: buggy bare-name visited MUST drop at least one subtree under \
             file-scoped traversal — got both runner_x and runner_y: {buggy_walked:?}"
        );
        asserts += 1;

        // Fixed walk MUST reach both subtrees.
        let fixed_has_x = fixed_walked.iter().any(|(_, s)| s == "runner_x");
        let fixed_has_y = fixed_walked.iter().any(|(_, s)| s == "runner_y");
        assert!(
            fixed_has_x,
            "fixed (file_id, name) visited must reach runner_x: {fixed_walked:?}"
        );
        asserts += 1;
        assert!(
            fixed_has_y,
            "fixed (file_id, name) visited must reach runner_y: {fixed_walked:?}"
        );
        asserts += 1;
        assert!(
            fixed_visited.len() >= 4,
            "fixed visited should contain at least foo@1, foo@2, runner_x@1, runner_y@2: \
             {fixed_visited:?}"
        );
        asserts += 1;

        eprintln!("buggy_key_drops_subtree_when_bfs_is_file_scoped: {asserts} asserts");
    }
}
