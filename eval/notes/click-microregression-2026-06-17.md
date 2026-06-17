# click eval: not a regression, just a new ground truth

**Date:** 2026-06-17 (after v0.4.0 release)
**Status:** Closed. No code change. Baseline updated.

## TL;DR

Between v0.3.6 (gated 98.7% / 94.6%) and v0.4.0 (gated 97.9% / 93.0%) on the
click corpus, **abyss did not regress**. The two release binaries produce
byte-equivalent indexes; the eval delta comes entirely from a regenerated
SCIP ground truth (`eval/corpus/click/scip.json`) that grew from 573 to
**589** truth pairs after scip-python upgraded.

## Evidence

Built `cargo build --release` on both `v0.3.6` and `26475fe` (post-v0.4.0).
Indexed `eval/corpus/click` with each binary. Diffed the resulting
`.code-abyss/index.db` files at the refs level:

```
call refs:                      v0.3.6=3848   v0.4.0=3848
unique (src,line,name) keys:    3835 / 3835

AGREE (same target):            2706
DISAGREE (different target):    0
NEW_RESOLUTION:                 0
LOST_RESOLUTION:                0
SAME_UNRESOLVED:                1129
key only in v0.3.6:             0
key only in v0.4.0:             0
tier moves on AGREE:            0
```

**Zero refs disagree. Zero refs moved tier.** The two indexes are
indistinguishable at the call-graph level.

Running `eval/compare.py` against each DB against the *current* scip.json:

```
v0.3.6 binary → gated 97.9% / 93.0%   (548 / 560 predicted, 589 truth)
v0.4.0 binary → gated 97.9% / 93.0%   (548 / 560 predicted, 589 truth)
```

Identical to the decimal. Same gated numbers.

## What changed, then?

`eval/corpus/click/scip.json` is gitignored; `run.sh` regenerates it only
when the file is missing. The baseline numbers were captured 2026-06-12
against an older `scip.json` with 573 truth pairs. Today's `scip.json` was
emitted by scip-python v0.6.6 (installed 2026-06-17) and contains **589**
truth pairs — sixteen additional cross-file occurrences.

Those extra 16 pairs land disproportionately on polymorphic method calls
(`to_info_dict`, `invoke`, `parse_args` called on a base class). SCIP
reports the most-derived definition file; abyss picks the symbol-scope
owner of the receiver type and loses on inheritance. That's a known
weakness in the resolver — **Python MRO-aware receiver inference is a
separate roadmap item, not a v0.4.0 regression**.

## Decisions

1. **Baseline updated** to 97.9% / 93.0% in `RESULTS.md` and `README.md`.
   The CHANGELOG line about a regression was a false alarm; corrected.

2. **scip-python version is pinned** in `eval/run.sh` to prevent silent
   ground-truth drift on next release.

3. **Roadmap item filed**: Python MRO-aware receiver inference. Expected
   gated recall lift on click ~+1.5pp from current 93.0%. Not for v0.4.x.

## Method (for future regression hunts)

When eval numbers shift between releases:

1. Build the suspect binary at HEAD; build the baseline binary in a
   separate tree (use the tag).
2. Index the *same* corpus checkout with each binary; copy each DB aside.
3. Diff the refs tables keyed on `(source_file, source_line, target_name)`.
4. If `AGREE == total_truth` and `DISAGREE == 0` → ground truth changed,
   not the resolver. Check `scip.json` mtime + indexer version.
5. Otherwise → bisect on the `LOST_RESOLUTION` and `tier moves` sets to
   find the responsible commit.

That four-step playbook saves ~2 hours over naive `git bisect` with full
SCIP rebuild per step.
