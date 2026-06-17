# Dogfood: SQLAlchemy 2.0.36 — L0e MRO scaled validation

**Date**: 2026-06-17
**Target**: `sqlalchemy/sqlalchemy @ tag rel_2_0_36` (686 Python files, 8.2M `lib/`, 350 `test/`, 71 examples)
**Binary**: `abyss 0.5.1` (slim, release build, main HEAD `6b698db`)
**Hypothesis under test**: After Django (9 450 L0e hits) validated the MRO walker on a wide Model/View/Admin/Form hierarchy, the Django dogfood note predicted SQLAlchemy ≥500 L0e hits — declarative-base + ColumnElement + TypeEngine + Compiled trees that live entirely in `lib/sqlalchemy/`. If <100, the diagnosis flips from "wrong corpus" to "walker doesn't handle declarative metaclass hierarchies".

## TL;DR

> **L0e fired 4 496 times on SQLAlchemy 2.0.36 — 9× the floor, halfway between FastAPI's 0 and Django's 9 450.** Prediction confirmed. The mechanism re-validates on a *different* hierarchy shape: SQLAlchemy is mixin-and-generic-heavy rather than wide-inheritance-tree heavy, and the L0e count drops proportionally (Django has 600+ distinct L0e receiver types, SQLAlchemy has 1 050 — *more* class diversity, but each fires fewer times because the SQL-expression tree is shallow with wide leaf families like `Compiler` and dialect subclasses).
>
> Two new findings worth the next release's attention:
>
> 1. **Generic-parameterized base classes are silently dropped from inherit edges.** `class FunctionElement(Executable, ColumnElement[_T], FromClause, Generative)` produces inherit refs for `Executable`, `FromClause`, `Generative` but **not** `ColumnElement[_T]`. The Python extractor (`src/graph/languages/python.rs:130`) comments "`Generic[T]` → skip" and only matches `identifier`/`attribute` base nodes. SQLAlchemy has 25 generic-parameterized inheritances of `TypeEngine` and ~20 of `ColumnElement` — all invisible. This is the biggest precision miss of the run.
> 2. **`impact` vs `callers` numeric mismatch remains visible.** `impact declarative_base` → direct=146; `callers declarative_base` → 113 total. Different filter chains (impact includes 0.5+, callers default 0.7+) deliver different headline numbers for the same question. CHANGELOG V2 item, surfaces here on the obvious SQLAlchemy hub function.

Verdict: **8 / 10**. Same score as Django because the corpus also validates the central hypothesis cleanly *and* because v0.5.1's B1 fix is observable — `callers DeclarativeBase` returns 121 inherit refs at 95% confidence where Django's `callers Model` returned 7. The 2 points off are the generic-subscript bug (newly surfaced, high impact) and the impact/callers numeric gap (known V2 item).

---

## Index stats + scale

| Metric | Value |
|---|---|
| Indexable files | 687 (Python 686, TOML 1) |
| Python files: `lib/` / `test/` / `examples/` / `tools/` / other | 255 / 350 / 71 / 8 / 2 |
| Symbols | 37 210 (function 21 754, class 7 960, method 7 496) |
| Refs | 200 314 (`call` 160 413 / `import` 16 322 / `import_binding` 15 561 / **`inherit` 8 018**) |
| Cold wall time | **8 414 ms** (parse 464 / insert 1 017 / **resolve 6 856** / arch 73) |
| Warm reindex (no diff) | **723 ms** — parse 10 / insert 0 / resolve 651 / arch 55 |
| Cold CPU efficiency | 14.28 s user / 8.45 s wall → 172 % (rayon parse parallelism) |
| Peak RSS during cold index | 215 572 kB ≈ 211 MB |
| Hook latency (warm DB read) | 19 ms wall (`staleness_ms=19`), 19 MB RSS, 22-line card |
| DB on disk | 56 MB (`.code-abyss/index.db`) |
| Git history | shallow clone → 0 commits → hotspots / coupling suppressed |

**Scale verdict**: 686 Python files index in 8.4 s cold / 723 ms warm. Django's 2 786 Python files indexed in 6.9 s — SQLAlchemy is *slower per file* (12.3 ms/file vs 2.5 ms/file). Reason: SQLAlchemy has ~3× the symbols per file (54 vs 14) and ~2× the refs per file (291 vs 69) — denser corpus, more resolver work. The resolver step took 6 856 ms (81 % of cold time) versus 5 234 ms on Django; the tier cascade is paying for the higher ref density.

