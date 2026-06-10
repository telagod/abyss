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

    pub fn find_callers(&self, symbol_name: &str, limit: usize) -> Result<Vec<CallerInfo>> {
        let refs = self.repo.find_callers_of(symbol_name, None, limit)?;
        let mut callers = Vec::new();
        for r in refs {
            let is_test = self.repo.is_test_file(r.source_file_id).unwrap_or(false);
            callers.push(CallerInfo {
                file_path: r.source_file_path,
                symbol: r.source_symbol.unwrap_or_default(),
                line: r.source_line,
                depth: 0,
                confidence: r.confidence,
                is_test,
            });
        }
        Ok(callers)
    }

    pub fn impact_analysis(&self, symbol_name: &str, max_depth: u32) -> Result<ImpactResult> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, u32)> = VecDeque::new();
        let mut direct = Vec::new();
        let mut transitive = Vec::new();
        let mut tests = Vec::new();

        // Seed: direct callers
        let direct_refs = self.repo.find_callers_of(symbol_name, None, 200)?;
        for r in &direct_refs {
            let is_test = self.repo.is_test_file(r.source_file_id).unwrap_or(false);
            let caller = CallerInfo {
                file_path: r.source_file_path.clone(),
                symbol: r.source_symbol.clone().unwrap_or_default(),
                line: r.source_line,
                depth: 0,
                confidence: r.confidence,
                is_test,
            };
            if is_test {
                tests.push(caller);
            } else {
                direct.push(caller.clone());
                if let Some(ref sym) = r.source_symbol
                    && !sym.is_empty() && !visited.contains(sym) {
                        visited.insert(sym.clone());
                        queue.push_back((sym.clone(), 1));
                    }
            }
        }

        // BFS for transitive callers
        while let Some((sym, depth)) = queue.pop_front() {
            if depth > max_depth { continue; }

            let callers = self.repo.find_callers_of(&sym, None, 100)?;
            for r in callers {
                let is_test = self.repo.is_test_file(r.source_file_id).unwrap_or(false);
                let caller = CallerInfo {
                    file_path: r.source_file_path.clone(),
                    symbol: r.source_symbol.clone().unwrap_or_default(),
                    line: r.source_line,
                    depth,
                    confidence: r.confidence,
                    is_test,
                };

                if is_test {
                    tests.push(caller);
                } else {
                    transitive.push(caller);
                    if let Some(ref sym_name) = r.source_symbol
                        && !sym_name.is_empty() && !visited.contains(sym_name) {
                            visited.insert(sym_name.clone());
                            queue.push_back((sym_name.clone(), depth + 1));
                        }
                }
            }
        }

        // Find uncovered paths (callers with no test covering them)
        let tested_symbols: HashSet<String> = tests.iter().map(|t| t.symbol.clone()).collect();
        let uncovered: Vec<String> = direct.iter()
            .chain(transitive.iter())
            .filter(|c| !c.symbol.is_empty() && !tested_symbols.contains(&c.symbol))
            .map(|c| format!("{}:{}", c.file_path, c.symbol))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        // Risk scoring
        let risk_score = compute_risk(direct.len(), transitive.len(), uncovered.len());
        let risk_factors = compute_risk_factors(direct.len(), transitive.len(), uncovered.len());

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
    if direct > 10 { f.push(format!("high blast radius ({direct} direct callers)")); }
    if transitive > 20 { f.push(format!("deep dependency chain ({transitive} transitive)")); }
    if uncovered > 0 { f.push(format!("{uncovered} call paths without test coverage")); }
    f
}
