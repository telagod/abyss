# Dogfood reports

We run abyss on real codebases, score it 0–10, and ship the verdict
even when it's unflattering. Each report carries:

- index stats (cold + warm wall time, DB size, resolver tier hits)
- the questions an agent asked the index and whether they got useful
  answers
- the debts and surprises surfaced
- a score 0–10 with the reasoning

## v0.4.0 reports

| Corpus | Lang | Score | Headline |
|--------|------|------:|----------|
| [helix-editor](./helix-editor.md) | Rust | 7.5/10 | Big-Rust workspace, contracts dedup needed |
| [vite](./vite.md) | TS + JS | 7/10 | TS type refs surfaced as the highest-ROI debt |
| [FastAPI](./fastapi.md) | Python | 6.5/10 | MRO L0e walker fell flat — hierarchy roots are external |

hono (7/10) was the original TypeScript dogfood. Its report lives in
git history at `docs/dogfood/hono-…-2026-06-17.md` if present in your
checkout.

## What dogfood drove in v0.5.0

The four reports above are not anecdotes — they each turned into
shipped fixes:

- **vite** → `callers ViteDevServer` was returning **0** instead of
  the real **71** because the command silently filtered out type
  refs. v0.5.0 includes type refs by default; new `--calls-only` /
  `--types-only` flags.
- **helix-editor** → contracts dedup, monorepo labeller honesty
  (`p-18` → `vite-18`, `mixed:temporal+` → `cluster-1`).
- **FastAPI** → MRO L0e hypothesis falsified. Logged honestly in
  [eval/notes/click-mro-walker-2026-06-17.md](../eval/notes-mro-walker.md).