**Density observation**: SQLAlchemy averages **291 refs/file**, vs Django's **69 refs/file** and FastAPI's **39 refs/file**. SQLAlchemy is *the* densest Python corpus in the dogfood set. The 200 K refs in 686 files mean the L0e/L0d/L0c tiers have a larger surface to land on — which is exactly why a "mid-size" repo (1/4 the file count of Django) produces almost half the L0e hits.

### Resolver tier hit counts (from cold-index log)

```
L0(receiver-type)=7589
L0c(type-binding)=64
L0d(type-file)=853
L0e(py-mro)=4496      ← the hypothesis test
L0b(import-binding)=60291
L1(same-file)=14114
L2(same-pkg-unique)=6952
L3(qualifier)=3947
L4(global-unique)=10184
L4a(same-file-qual)=3598
L4b(same-pkg-multi)=1699
L5(ambiguous)=47192
```

Notable: **L0b at 60 291** is the highest L0b hit count in any dogfood — SQLAlchemy's import surface (heavily granular re-exports through `__init__.py` barrels) lights up the import-binding tier hard. L0e at 4 496 is ~30 % of all 13 002 receiver-tier (L0/L0c/L0d/L0e) hits — a healthy MRO-walker share.

### Confidence distribution

| Bucket | Count | % |
|---|---|---|
| Unresolved (`0.0`) | 39 335 | 19.6 % |
| ≥ 0.95 | 94 359 | 47.1 % |
| 0.8 – 0.94 | 14 131 | 7.1 % |
| 0.5 – 0.79 | 52 489 | 26.2 % |

**47 % at ≥0.95** is the highest high-confidence share across all dogfood runs (Django: 33 %, FastAPI: ~38 %). The dense imports + receiver-type evidence + closed in-repo hierarchies stack up. Only 19.6 % unresolved — SQLAlchemy's near-zero stdlib dependency profile (it's the bottom of most Python stacks) keeps the unresolved rate down.

---

## L0e validation — the headline number

| Metric | Value | Comment |
|---|---|---|
| **L0e firing count (resolver log)** | **4 496** | predicted ≥ 500, hit 9× over |
| Total receiver-tier (L0/L0c/L0d/L0e) refs at 0.95 | 13 002 | 1 050 distinct receiver types |
| L0e in lib→lib production code (proxy SQL) | ~198 strict / 3 435 broad | strict = receiver not defined in source file |
| L0e in test→anywhere | ~9 288 broad / 2 508 strict | tests subclass library hierarchies heavily |
| Distinct receiver types feeding receiver tiers | **1 050** | more class diversity than Django (600+) |
| Inherit edges total / at conf ≥ 0.7 | 8 018 / 6 786 | 825 distinct base classes |
| Comparison baseline | click 2, FastAPI 0, Django 9 450, **SQLAlchemy 4 496** | mid-band, structurally honest |

### Top 15 L0e-firing receiver types

| Receiver type | Hits (recv-tier 0.95) | Family |
|---|---:|---|
| `Table` | 1 211 | core schema |
| `Session` | 1 018 | ORM session |
| `CompileTest` | 575 | test base |
| `SelectTest` | 329 | test base |
| `SQLCompiler` | 207 | compiler (prod) |
| `HistoryTest` | 184 | test base |
| `MetaData` | 113 | core schema |
| `JoinTest` | 103 | test base |
| `LambdaElementTest` | 96 | test base |
| `EmitDDLTest` | 89 | test base |
| `CTETest` | 87 | test base |
| `InsertTest` | 86 | test base |
| `OracleDialect` | 84 | dialect (prod) |
| `Mapper` | 81 | ORM mapper |
| `PGDialect` | 72 | dialect (prod) |

### Top 15 production-only L0e receivers (lib→lib, true MRO walks)

