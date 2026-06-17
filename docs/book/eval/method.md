# How resolution is measured

abyss's call-graph resolver is measured against SCIP (compiler-grade
indexing) ground truth across six corpora — gin (Go), hono (TS),
click (Python), ripgrep + abyss (Rust), cmark (C).

> Source-of-truth: the [eval/README.md](https://github.com/telagod/abyss/blob/main/eval/README.md) on the repo.

## Method

For every call reference abyss extracts, SCIP tells us where the called
symbol is actually defined. abyss's prediction is **correct iff it
resolved the call to the same file**. Join key:
`(file, line, symbol name)`. Only in-repo symbols count (abyss does
not resolve into dependencies).

- **precision** — when abyss commits to an answer, how often is it
  right
- **recall** — how much of the SCIP-known call graph abyss resolves
  correctly

Gated metrics use `--min-confidence 0.7` (the default agent gate).
"All" metrics include the demoted possibility tier.

## Reproducing the eval

```sh
bash eval/setup-indexers.sh   # idempotent: installs whatever's missing
bash eval/run.sh              # clones corpora, builds ground truth, compares
```

`setup-indexers.sh` installs:

| Indexer | Pinned to |
|---------|-----------|
| `scip` CLI | `v0.8.1` |
| `scip-go` | `v0.2.7` |
| `scip-typescript` | `0.4.0` |
| `scip-python` | `0.6.6` |
| `scip-clang` | `v0.3.2` |
| `rust-analyzer` | rustup toolchain |

## Reproducibility policy

Baselines published in `RESULTS.md` are reproducible **only** against
the pinned indexer versions. SCIP indexers move silently — between
v0.3.6 and v0.4.0 the click corpus drifted 98.7/94.6 → 97.9/93.0
gated P/R with zero abyss code change. `scip-python` v0.6.6 had
started emitting 16 extra truth pairs.

Bumping any pinned SCIP indexer requires re-running `eval/run.sh` and
updating the affected rows in `RESULTS.md` **in the same commit**.
The `--- indexer versions` line that `run.sh` prints to stderr is the
audit trail.

If a bump moves a baseline that looks like a regression, check the
indexer's release notes first. The
[2026-06-17 click microregression](./notes-click-microregression.md)
is the canonical example.

## Per-corpus results

Latest gated precision and recall, plus per-tier breakdowns, in
[Per-corpus results](./results.md).
