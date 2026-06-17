//! Architectural coordinates — L0 inference of layer / role / module.
//!
//! The pipeline runs after the call-graph resolver and combines four
//! independent signal sources into a single fact per file:
//!
//! - [`dictionary`] — path-segment regex hints (weight 0.4)
//! - [`naming`] — filename-suffix tiebreakers (weight 0.2)
//! - [`entry`] — language-aware entry-point detection (weight 1.0 override)
//! - [`graph`] — weighted PageRank, Tarjan SCC, single-pass Louvain
//!
//! Fusion happens in [`inference`], which produces [`ArchFact`] rows persisted
//! into the `arch_facts` and `arch_modules` tables. The pre-edit card reads
//! from these tables to fill in the `where` line (layer / module / role)
//! instead of the W1 placeholder.

pub mod dictionary;
pub mod entry;
pub mod graph;
pub mod inference;
pub mod naming;
pub mod override_config;

pub use dictionary::{LayerHint, classify_path};
pub use entry::is_entry_point;
pub use graph::{
    ArchGraph, CentralityResult, ModuleResult, SccResult, build_arch_graph, compute_centrality,
    compute_modules, compute_sccs,
};
pub use inference::{ArchFact, ArchModuleRow, collect_modules, infer_all};
pub use naming::classify_naming;
pub use override_config::{ArchOverride, LayerOverride, load_overrides};