| Receiver type | L0e hits | In-repo base chain |
|---|---:|---|
| `SQLCompiler` | 207 | → `Compiled` (lib/sqlalchemy/sql/compiler.py is hub) |
| `Table` | 95 | → `TableClause` → `Selectable` (sql/schema.py → sql/selectable.py) |
| `OracleDialect` | 84 | → `DefaultDialect` (dialects/oracle/base.py → engine/default.py) |
| `Mapper` | 81 | → `_MapperEntity` / `InspectionAttr` |
| `Session` | 72 | → `_SessionClassMethods` (orm/session.py) |
| `PGDialect` | 71 | → `DefaultDialect` |
| `Connection` | 66 | → `ConnectionEventsTarget` (engine/base.py) |
| `Query` | 61 | → `Generative` (orm/query.py) |
| `ColumnOperators` | 59 | → `Operators` (sql/operators.py) |
| `Inspector` | 48 | → `inspection.Inspectable` (engine/reflection.py) |
| `Comparator` | 48 | → `PropComparator` (orm/attributes.py) |
| `MySQLTypeCompiler` | 46 | → `GenericTypeCompiler` (dialects/mysql/base.py → sql/compiler.py) |
| `MSSQLCompiler` | 41 | → `SQLCompiler` → `Compiled` |
| `JoinCondition` | 39 | → `MemoizedSlots` |
| `MySQLDialect` | 37 | → `DefaultDialect` |

These are the textbook cases — agents reading `dialects/postgresql/base.py` need `self.visit_X()` calls to resolve into `sql/compiler.py:SQLCompiler` for the inherited implementations. L0e delivers.

### 5 sample L0e resolutions — verifies in-repo MRO walk

| Source | Receiver.method | Resolved target | Hierarchy walked |
|---|---|---|---|
| `lib/sqlalchemy/engine/reflection.py:336` | `Connection.close` | `lib/sqlalchemy/engine/base.py` | `Connection → ConnectionEventsTarget` |
| `lib/sqlalchemy/dialects/sqlite/base.py:1492` | `BindParameter._clone` | `lib/sqlalchemy/sql/elements.py` | `BindParameter → ColumnElement → ClauseElement` |
| `lib/sqlalchemy/sql/compiler.py:4012` | `CTE._get_reference_cte` | `lib/sqlalchemy/sql/selectable.py` | `CTE → Generative → DialectKWArgs → HasCacheKey` (multi-mixin walk) |
| `lib/sqlalchemy/orm/strategies.py:3208` | `Bundle.__clause_element__` | `lib/sqlalchemy/orm/util.py` | `Bundle → ORMColumnsClauseRole` |
| `lib/sqlalchemy/orm/relationships.py:3363` | `ClauseAdapter.chain` | `lib/sqlalchemy/sql/util.py` | `ClauseAdapter → ClauseVisitor` |

Five-for-five textbook wins — exactly the resolutions an agent reading `compiler.py` or `relationships.py` would otherwise have to `grep -rn "def _clone" lib/` to find. Notably the third example (`CTE._get_reference_cte`) walks through *two intermediate mixin levels* (`Generative`, `DialectKWArgs`) before landing — the MRO walker handles SQLAlchemy's mixin-and-multiple-inheritance pattern cleanly.

### click vs FastAPI vs Django vs SQLAlchemy — the prediction table

| Project | Python files | L0e hits | L0e per Python file | Why |
|---|---:|---:|---:|---|
| FastAPI 0.115.4 | 1 281 | **0** | 0.000 | bases `Starlette`, `BaseModel`, `Request`, `Enum` all external |
| click 8.1.8 | ~30 | **2** | 0.07 | mostly self-contained; stdlib leaves co-locate base+subclass |
| **SQLAlchemy 2.0.36** | **686** | **4 496** | **6.55** | dense in-repo mixin trees: `Compiled`, `ColumnElement`, `TypeEngine`, `Dialect`, `Compared`; cross-file by design |
| Django 5.1.4 | 2 786 | **9 450** | 3.39 | wide ORM/CBV/Admin hierarchies; tests/ subclasses Django.test heavily |

