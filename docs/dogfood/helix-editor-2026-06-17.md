# abyss × helix-editor dogfood report

**Target**: `helix-editor/helix` @ `43bf7c2` (depth-1 clone, master)
**Abyss version**: v0.4.0 (commit b81764b)
**Date**: 2026-06-17
**Host**: Linux 6.17

---

## 1. Index stats — cold + warm

| metric | value |
|---|---|
| indexable files (walker output) | 545 |
| .rs files in tree | 243 |
| chunks | 1,875 |
| symbols | 5,580 |
| refs (resolved + unresolved) | 43,062 |
| modules clustered | 29 |
| DB size | 14 MB |
| **cold index wall-time** | **1.565 s** |
| parse | 147 ms |
| insert | 252 ms |
| resolve | 1146 ms |
| arch | 17 ms |
| git | overlapped (1 commit — shallow clone) |
| **warm reindex (no diff)** | **360 ms** (0 new refs) |

Resolver tier breakdown (refs resolved per tier):
```
L0  receiver-type       3280
L0c type-binding         102
L0d type-file             41
L0b import-binding      1972
L1  same-file           5862
L2  same-pkg-unique      639
L3  qualifier              0
L4  global-unique       4838
L4a same-file-qual      1504
L4b same-pkg-multi      2428
L5  ambiguous           5982
```

Note: coupling signal suppressed (1 commit history from `--depth 1` clone — not a bug, working as designed).

**Assessment**: cold-index latency on a 545-file / 5.6K-symbol Rust workspace is **excellent**. 1.5s is well inside the "<5s on medium codebases" budget. Warm reindex at 360ms shows the hash-incremental path is doing its job. Resolve dominates (1.1s) — the heaviest tier is L1 (5862, same-file) and L4 (4838, globally unique). Refs/file ratio of 79 is healthy for a Rust workspace this size.

---

## 2. arch_map module clustering

29 modules clustered. Top-15 by file count:

