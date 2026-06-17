# click eval: Python MRO walker (L0e tier) — minimal lift, expected

**Date:** 2026-06-17 (post-v0.4.0)
**Status:** Closed. Shipped. Baseline numbers unchanged.

## TL;DR

Added L0e: a Python MRO-aware receiver-inference tier that walks
`inherit` refs (left-to-right DFS, 6-hop cap) from a typed receiver up
the class hierarchy looking for a base class that owns the called method.
Resolves at confidence 0.95 when found.

On click 8.1.8 the new tier fires **2 times** and the gated precision /
recall numbers are unchanged (97.9% / 93.0%, same as the post-ground-truth-
refresh baseline). The hits are correct — they're cases where the
inheritance walker arrived at the right answer at honest confidence
where the same-file tier had previously claimed them at 1.0 — but the
file-of-record was identical, so the overall correctness numbers don't
move.

## Why the lift is small on click

The D2 differential analysis named "16 polymorphic dispatches" as the
miss class behind the 6.6pp recall gap. Inspecting click's class
hierarchy explains why an MRO walker can't recover most of them:

- `BaseCommand`, `Command`, `MultiCommand`, `Group`, `CommandCollection`
  all live in **`src/click/core.py`** (lines 1163, 1480, 1789, 1961).
- `ParamType` and its 14+ subclasses (`Choice`, `IntRange`, `File`,
  `Path`, ...) live in **`src/click/types.py`**.
- `ClickException` and its 9 subclasses live in
  **`src/click/exceptions.py`**.
- `ShellComplete` and its 3 subclasses live in
  **`src/click/shell_completion.py`**.

When a subclass and its base co-locate in one file, the *same-file* tier
(L1, confidence 1.0) already resolves polymorphic method calls
correctly — the file pointer is right, just the receiver-vs-defining-
class distinction is collapsed. The L0e tier only fires when L0/L0c/L0d
have all missed AND the method definition is in a different file from
the receiver type's declaring file.

Click happens to put its inheritance hierarchies in single files. So
L0e only catches the few cross-file cases that exist (e.g. mixins, or
subclasses in `decorators.py` / `shell_completion.py` extending bases in
`core.py`). The 2 hits we observe are exactly those.

## Evidence

Baseline (HEAD before this change, 57fe61b):

```
=== click — abyss vs SCIP ground truth ===
ground-truth call pairs: 589   unresolved by abyss: 8
  tier  correct  wrong  precision
   1.0      178      1      99.4%
  0.95      348     11      96.9%
   0.9        0      0          —
   0.8       22      0     100.0%
   0.6       15      5      75.0%
   0.5        0      1       0.0%
 gated@0.7: precision 97.9%  recall 93.0%  (548/560 predicted, 589 truth)
       all: precision 96.9%  recall 95.6%  (563/581 predicted, 589 truth)
```

After L0e (this change):

```
=== click — abyss vs SCIP ground truth ===
ground-truth call pairs: 589   unresolved by abyss: 8
  tier  correct  wrong  precision
   1.0      176      1      99.4%
  0.95      350     11      97.0%
   0.9        0      0          —
   0.8       22      0     100.0%
   0.6       15      5      75.0%
   0.5        0      1       0.0%
 gated@0.7: precision 97.9%  recall 93.0%  (548/560 predicted, 589 truth)
       all: precision 96.9%  recall 95.6%  (563/581 predicted, 589 truth)
```

Net delta on click 8.1.8:

- L0e fired 2 times (`L0e(py-mro)=2` in the resolver log).
- Those 2 refs MOVED from tier 1.0 (same-file) to tier 0.95 (MRO walker).
- Both were already correct — the same-file tier was claiming them at
  1.0 because base and subclass shared a file, but the receiver-type
  evidence (subclass) + inheritance-edge (base) + method-on-base path
  is more honest about *why* the answer is right.
- Tier 1.0 dropped from 178→176 correct (-2 correct, 0 wrong delta).
- Tier 0.95 grew from 348→350 correct (+2 correct, 0 wrong delta).
- Gated@0.7 totals: identical at 548/560 predicted, 97.9/93.0.

## What would move the needle

The cases L0e *would* meaningfully resolve — cross-file inheritance with
a typed receiver — are rare in click's codebase but common in larger
Python projects with `protocols.py` / `interfaces.py` / `base.py` /
implementation-file conventions (e.g. SQLAlchemy, FastAPI plugins,
Django mixins, large CLI frameworks where command subclasses live next
to their commands). The unit tests in `tests/python_mro_receiver.rs`
exercise exactly that shape: `base.py` → `derived.py` → `caller.py`.

A bigger lift on click specifically would need either:

1. **Multi-class-owner symbol scope.** Today `Group.invoke` and
   `Command.invoke` have separate symbol rows; the L0 tier picks one and
   misses the other. MRO walker bridges this, but only when the receiver
   type is statically known AND the method definition crossed a file.
2. **Cross-method type flow.** `ctx.command.invoke(ctx)` — abyss infers
   `ctx: Context` but loses the chain when `command` is `Command|None`
   (Optional). A union-aware receiver inference would help.

Both are larger V2/V3 work.

## Decisions

1. **L0e shipped** despite the small lift on click: the walker is
   correct, bounded (6-hop cap), and the unit tests cover the
   single-inheritance / three-level / multi-base / negative cases. The
   value will appear on other Python corpora.

2. **No baseline change** in `RESULTS.md`. Click stays at 97.9% / 93.0%
   gated. Adding a row would just say "same numbers."

3. **Follow-up filed** for cross-method type flow (Optional / union
   receivers) — that's where the next big Python recall jump comes from.