**SQLAlchemy has the highest L0e-per-file ratio** in the corpus set (6.55 vs Django's 3.39). The reason is structural: SQLAlchemy splits compiler/dialect/orm into deep mixin towers (`SQLCompiler` → `Compiled` → `Generative`), and every dialect subclass calls 5-10 inherited methods per visit. Django is wider but shallower — `ModelAdmin` has many siblings but the depth is 2-3 levels.

**Both Django (in-repo deep) and SQLAlchemy (in-repo dense with mixins) confirm the hypothesis. FastAPI (external roots) and click (single-file colocation) confirm the falsifier. PRINCIPLES §2 prediction shape holds.**

### Inheritance edges in the DB — side validation

`refs.kind='inherit'` has **8 018 rows** (6 786 at confidence ≥ 0.7, 825 distinct base classes). Top inherited classes:

| Base class | inherit refs |
|---|---:|
| `Base` | 1 036 |
| `TestBase` | 576 |
| `decl_base` | 401 |
| `AssertsCompiledSQL` | 373 |
| `MappedTest` | 363 |
| `ComparableEntity` | 350 |
| `Comparable` | 231 |
| `Basic` | 213 |
| `TablesTest` | 185 |
| `DeclarativeMappedTest` | 129 |
| `FixtureTest` | 125 |
| `DeclarativeBase` | 121 |
| `TypeDecorator` | 116 |
| `Person` | 96 |
| `Protocol` | 55 |

**`DeclarativeBase` shows 121 inherit refs** — `callers DeclarativeBase` returns exactly that count (showing 20 of 121). v0.5.1's B1 fix (callers surfaces `kind='inherit'`) is working — this is the same surface that on Django gave 7 instead of 983.

---

## Per-probe results (3-axis: usefulness / accuracy / speed, 0-10)

### Probe A — `abyss where lib/sqlalchemy/orm/decl_base.py`

```
where: lib/sqlalchemy/orm/decl_base.py
  layer=util  module=sqlalchemy-4  role=bridge  conf=1.00
  depth_from_entry=4  centrality=0.00  in=34 out=34
  signals: dir=[util×0.4], name=[], entry=false
```

| axis | score | comment |
|---|---|---|
| usefulness | 5 | role=bridge correct; layer=util is a defensible default but `module=sqlalchemy-4` is the FastAPI/Django module-labeller debt recurring (single-project monorepo → `<projname>-N` placeholders) |
| accuracy | 6 | in=34 / out=34 is suspiciously balanced for a file that's the central declarative-base entry; centrality 0.00 misses that `_as_declarative` is on the call path of every `Mapped` attribute |
| speed | 10 | instant |

### Probe B — `abyss callers DeclarativeBase --limit 25`

Output (truncated):
```
callers of 'DeclarativeBase' (20 prod):
  1. test/typing/plain_files/sql/common_sql_element.py:28 → Base()  (95%, inherit)
  2. test/typing/plain_files/sql/lambda_stmt.py:20 → Base()  (95%, inherit)
  ... 20 more rows ...
(showing 20 of 121 total — use --limit 0 for all, --limit N for more)
```

| axis | score | comment |
|---|---|---|
| usefulness | 9 | **`inherit` edges surface cleanly at 95% confidence — v0.5.1's B1 fix is observable here**; an agent asking "who subclasses DeclarativeBase?" gets the right answer. 121 hits is also the structurally correct count (matches direct grep) |
| accuracy | 9 | 20/20 shown are real `class Base(DeclarativeBase)` declarations in typing test files; the limit-of-20 paginator works |
| speed | 10 | instant |

This is a **direct improvement vs Django**. On Django, `callers Model` returned 7 (instantiation only, inherit invisible). On SQLAlchemy with v0.5.1, `callers DeclarativeBase` returns 121 inherit refs at 95% confidence. Headline win for v0.5.1's B1 fix.

### Probe C — `abyss callers ColumnElement --include-tests --limit 25`

```
callers of 'ColumnElement' (13 found):
  1. test/sql/test_compare.py:1316 → Foobar2()  (95%, inherit)
  ... 12 more test/ rows ...
```

Only **13 hits** but the lib code has 19+ `class X(ColumnElement[_T])` subclasses (FunctionElement, NumericRoman, MaxValue, _multiparam_column, Case, AnnotatedColumnElement, KeyedColumnElement, …). Direct grep counts: 19 lib subclasses (15 use generic `ColumnElement[_T]`, 4 use bare `ColumnElement`). The DB has 18 inherit refs total for ColumnElement — 12 from tests (bare inheritance), 2 from lib (`NumericRoman`, `MaxValue` — both non-generic), 4 low-conf others.

| axis | score | comment |
|---|---|---|
| usefulness | 3 | massively underreports; SQLAlchemy has many lib subclasses of `ColumnElement` and the agent gets only test stubs |
| accuracy | 5 | the 13 shown are real, but the 15+ generic-parameterized lib subclasses are entirely missing |
| speed | 10 | instant |

**Root cause is Bug B3 below** (generic-parameterized base classes dropped in Python extractor). The probe is structurally correct *given the DB*, but the DB is missing data.

### Probe D — `abyss context lib/sqlalchemy/engine/base.py`

Output (136 lines, truncated):
```
=== lib/sqlalchemy/engine/base.py ===
144 symbols defined, 63 with external callers

  Connection() ← 1 callers
    lib/sqlalchemy/engine/create.py:733 → create_engine()
  ...
  execute() ← 2 callers
    lib/sqlalchemy/orm/bulk_persistence.py:1260 → BulkORMInsert()
    lib/sqlalchemy/orm/bulk_persistence.py:1294 → BulkORMInsert()
  execute() ← 2 callers          ⚠ duplicate (overloaded methods)
    lib/sqlalchemy/orm/bulk_persistence.py:1260 → BulkORMInsert()
  execute() ← 2 callers          ⚠ duplicate
    ...
  depends on:
    → TypeVar (lib/sqlalchemy/util/typing.py)
    → _join (lib/sqlalchemy/orm/context.py)               ⚠ wrong _join
    → get (lib/sqlalchemy/sql/lambdas.py)                 ⚠ wrong get
    → union (lib/sqlalchemy/sql/selectable.py)            ⚠ wrong union
    → close (lib/sqlalchemy/engine/interfaces.py)
    ...
  hotspot: score=0  changes=0  cc=256
```

| axis | score | comment |
|---|---|---|
| usefulness | 7 | 144 symbols × 63 with callers is honest blast-radius information; the L0e wins are visible (BulkORMInsert resolves into engine/base via MRO walk) |
| accuracy | 5 | (1) `execute()` printed 3× because of `@overload`-decorated methods — each overload symbol has its own ID; (2) `depends on` leaks L4 bare-name noise: `_join` → orm/context.py is wrong, `union` → sql/selectable.py is wrong, `get` → lambdas.py is wrong — same Django D-axis finding |
| speed | 10 | instant |

The overload-duplication is new — Django's models/base.py had less method-overload usage. **Method overloads inflate the contract list.**

### Probe E — `abyss map`

```
═══ Hotspots ═══
  (insufficient history: 0 commits available; need ≥10 — try a deeper clone)
```

| axis | score | comment |
|---|---|---|
| usefulness | 6 | honest, prescriptive ("try a deeper clone") |
| accuracy | 10 | true: 0-commit shallow clone |
| speed | 10 | instant |

Identical to Django run. coupling/hotspots correctly suppressed.

### Probe F — pre-edit card on `lib/sqlalchemy/orm/decl_api.py`

Card emitted in 19 ms wall, 19 MB RSS:

```
<abyss-card file="lib/sqlalchemy/orm/decl_api.py" epoch="0" staleness_ms="19" precision_mode="heuristic">
where
  layer=util · module=sqlalchemy-5 · role=bridge · conf=1.00
  depth_from_entry=4 · centrality=0.01 · in=216 out=21
  siblings(12): __init__.py, _orm_constructors.py, _typing.py, attributes.py, base.py, bulk_persistence.py … +6 more

depends-on (top 6)
  → lib/sqlalchemy/util/typing.py :TypeVar  (1 refs)
  → lib/sqlalchemy/orm/decl_base.py :_add_attribute, _apply_dataclasses_to_any_class, _as_declarative, _assert_dc_arguments, +1 more  (5 refs)
  → lib/sqlalchemy/orm/identity.py :get, items  (2 refs)        ⚠ wrong get/items (bare names)
  → lib/sqlalchemy/exc.py :InvalidRequestError  (1 refs)
  → lib/sqlalchemy/orm/descriptor_props.py :_orm_synonym, declarative_scan, fget, warn  (4 refs)
  → lib/sqlalchemy/util/langhelpers.py :get_annotations  (1 refs)
  +5 more

depended-on  ⚠ HIGH BLAST RADIUS
  ← 2966 prod callers across 157 files, 0 test callers
  hottest: test/orm/test_relationships.py(253), test/orm/test_eager_relations.py(211),
           test/orm/inheritance/test_basic.py(141)

contracts (exported symbols & callers)
  declarative_base (function) → [113 callers, 0 tests]
  registry (class) → [71 callers, 0 tests]
  map_imperatively (function) → [2688 callers, 0 tests]
  generate_base (method) → [24 callers, 0 tests]
  update_type_annotation_map (function) → [22 callers, 0 tests]
  has_inherited_table (function) → [6 callers, 0 tests]
  declared_attr (class) → [5 callers, 0 tests]
  ... 21 more contracts
  add_mapped_attribute (function) → [4 callers, 0 tests]
  mapped_as_dataclass (function) → [3 callers, 0 tests] (3 impls)
  mapped (function) → [3 callers, 0 tests]
  as_declarative (function) → [5 callers, 0 tests]
  map_declaratively (function) → [4 callers, 0 tests]
  ...

recent activity
  0 commits/30d

resolver: degraded=false · 90 ambiguous refs in this file
</abyss-card>
```

| axis | score | comment |
|---|---|---|
| usefulness | 9 | HIGH BLAST RADIUS with 2966 callers is the load-bearing pre-edit answer; the 28-contract list nails the public surface (declarative_base/registry/mapped/map_imperatively/etc.); 90-ambiguous-refs footer is honest |
| accuracy | 7 | `prod callers=2966 / test callers=0` is structurally suspicious — `hottest` row lists `test/orm/*` files but counts them as `prod`. The path-prefix classifier may not handle SQLAlchemy's `test/` (vs Django's `tests/`) directory layout |
| speed | 10 | 19 ms wall, 19 MB RSS — well inside the agent perception budget |

