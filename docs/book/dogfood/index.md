# Dogfood reports

<!-- Canonical source: docs/DOGFOOD.md. The summary table below is the
     at-a-glance row-per-corpus comparison; the long-form summaries
     ("Per-project summary", "What dogfood taught us", "Reproducing a
     dogfood") are included from DOGFOOD.md below. Edit per-corpus
     content in DOGFOOD.md; edit the comparison table here. -->

## Comparison at a glance

Six real codebases, six runs, six honest reports. Sort by what matters
to you — language, size, L0e (MRO-walker firings, the v0.4.0 → v0.5.x
maturation arc), or score.

| Project | Language | Files | Cold index | L0e hits | Score | Top finding |
|---------|----------|------:|-----------:|---------:|------:|-------------|
| [SQLAlchemy 2.0.36](sqlalchemy-2026-06-17.md) | Python (mixin / SQL-expr) | 687 | 8.41 s | **4 496** | **8 / 10** | L0e fires 9× the floor on declarative mixin towers; generic-base inherit drop (B3) drove v0.5.2 |
| [Django 5.1.4](django-2026-06-17.md) | Python (ORM / CBV / Admin) | 3 292 | 6.91 s | **9 450** | **8 / 10** | L0e validation case: 94× the FastAPI floor; B1 `kind='inherit'` invisibility drove v0.5.0 |
| [helix-editor @ `43bf7c2`](helix-editor-2026-06-17.md) | Rust workspace | 545 | 1.57 s | n/a | **7.5 / 10** | 100K-LOC Rust workspace in 1.5 s cold; topology signals honest, Rust labels need work |
| [vite v5.4.0](vite-2026-06-17.md) | TS / JS monorepo | 1 793 | 0.91 s | n/a | **7 / 10** | `callers ViteDevServer` returned 0 — drove v0.5.0 `callers` default-both fix |
| [FastAPI 0.115.4](fastapi-2026-06-17.md) | Python | 2 164 | 1.07 s | **0** | **6.5 / 10** | L0e predicted ≥50 hits, fired 0 — falsified in public, redirected toward Django |
| [hono v4.6.14](hono-2026-06-17.md) | TypeScript | 388 | 0.79 s | n/a | **8 / 10** | First dogfood; W1 UX debts fed v0.4.0; v0.5.0 wins observable on re-run |

Cold-index totals: **8 882 files, 6 runs, every cold pass under 9 s**.
The hash-incremental warm path stays under 400 ms even on the largest.
Hook p95 is 10–25 ms across every corpus.

## What this chapter contains

The summary below mirrors `docs/DOGFOOD.md` (single source of truth).
Edit comparison rows here; edit per-corpus prose in `DOGFOOD.md`.

{{#include ../../../docs/DOGFOOD.md:3:}}
