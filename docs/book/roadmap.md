# Roadmap

The roadmap is intentionally short — abyss earns features by being
proven against the eval and dogfood corpora, not by being promised.

## v0.6 candidates

These are on deck but not committed; expect them to ship only if
dogfooding turns up the underlying debt.

- **MCP-over-socket (Daemon V2)** — let the hook, the MCP server, and
  an editor extension share one in-process index. The V1.5 daemon
  already speaks JSON over a Unix socket; V2 extends that to the full
  MCP surface so the pre-edit fast path stops doing direct SQLite
  reads.
- **Stub-class registry for external hierarchies** — the FastAPI
  dogfood falsified the MRO L0e walker; the real fix is letting
  abyss know about external base classes (Starlette, pydantic, the
  stdlib) so the chain doesn't dead-end on the first hop. Design
  tradeoff: stub footprint vs accuracy.
- **scip-java ground truth** — bring Java into the SCIP eval. Today
  Java is covered by contract tests only.
- **Interface dispatch in Go** — interface-typed receivers stay
  demoted at 0.6 by design. A first-pass interface-satisfaction
  approximation could rescue some of that recall without claiming
  compiler-grade accuracy.

## Won't fix (by design)

- **Dynamic / metaprogrammed methods** (TS router verbs, Python
  `__getattr__`, Ruby `method_missing`) stay unresolved or demoted.
  They are correctly unresolvable by static naming; agents see them
  in `possible_callers`. abyss is not a compiler.
- **Data-flow receiver inference**. The lite inference rules
  (method receivers, typed params, constructor initializers,
  `self` / `this`) are intentional. Full data flow is compiler
  territory; the precision/recall numbers say the lite version is
  enough for the agent use case.

## How to influence the roadmap

Open an issue with a dogfood result — a real codebase, a real
question abyss got wrong, and the precision/recall delta you'd
want. The eval-driven release cadence is the only reliable way to
move the tiers.