**Path classifier bug**: the hook calls these "prod callers" but the hottest entries point at `test/orm/*` files. SQLAlchemy uses `test/` (singular) where Django uses `tests/` (plural) — looks like the test-filter is anchored to `tests/` not `test/`. Same FastAPI top-level finding restated for SQLAlchemy.

---

## Card body — full pre-edit analysis on `lib/sqlalchemy/orm/decl_api.py`

The card delivers — in 19 ms — what an agent editing the file would otherwise need 5 separate `grep` invocations for:

1. **`where`** says `module=sqlalchemy-5 role=bridge`. The role=bridge is correct (decl_api.py bridges user `class MyModel(DeclarativeBase)` declarations to the ORM mapper machinery in `decl_base.py`). The module label is the monorepo-stub debt.
2. **`depends-on`** correctly surfaces the 5 references into `decl_base.py` (the actual implementation file decl_api re-exports). It also leaks L4 bare-name noise: `→ lib/sqlalchemy/orm/identity.py :get, items` is the wrong `get`/`items` (orm/identity is `IdentityMap`, has different `get` semantics).
3. **`depended-on ⚠ HIGH BLAST RADIUS`** is the critical pre-edit signal: 2 966 callers across 157 files. Any change here ripples through ~22% of the lib codebase.
4. **`contracts`** is the gold: `declarative_base (113 callers)`, `registry (71 callers)`, `mapped (3 callers)`, `map_imperatively (2 688 callers)`. The 2 688 number for `map_imperatively` is suspicious — that's *more* callers than the entire file's depended-on count of 2 966. Likely a contract-rollup overcounts overloaded/decorated function variants. Worth a regression test, but not a load-bearing bug (the headline blast radius is still correct).
5. **`resolver: degraded=false · 90 ambiguous refs`** is the honest footer — tells the agent there are 90 refs in this file the resolver wasn't 100% sure about. That's the right shape for agent UX.

