# The pre-edit card

Before the agent edits a file, abyss prints a compact `<abyss-card>`
that summarizes the file's role in the codebase. The model reads this
card instead of grepping around — the goal is *minimum sufficient
context* for one edit.

A typical card carries:

- `where:` — the architectural layer (`api`, `domain`, `infra`,
  `util`, `entry`, `test`, `config`, `vendor`, `generated`) inferred
  from the path-segment dictionary. See
  [L0 architectural coordinates](../architecture/arch-layers.md).
- `callers:` — top callers above the confidence gate
  (default `min_confidence=0.7`). Type refs and call refs are both
  included; pass `--calls-only` or `--types-only` to narrow.
- `coupled:` — change-coupled neighbors, mined from git history. These
  are the files that historically changed together — the implicit
  contract the model should keep in mind.
- `risk:` — blast-radius score 0–10, computed from direct/transitive
  callers and uncovered paths.
- warnings — emitted as separate stderr lines that the hook treats as
  blocking (e.g. "production callers", "ambiguous reference", "hotspot
  override needed").

Hooks must never block the agent on infrastructure errors — `cmd_hook`
silently succeeds on every error path. The only stderr you'll see is
actionable.