| files | label | sample |
|---|---|---|
| 29 | `helix-` | helix-stdx, helix-lsp-types files |
| 24 | `src` | helix-core/src/snippets/* |
| 20 | `helix--8` | helix-view/src/graphics.rs, helix-term/src/ui/lsp/* |
| 14 | `helix--9` | helix-view/src/{keyboard,theme,expansion}.rs |
| 11 | `helix-lsp` | helix-lsp-types/* |
| 11 | `helix--7` | helix-view/src/handlers/* |
| 11 | `src-4` | helix-core/src/{indent,surround,config}.rs |
| 9 | `helix--20` | helix-core/src/uri.rs + helix-view/src/handlers/lsp.rs |
| 9 | `src-12` | helix-core/src/command_line.rs, compositor.rs |
| 8 | `test` | helix-term/tests/test/* |
| 8 | `helix--10` | dap.rs across crates |
| 8 | `helix--6` | snippets/active.rs + completion handlers |
| 7 | `helix--11` | helix-core/src/increment/integer.rs, args.rs, lib.rs |
| 6 | `helix--21` | helix-loader/* |
| 6 | `helix-t` | helix-term/src/ui/{statusline, markdown, spinner}.rs |

mixed labels: **0** (no `mixed:` prefix anywhere).
distinct meaningful labels: ~6 (`helix-lsp`, `test`, `ui`, `indent`, `src`, `helix-t`).
degraded numeric fallbacks (`helix--N`, `src-N`): ~22 of 29.

**Assessment**: clustering itself is reasonable (look at the sample paths — `helix--8` truly groups view+term UI files, `helix--7` truly groups handlers, `helix--10` truly groups DAP code across 3 crates). The **clustering shape is OK; the labelling is broken**. Helix uses a workspace-with-shared-`src` layout (`helix-{view,core,term,tui}/src/*`) where the common path segments are `helix-X` and `src` — the labeler picks longest-common-substring and bails to `helix-`/`src` with numeric disambiguators. Need a Rust-workspace-aware labeler that strips the crate prefix and picks a representative subdirectory name instead.

**Concrete proposal**: if all module members share `helix-{crate}/src/{subdir}/...`, label as `{crate}/{subdir}` instead of `helix-`. That would turn `helix--8` into `view/lsp` (or `term/ui-lsp`), `helix--7` into `view/handlers`, `helix--10` into `dap`, etc.

---

## 3. Per-file `where` probes

| file | layer | module | role | conf | depth | cent | in | out | one-line |
|---|---|---|---|---|---|---|---|---|---|
| `helix-term/src/main.rs` | **entry** | `helix--5` | `entry_point` | 1.00 | 0 | 0.00 | 0 | 15 | Entry detection correct. |
| `helix-view/src/editor.rs` | unknown | `helix--9` | `bridge` | 0.00 | 2 | 0.01 | 54 | 54 | Hub correctly tagged bridge by topology, layer dictionary fails (no signal). |
| `helix-core/src/syntax.rs` | unknown | `src-4` | `bridge` | 0.00 | 2 | 0.01 | 38 | 28 | Same — topology right, layer blank. |
| `helix-term/src/commands.rs` | unknown | `src` | `bridge` | 0.00 | 2 | 0.01 | 34 | 83 | Bridge correct. out>in matches "commands push to many subsystems". |
| `helix-tui/src/buffer.rs` | unknown | `helix--8` | `bridge` | 0.00 | 2 | 0.01 | 36 | 13 | Bridge correct. Should probably classify as `ui` layer. |

Layer distribution across all 545 files:
```
455 unknown   (83%)
 35 test
 23 api
 16 util
  5 config
  3 entry
  3 generated
  3 infra
  2 domain
```

Layer confidence distribution:
```
455 conf=0
 89 conf=0.9+
  1 conf=0.5-0.9
```

Role distribution:
```
312 orphan
182 bridge
 34 entry_point
 17 leaf
```

Top-10 centrality (in-degree weighted):
```
cent=0.096  in=133 out= 17  unknown/bridge  helix-core/src/selection.rs
cent=0.058  in= 54 out= 37  util/bridge     helix-lsp-types/src/lib.rs
cent=0.047  in= 20 out=  0  util/leaf       helix-core/src/lib.rs
cent=0.029  in= 65 out= 10  unknown/bridge  helix-stdx/src/rope.rs
cent=0.026  in= 51 out=  5  unknown/bridge  helix-view/src/graphics.rs
cent=0.024  in= 46 out=  7  unknown/bridge  helix-lsp-types/src/lsif.rs
cent=0.023  in= 41 out= 11  unknown/bridge  helix-core/src/snippets/parser.rs
cent=0.022  in= 72 out= 14  unknown/bridge  helix-core/src/transaction.rs
cent=0.022  in= 57 out= 57  unknown/bridge  helix-view/src/document.rs
cent=0.021  in= 42 out= 13  unknown/bridge  helix-view/src/tree.rs
```

**Assessment**:
- Topological signals (role, centrality, in/out) **work great** on helix — the top-centrality list IS the right set of "if you change these, you break the editor" files (selection, rope, graphics, transaction, document, tree).
- Layer dictionary is **tuned for web/service architectures** and is dead weight here. 83% `unknown` with 0 confidence is a clear gap. The dictionary likely matches dir/file names like `auth`, `handler`, `middleware`, `service`, `controller`. Helix's vocabulary (`view`, `term`, `tui`, `core`, `lsp`, `dap`, `vcs`, `loader`, `stdx`, `event`) gets nothing. This is the **single largest UX gap** exposed by this dogfood.

---

## 4. Card body for `editor.rs`

**Total size**: **5014 bytes**. All 7 sections rendered with real data:

```
<abyss-card file="helix-view/src/editor.rs" epoch=... staleness_ms="15" precision_mode="heuristic">

where
  layer=unknown · module=helix--9 · role=bridge · conf=0.00
  depth_from_entry=2 · centrality=0.01 · in=54 out=54
  siblings(12): annotations.rs, clipboard.rs, document.rs, events.rs, expansion.rs, graphics.rs … +6 more

depends-on (top 6)               [OK · 6 rendered + "+2 more"]
depended-on  ⚠ HIGH BLAST RADIUS [OK · 937 prod callers, 45 files, 9 test callers]
  hottest: helix-view/src/tree.rs(224), helix-view/src/view.rs(208),
           helix-term/src/commands.rs(105)
contracts (exported symbols & callers)  [OK but noisy — see below]
recent activity                         [OK · 1 commits/30d · hotspot 165]
resolver                                [OK · degraded=false · 55 ambiguous refs]
```

| section | rendered? | data quality |
|---|---|---|
| where | yes | layer + conf blank (see §3), rest good |
| depends-on | yes | 8 dependency edges, sensible |
| depended-on (blast radius) | yes | **excellent** — 937 prod callers across 45 files, names top callers |
| contracts | yes | see noise below |
| recent activity | yes | hotspot 165 matches the map (3rd of 10) |
| resolver | yes | honest about 55 ambiguous in-file refs |

**Contract section noise** — observed in the wild:
```
default (method) → [35 callers, 0 tests]      ← repeats 18 times
default (method) → [35 callers, 0 tests]
default (method) → [35 callers, 0 tests]
...
```
The contracts list shows every method named `default` (each from a different struct's `impl Default`) as a separate line, all with the same caller count, because they're being looked up by bare name without scope qualification. This collapses to "`default` is called 35 times somewhere" instead of "`Foo::default` is called X, `Bar::default` is called Y". 18 of ~80 lines are duplicated like this. Fix: include `impl_target::method` in the rendered name for Default/From/etc.

Same problem with `from (method) → [0 callers, 0 tests]` showing twice — different `From` impls collapsing.

---

## 5. Hook latency p50/p95

10 pre-edit hooks across hub files (cold-OS-cache, hot-DB):

| file | ms |
|---|---|
| helix-view/src/editor.rs | 22 |
| helix-core/src/syntax.rs | 14 |
| helix-term/src/commands.rs | 10 |
| helix-tui/src/buffer.rs | 7 |
| helix-term/src/main.rs | 6 |
| helix-core/src/lib.rs | 5 |
| helix-view/src/document.rs | 10 |
| helix-view/src/view.rs | 9 |
| helix-term/src/ui/editor.rs | 9 |
| helix-core/src/selection.rs | 15 |

Sorted: 5, 6, 7, 9, 9, 10, 10, 14, 15, 22.

- **p50 = 9.5 ms**
- **p95 ≈ 22 ms**

Well under the "<200ms agent-blocking budget". The 22ms outlier (`editor.rs`) is the heaviest file (54 in, 54 out, 5KB card body) — still fine.

---

## 6. Search probe results

### `abyss search "Editor"` top 5
1. `helix-view/src/editor.rs:1267` `enum EditorEvent` (class) — **canonical hit**
2. `helix-core/src/editor_config.rs:11` — relevant
3. `helix-term/src/ui/editor.rs:1` — relevant (the UI's Editor wrapper)
4. `helix-core/src/editor_config.rs:274` `mod test` — test-module chunk (minor pollution, but it does contain "Editor")
5. `helix-term/src/application.rs:645` `handle_editor_event` — relevant

No `tests/test/*.rs` integration-test pollution. Good.

### `abyss search "command"` top 5
1-4. All four hits are different chunks of `helix-dap-types/src/lib.rs` (DAP protocol structs with `command: String` fields)
5. `helix-view/src/clipboard.rs:68` `struct Command`

**FTS bias observation**: when one file contains the keyword many times (DAP protocol stubs all carry a `command:` field), FTS5 scoring concentrates on it. The actually-canonical `Command` struct ranks 5th. This isn't pollution — the chunks are real — but it does mean BM25 ranking buries the canonical answer. Symbol-name match should probably get a boost.

---

## 7. Bugs / UX debts

1. **arch_map labeler degraded on Rust-workspace layouts**. Numbered fallback labels (`helix--8`, `src-12`) dominate when the workspace shares a common `helix-` prefix and `src/` directory. Fix: cargo-workspace-aware labeler that strips crate prefix and picks the dominant sub-path. Concrete heuristic: if cluster shares `{prefix}-{crate}/src/{subdir}` for >50% of members, label as `{crate}/{subdir}`.
2. **Layer dictionary blind to editor/UI vocabulary**. 83% of files come back `layer=unknown` `conf=0.00`. The dictionary needs entries for `view`, `term`, `tui`, `ui`, `editor`, `command*`, `event*`, `lsp`, `dap`, `vcs`, `stdx`, `loader`, `core`. Until then, layer is dead metadata on terminal/editor/CLI codebases — they all read as `unknown/bridge`.
3. **Contract section collapses trait method names across impls**. `default (method) → [35 callers]` repeats 18× in editor.rs because every `impl Default for X` registers as a bare `default` method. Need to render as `X::default` so the rows are distinguishable and not falsely showing the same caller count.
4. **FTS-only ranking buries canonical symbol hits**. `search "command"` returns 4× `helix-dap-types/lib.rs` before the actual `struct Command`. Symbol-name exact/case-match should have a tier-bonus over BM25.
5. **Module-id encoding visible in `where` output**. Showing `module=helix--9` to a human (or an agent) provides zero information. The number after the dash is just an internal id. The label should always be human-readable; if the cluster has no good name, say `cluster-9` or `unlabelled-9` to signal "we couldn't name this" rather than appending a digit to a fragment.
6. **Coupling silently empty on shallow clones**. Working as designed but worth a note in `stats` or `map` output: "coupling: insufficient history (need ≥50 commits, have N)". Currently only visible in indexer log.
7. **Warm reindex log says `0 refs`** — misleading when reading the line in isolation. Means "0 new refs inserted", not "0 refs in index". Consider phrasing `0 changed refs / 43062 total`.

None of these are correctness regressions — the per-file `where` data, blast radius, and topological signals are all honest. The bugs are all in labelling / ranking presentation.

---

## 8. Final score: **7.5 / 10**

What's load-bearing and works:
- **Cold index on 545 files in 1.5s** — well within budget.
- **Topology signals (in/out/centrality/role) are honest and useful** — top-10 centrality list is the right set of hub files.
- **Blast radius on `editor.rs` (937 prod callers across 45 files) is the single most useful signal the card delivers**.
- **Hook latency p95=22ms** — never blocks an agent.
- **Resolver tier ordering** holds up: 0 L3, 12K resolved at L0+L0c+L0d+L0b+L1+L2 (≈ 28%), only 5.9K ambiguous at L5 of 43K total (≈14%).
- **No test/import pollution** in the search top-5 — the v0.4.0 hygiene work survives.

What costs the 2.5 points:
- L0 layer dictionary contributes ~zero on this codebase (455/545 unknown), so the `where` summary's first line is dead.
- arch_map labels are mostly degraded numbers — useful to humans only after manually inspecting members.
- Contracts section duplicates trait-method rows confusingly.
- Search ranking buries canonical symbol matches under FTS-keyword frequency.

Net: **abyss handled a real 100K-LOC Rust workspace cleanly with no crashes, no degraded resolver, no hangs**. The structural signal layer is solid; the L0 architectural labels and contract presentation are the weakest links and the main upgrade targets after this dogfood.