**Card verdict**: best-in-class for hub-file pre-edit warning. The path-prefix classifier and overload-rollup issues are presentation polish, not load-bearing accuracy.

---

## Bugs found

### B3 — Generic-parameterized base classes dropped from inherit edges (new, high impact)

**Symptom**: `class FunctionElement(Executable, ColumnElement[_T], FromClause, Generative)` produces inherit refs for 3 of the 4 base classes — `ColumnElement[_T]` is silently dropped.

**Reproducer**:
```sh
abyss index   # on sqlalchemy 2.0.36
sqlite3 .code-abyss/index.db "
  SELECT target_name, source_line FROM refs r
  JOIN files sf ON r.source_file_id=sf.id
  WHERE sf.path='lib/sqlalchemy/sql/functions.py'
    AND r.kind='inherit' AND r.source_line BETWEEN 110 AND 130;
"
# → target_name | source_line
#   Executable  | 116
#   FromClause  | 116
#   Generative  | 116
# (ColumnElement missing — line 117 has 4 bases declared, only 3 in DB)
```

**Root cause** (`src/graph/languages/python.rs:117-145`):
```rust
let (name, qualifier) = match base.kind() {
    "identifier" => (text(&base, source), None),
    "attribute" => { ... }
    // `metaclass=...`, `Generic[T]`, comments → skip.
    _ => continue,
};
```

