# Per-corpus results

The numbers below mirror [`eval/RESULTS.md`](https://github.com/telagod/abyss/blob/main/eval/RESULTS.md)
in the repo — the source of truth that ships with every release.

## SCIP-graded corpora — 2026-06-17, abyss v0.5.1

These six corpora carry SCIP (compiler-grade) ground truth and gate
every release. Regressions here are release-blockers.

| Corpus | Lang | Truth pairs | Gated precision | Gated recall | All precision | All recall |
|--------|------|------------:|----------------:|-------------:|--------------:|-----------:|
| gin v1.10.0 | Go | 2,968 | **99.3%** | **82.6%** | 89.2% | 88.0% |
| hono v4.6.14 | TypeScript | 5,612 | **98.8%** | **63.8%** | (n/a) | (n/a) |
| click 8.1.8 | Python | 589 | **97.9%** | **93.0%** | 96.9% | 95.6% |
| ripgrep 14.1.1 | Rust | 4,283 | **98.5%** | **75.3%** | 86.9% | 86.8% |
| abyss @8099aeb | Rust (dogfood) | 450 | **100.0%** | **90.9%** | 98.4% | 98.4% |
| cmark 0.31.1 | C | 1,383 | **99.1%** | **74.8%** | 99.1% | 74.8% |

Gated = `--min-confidence 0.7`. abyss index time per corpus is
~150–900 ms; the SCIP indexers take 40 s–4 min.

> Captured against: scip v0.8.1, scip-go v0.2.7, scip-typescript
> 0.4.0, scip-python 0.6.6, scip-clang v0.3.2, rust-analyzer 1.95.0.

## Dogfood corpora — 2026-06-17, abyss v0.5.1

Five real third-party codebases run end-to-end against the live
agent surface (`where` / `context` / `callers` / `impact` / pre-edit
card). No SCIP ground truth — these score on signal density, noise,
and latency per probe, then a final 0–10. Full reports under
[`docs/dogfood/`](https://github.com/telagod/abyss/tree/main/docs/dogfood);
index at [`docs/DOGFOOD.md`](https://github.com/telagod/abyss/blob/main/docs/DOGFOOD.md).

| Project | Lang | Files | Cold index | Score | Notable surface |
|---------|------|------:|-----------:|------:|-----------------|
| Django 5.1.4 | Python | 3 292 | 6.91 s | **8 / 10** | L0e fired 9 450× (validates the FastAPI-falsified MRO walker on in-repo inheritance — see [Notes: Django MRO validation](notes-django-mro-validation.md)) |
| helix-editor @ `43bf7c2` | Rust workspace | 545 | 1.57 s | **7.5 / 10** | best-in-class blast radius card; rust-workspace labelling debt |
| vite v5.4.0 | TS/JS monorepo | 1 793 | 0.91 s | **7 / 10** | drove v0.5.0 `kind='type_ref'` default on `callers` |
| FastAPI 0.115.4 | Python | 2 164 | 1.07 s | **6.5 / 10** | falsified MRO ≥50-hits prediction (external bases — re-validated on Django) |
| hono v4.6.14 | TypeScript | 388 | 0.79 s | **8 / 10** | drove v0.5.1 `callers --limit` + built-in name-shadow filter |

## Per-tier breakdowns

### gin (Go) — scip-go

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like calls) | 656 | 0 | **100%** |
| 0.95 | receiver-type + same-package unique | 1,788 | 16 | 99.1% |
| 0.9 | import qualifier, unique | 1 | 0 | 100% |
| 0.8 | global unique | 6 | 3 | 66.7% |
| 0.6 / 0.5 | demoted / ambiguous | 138 | 299 | 31.6% |

### hono (TypeScript) — scip-typescript

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like) | 494 | 2 | **99.6%** |
| 0.95 | receiver-type + import binding + same-pkg unique | 2,910 | 30 | 99.0% |
| 0.9 | import qualifier, unique | 1 | 0 | 100% |
| 0.8 | global unique (member-shaped for qualified) | 175 | 10 | 94.6% |
| 0.6 / 0.5 | demoted / ambiguous | 704 | 1,186 | 37.2% |

### click (Python) — scip-python

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like) | 189 | 1 | **99.5%** |
| 0.95 | receiver-type + named-import + same-pkg unique | 333 | 6 | 98.2% |
| 0.8 | global unique | 18 | 0 | 100% |
| 0.6 / 0.5 | demoted / ambiguous | 9 | 7 | 56.3% |

### ripgrep (Rust, third-party) — rust-analyzer

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like) | 561 | 0 | **100%** |
| 0.95 | receiver-type + use-binding | 2,397 | 19 | 99.2% |
| 0.8 | global unique | 269 | 29 | 90.3% |
| 0.6 / 0.5 | demoted / ambiguous | 492 | 515 | 48.9% |

### abyss (Rust, dogfood) — rust-analyzer

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like) | 165 | 0 | **100%** |
| 0.95 | receiver-type + use-binding | 154 | 0 | **100%** |
| 0.8 | global unique | 90 | 0 | **100%** |
| 0.6 / 0.5 | demoted / ambiguous | 34 | 7 | 82.9% |

### cmark (C) — scip-clang

| Tier | Strategy | Correct | Wrong | Precision |
|------|----------|--------:|------:|----------:|
| 1.0 | same file (bare + self-like) | 373 | 9 | **97.6%** |
| 0.95 | receiver-type + include-binding + same-dir unique | 526 | 0 | **100%** |
| 0.8 | global unique | 135 | 0 | **100%** |

## Known weaknesses

1. **Dynamic / metaprogrammed methods** (TS): hono assigns router verbs
   (`app.get/post/use`) in a constructor loop — no static definition
   exists. These stay unresolved or demoted; they account for the
   bulk of hono's recall gap (51% un-gated). Surfaced as
   `possible_callers`.
2. **Interface dispatch** (Go): interface-typed receivers stay
   demoted at 0.6 by design. Resolving them needs interface-
   satisfaction analysis — compiler territory.
3. **JSX / TS noise**: hono's 0.5 tier is still large (1,161 joined
   pairs). JSX runtime calls and non-imported common names dilute
   global tiers.
4. **Java ground truth**: TODO. scip-java needs a build; planned.

## Eval-driven chronicle

The full round-by-round chronicle of how the eval drove the resolver
(rounds 1–10, from the initial 85.6% Go run through C/C++ caller
tracing) lives in [`eval/RESULTS.md`](https://github.com/telagod/abyss/blob/main/eval/RESULTS.md).
Worth reading if you want to understand *why* the tiers look the way
they do.
