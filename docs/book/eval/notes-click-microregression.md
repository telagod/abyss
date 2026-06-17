# Notes: click microregression (2026-06-17)

The click corpus's gated precision/recall drifted 98.7/94.6 → 97.9/93.0
between v0.3.6 and v0.4.0 — with zero abyss code change. Root cause:
`scip-python` v0.6.6 started emitting 16 extra truth pairs.

Full incident note:

{{#include ../../../eval/notes/click-microregression-2026-06-17.md}}