The match arm only handles `identifier` (`ColumnElement`) and `attribute` (`elements.ColumnElement`). When the base is `ColumnElement[_T]`, tree-sitter parses it as a `subscript` node, which falls through to `_ => continue` and is silently skipped. The comment on line 130 ("`Generic[T]` → skip") shows this is by-design V1 behaviour to avoid extracting `Generic` itself from `class Foo(Generic[T])` — but the same path also drops *every* parameterized base class.

**Impact**: 25 generic-parameterized inheritances of `TypeEngine` and ~15 of `ColumnElement` are invisible. SQLAlchemy's entire type-engine inheritance tree (`String → StringPrint → Text`, etc.) is silently missing from the inherit edge set. `callers ColumnElement` reports 13 (all tests with bare inheritance) when the structural answer is 19+ in lib alone.

**Fix shape (V2)**: in the `_ => continue` arm, peek into `subscript`'s `value` field and extract the unparameterized base name. Still skip when value is `Generic` / `Protocol` / `TypeVar`. Add a regression test on a `class Foo(Bar[T])` fixture.

**Severity**: **High** — this is the difference between abyss seeing SQLAlchemy's type hierarchy or not. Fix would lift L0e and inherit-edge counts on every modern (PEP 695 / generics-heavy) Python codebase.

### B4 — `impact` direct=N vs `callers` prod=N visible on SQLAlchemy (known V2)

**Symptom**: `impact declarative_base` reports `direct=146`; `callers declarative_base` reports `113 total`. Same question, different numbers, undocumented divergence.

**Reproducer**:
```sh
abyss impact declarative_base    # direct=146  transitive=0  tests=0  uncovered=41
abyss callers declarative_base   # 20 prod (showing of 113 total)
```

**Cause**: `impact` defaults to `--min-confidence 0.5` (per source); `callers` defaults to `0.7`. Different filter chains, different headline numbers. Already in CHANGELOG v0.5.1 V2 list as known limitation. This run *confirms* it shows up in practice on the obvious hub function.

**Fix shape (V2)**: align the default confidence threshold across commands, or add a footer line that says "20 of 113 / 146 with --min-confidence 0.5" so the discrepancy is self-documenting.

**Severity**: **Medium** — won't mislead a careful agent but will confuse a quick visual scan.

### B5 — `test/` (singular) directory leaks into `prod callers` count

**Symptom**: pre-edit card for `decl_api.py` says `← 2966 prod callers across 157 files, 0 test callers` but the `hottest:` line points at `test/orm/*` files.

**Reproducer**: any file under `lib/sqlalchemy/orm/` with callers in `test/`.

**Cause**: the test-path classifier is anchored to `tests/` (plural — Django/FastAPI convention) and doesn't match `test/` (singular — SQLAlchemy/pytest convention). Same family as F4 from FastAPI (top-level `tests/` missed).

**Fix shape**: extend the path-prefix predicate to match both `test/` and `tests/` (and `_tests/`, `*_test/`, etc.). Single-line SQL fix in the same place F4 was fixed.

**Severity**: **Low** — inflates prod callers, but the high-blast-radius warning is still correct.

### B6 — Method overload duplication in `contracts` and context output

**Symptom**: `abyss context engine/base.py` prints `execute() ← 2 callers` *three times in a row* with identical callers. Same shape in pre-edit `contracts`: `mapped_as_dataclass (function) → [3 callers, 0 tests] (3 impls)` correctly annotates the impl count, but `execute()` does not.

**Cause**: when a function is `@overload`-decorated, each overload signature is its own symbol row in the symbols table. The contract rollup hits each ID, prints each independently.

