# Resolver tiers and confidence

References resolve through a tiered SQL UPDATE cascade. Each level
only touches refs the previous levels left unresolved
(`confidence = 0.0`). The tier that resolves a ref decides its
confidence score, which is **stored on the row**.

| Tier | Strategy | Confidence |
|------|----------|-----------|
| L0   | Receiver-type → `symbols.scope` exact match (unique file) | 0.95 |
| L0c  | Receiver-type → type's import binding target file | 0.95 |
| L0d  | Receiver-type → type's unique defining file | 0.95 |
| L0e  | Python MRO walker (6-hop cap, C3-approx) on `inherits` refs | 0.95 |
| L0b  | Named-import binding (`import { x } from './m'`, `from m import x`, `use crate::m::x`) | 0.95 |
| L1   | Same-file (bare + self-like calls only) | 1.0 |
| L2   | Same package/directory, unique candidate | 0.95 |
| L3   | Import-qualifier match, unique candidate | 0.9 |
| L4   | Globally unique symbol (member-shaped for qualified calls) | 0.8 |
| L5   | Same package, multiple candidates (demoted) | 0.6 |
| L6   | Same-file fallback for qualified / ambiguous | 0.6 / 0.5 |

Ordering matters: L0 runs before L1 because type evidence beats
proximity. Each tier's confidence threshold was set by measuring
precision against the SCIP eval corpora.

## Receiver-type lite inference

Receiver types are inferred *lite* — method receivers, typed
parameters, `x := T{}` / `new T()` / `NewT()` / `x = Type()`
declarations, `this` / `self` / `Self`. No data-flow. No interface
resolution. Forward-ref string annotations work for Python; `self`
threading works through enclosing classes/impls.

When a receiver's type **is known**, only type-consistent evidence may
resolve the call — name-only proximity guesses demote instead. This
is why hono's runtime-assigned `app.get()` (no static definition)
never poses as a fact: L0 finds no owned `get`, the name-only tiers
skip refs with a known receiver, and the ref correctly demotes to
0.6 as a possibility.

Full confidence (1.0) is reserved for call shapes measured ≥98%
correct: bare and self-like calls. Qualified calls with an unknown
receiver never reach 1.0.

## Same-language-family filter

Cross-file resolution tiers L2/L3/L4/L4b/L5 require the candidate's
`files.lang_family` to equal the source file's. Families: `rust`,
`go`, `python`, `ts` (typescript + tsx + javascript), `c` (c + cpp),
`java`, `bash`. Single-language corpora are unaffected; polyglot
repos (Go + JS, Rust + JS) get noticeably cleaner demoted tiers.

## Codegen-aware indexing

Machine-generated files (`parser::is_generated` — `DO NOT EDIT` /
`@generated` markers) keep symbols and chunks but skip ref
extraction. Pass `--index-generated` to opt back in. Measured: a
1953-file Go backend's index dropped 102 MB → 75 MB, refs −34%,
coupling −90%.

## Confidence is a contract

Agent-facing APIs default to `--min-confidence 0.7`. Changes to
tier thresholds require eval validation against all SCIP corpora —
regressions are release-blockers.