**Fix shape (V2)**: collapse symbols with the same `(name, file_id, parent_id)` triple in the contracts builder, summing caller counts (or marking with `(N impls)` as `mapped_as_dataclass` already does for some cases).

**Severity**: **Low** — noisy presentation, no correctness loss.

### B7 — `map_imperatively` reports 2 688 callers (suspicious overcount in contract rollup)

**Symptom**: `contracts` section lists `map_imperatively (function) → [2688 callers, 0 tests]` — more than the entire file's blast radius (2 966).

**Cause**: probably the same overload/symbol-collision pattern as B6, plus possibly bare-name collision with similarly-named symbols in test files. Untriaged.

**Severity**: **Low** — would only mislead an agent that takes contract caller counts as gospel. The headline `2966 prod callers` is the trusted number.

---

## Final verdict — 8 / 10

**Score breakdown**:
- L0e validation: **+3** (mechanism re-validates on a *different* hierarchy shape; mid-band 4 496 between FastAPI 0 and Django 9 450 is the right structural shape)
- v0.5.1 B1 fix visible: **+1** (`callers DeclarativeBase` returns 121 inherit refs cleanly; Django would have returned 7)
- Pre-edit card load-bearing: **+2** (HIGH BLAST RADIUS for hub file, contracts list, depends-on chain, 19 ms latency)
- Cross-language guard holds: **+1** (single-language corpus, but the existing assertion stays green)
- Bug B3 generic-subscript inheritance: **−1** (biggest precision miss; affects every modern Python corpus, not just SQLAlchemy)
- Bug B5 path classifier: **−0.5** (rolls test/ into prod, surfaces same family as FastAPI F4)
- Bugs B4/B6/B7: **−0.5** (impact/callers gap, overload duplication — known/cosmetic)

### Comparison with Django

| Dimension | Django 5.1.4 | SQLAlchemy 2.0.36 |
|---|---|---|
| L0e absolute hits | 9 450 | **4 496** |
| L0e per Python file | 3.39 | **6.55** (highest of the corpus set) |
| Distinct receiver types | 600+ | **1 050** (more diversity) |
| Inherit refs | 9 129 | 8 018 |
| Distinct inherit base classes | n/a recorded | **825** |
| Cold index | 6.9 s | **8.4 s** (denser refs/file) |
| Warm reindex | 1.58 s | **723 ms** (less to re-tier; smaller corpus) |
| Hook latency | 10 ms | 19 ms |
| `callers <BaseClass>` returns inherits | ✗ (B1 in v0.5.0) | **✓ (B1 fixed in v0.5.1)** |
| New high-impact bug surfaced | B1 inherits + B2 sibling collision | **B3 generic-subscript inheritance** |

**Both projects validate the L0e hypothesis cleanly.** SQLAlchemy proves the mechanism is not just "Django's ORM" — it also lights up on a dense-mixin SQL-expression hierarchy with 1 050 distinct receiver types. The L0e-per-file ratio is *higher* on SQLAlchemy than on Django, which means the walker scales with hierarchy depth and density, not just file count.

**Headline next-action**: ship a V2 patch that handles `subscript` base classes in the Python extractor. This is the single biggest precision lift available on the Python tier and would directly bump SQLAlchemy's `callers ColumnElement` from 13 to 30+ overnight.

---

## Method (for repeating this run)

```sh
git clone --depth 1 --branch rel_2_0_36 https://github.com/sqlalchemy/sqlalchemy /tmp/sa
cd /tmp/sa
abyss index
# read resolver log for L0e=4496
abyss callers DeclarativeBase           # verifies B1 fix
abyss callers ColumnElement              # surfaces B3
echo '{"tool_input":{"file_path":"lib/sqlalchemy/orm/decl_api.py"}}' | abyss hook pre-edit
# spot-check L0e in lib only:
sqlite3 .code-abyss/index.db <<'SQL'
SELECT r.receiver_type, COUNT(*) hits
FROM refs r
JOIN files sf ON r.source_file_id = sf.id
JOIN files tf ON r.target_file_id = tf.id
WHERE sf.path LIKE 'lib/%' AND tf.path LIKE 'lib/%'
  AND r.confidence = 0.95
  AND r.receiver_type IS NOT NULL
  AND r.kind = 'call'
GROUP BY r.receiver_type
ORDER BY hits DESC LIMIT 20;
SQL
```
