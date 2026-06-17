# SQLAlchemy eval: L0e MRO walker re-validated at 4 496 hits (mixin/generic shape)

**Date:** 2026-06-17 (post-v0.5.1)
**Status:** Hypothesis confirmed. Mechanism re-validates on a second
in-repo hierarchy shape. Full dogfood writeup:
[docs/dogfood/sqlalchemy-2026-06-17.md](../../docs/dogfood/sqlalchemy-2026-06-17.md).

## TL;DR

The Django MRO note
([django-mro-validation-2026-06-17.md](django-mro-validation-2026-06-17.md))
closed §3 with: *"The next falsification candidate is SQLAlchemy —
declarative base class hierarchies (DeclarativeBase → user models →
query mixins) that live across files in sqlalchemy/orm/. We expect
L0e ≥ 500 there. If it lands <100, the diagnosis is no longer 'wrong
corpus' — it's 'the walker doesn't handle declarative metaclass
hierarchies', which is a real V2 item."*

**SQLAlchemy 2.0.36 fired L0e 4 496 times — 9× the floor.** The
mechanism handles dense-mixin SQL-expression hierarchies as well as
it handles wide-tree ORM hierarchies. The L0e-per-Python-file ratio
on SQLAlchemy (6.55) is actually *higher* than on Django (3.39),
proving the walker scales with hierarchy depth and density rather
than file count.

## Hypothesis

From Django note §3 (2026-06-17):

> The next falsification candidate is SQLAlchemy. We expect L0e ≥ 500.

Conservative floor — high enough to call the mechanism load-bearing
on a second in-repo corpus, low enough that a 0-hit shape (FastAPI
external-base style) would still falsify cleanly.

## Actual

SQLAlchemy 2.0.36 (rel_2_0_36 tag) cold index — 687 Python files
(255 `lib/`, 350 `test/`, 71 `examples/`), 37 210 symbols, 200 314
refs:

```
L0(receiver-type)=7589
L0c(type-binding)=64
L0d(type-file)=853
L0e(py-mro)=4496      ← the headline number
L0b(import-binding)=60291
L1(same-file)=14114
L2(same-pkg-unique)=6952
L3(qualifier)=3947
L4(global-unique)=10184
L4a(same-file-qual)=3598
L4b(same-pkg-multi)=1699
L5(ambiguous)=47192
```

| Slice | Hits | Comment |
|---|---:|---|
| L0e total (resolver log) | **4 496** | 9× over the ≥500 floor |
| Total receiver-tier (L0/L0c/L0d/L0e at conf=0.95) | 13 002 | broader denominator |
| Strict cross-file MRO walks (receiver class not in source file) | 2 922 | tightest proxy |
| Distinct receiver-type classes feeding receiver tiers | **1 050** | more diversity than Django (~600) |
| Inherit refs total | 8 018 | 825 distinct base classes |
| Inherit refs at conf ≥ 0.7 | 6 786 | |

## 5 sample L0e resolutions

| Source | Receiver.method | Resolved target | Hierarchy walked |
|---|---|---|---|
| `lib/sqlalchemy/engine/reflection.py:336` | `Connection.close` | `lib/sqlalchemy/engine/base.py` | `Connection → ConnectionEventsTarget` |
| `lib/sqlalchemy/dialects/sqlite/base.py:1492` | `BindParameter._clone` | `lib/sqlalchemy/sql/elements.py` | `BindParameter → ColumnElement → ClauseElement` |
| `lib/sqlalchemy/sql/compiler.py:4012` | `CTE._get_reference_cte` | `lib/sqlalchemy/sql/selectable.py` | `CTE → Generative → DialectKWArgs → HasCacheKey` |
| `lib/sqlalchemy/orm/strategies.py:3208` | `Bundle.__clause_element__` | `lib/sqlalchemy/orm/util.py` | `Bundle → ORMColumnsClauseRole` |
| `lib/sqlalchemy/orm/relationships.py:3363` | `ClauseAdapter.chain` | `lib/sqlalchemy/sql/util.py` | `ClauseAdapter → ClauseVisitor` |

All five are textbook wins — exactly the resolutions an agent reading
`compiler.py` or `relationships.py` would otherwise `grep -rn "def
_clone" lib/` to find. The third example (`CTE._get_reference_cte`)
walks through two intermediate mixin levels — the walker handles
SQLAlchemy's mixin-and-multiple-inheritance pattern cleanly.

### Top production-only L0e receiver types

| Receiver type | Recv-tier hits | In-repo base chain |
|---|---:|---|
| `SQLCompiler` | 207 | → `Compiled` (sql/compiler.py is hub) |
| `Table` | 95 | → `TableClause` → `Selectable` |
| `OracleDialect` | 84 | → `DefaultDialect` (engine/default.py) |
| `Mapper` | 81 | → `_MapperEntity` / `InspectionAttr` |
| `Session` | 72 | → `_SessionClassMethods` |
| `PGDialect` | 71 | → `DefaultDialect` |
| `Connection` | 66 | → `ConnectionEventsTarget` (engine/base.py) |
| `Query` | 61 | → `Generative` (orm/query.py) |
| `ColumnOperators` | 59 | → `Operators` (sql/operators.py) |
| `Inspector` | 48 | → `inspection.Inspectable` |

## click vs FastAPI vs Django vs SQLAlchemy

| Project | Python files | L0e hits | L0e per .py file | Why |
|---|---:|---:|---:|---|
| FastAPI 0.115.4 | 1 281 | **0** | 0.00 | external bases (Starlette/BaseModel/Enum) |
| click 8.1.8 | ~30 | **2** | 0.07 | single-file co-located hierarchies |
| **SQLAlchemy 2.0.36** | **686** | **4 496** | **6.55** | dense in-repo mixin towers |
| Django 5.1.4 | 2 786 | **9 450** | 3.39 | wide ORM/CBV/Admin hierarchies |

**SQLAlchemy has the highest L0e-per-file ratio in the corpus set.**
SQLAlchemy is denser (291 refs/file vs Django's 69 vs FastAPI's 39),
which means the walker scales with hierarchy depth and ref density
rather than file count alone.

## Lesson

Two in-repo corpus shapes now validate L0e:

1. **Wide-tree** (Django) — `Model` → `ModelAdmin` / `View` / `Form` /
   `TestCase` with 600+ distinct receivers and 9 450 hits across
   2 786 files.
2. **Deep-mixin** (SQLAlchemy) — `Compiled` → `SQLCompiler` → dialect
   subclasses with 1 050 distinct receivers and 4 496 hits across 686
   files.

Both share the corpus-shape predicate from the Django note: *in-repo
base classes with cross-file split between base and subclass*. When
either condition fails (FastAPI external roots; click single-file
colocation), L0e fires zero or near-zero. PRINCIPLES §2 prediction
shape held across all four corpora.

Pragmatic upshot:

1. **L0e stays shipped at 0.95 confidence.** Two independent corpus
   shapes confirm.
2. **New high-impact V2 item: generic-parameterized base classes
   dropped from inherit edges.** `class FunctionElement(Executable,
   ColumnElement[_T], FromClause, Generative)` produces 3 inherit
   refs instead of 4. The Python extractor
   (`src/graph/languages/python.rs:117-145`) only handles
   `identifier` and `attribute` base nodes; `subscript` nodes are
   skipped to avoid extracting `Generic` from `class Foo(Generic[T])`
   but the broad skip drops every parameterized base. **Fix shape**:
   in the `_ => continue` arm, peek into `subscript`'s `value` field
   and extract the unparameterized base name; still skip when value
   is `Generic` / `Protocol` / `TypeVar`. Bumps `callers
   ColumnElement` from 13 (test-only) to 30+ (lib+test). This is the
   single biggest precision lift available on the Python tier.
3. **v0.5.1's B1 fix re-validated** — SQLAlchemy `callers
   DeclarativeBase` returns 121 inherit refs cleanly at 95%
   confidence. Django's `callers Model` under v0.5.0 returned 7
   instantiation hits; the same surface now lifts.

## Decisions

1. **L0e stays shipped at 0.95 confidence.** Two independent corpus
   shapes confirm.
2. **B3 (generic-subscript inheritance) filed as next-release
   precision item.** Highest single lift available on Python.
3. **SQLAlchemy added to dogfood index** as the dense-mixin reference
   case. See [docs/dogfood/sqlalchemy-2026-06-17.md](../../docs/dogfood/sqlalchemy-2026-06-17.md)
   and [docs/DOGFOOD.md](../../docs/DOGFOOD.md).
4. **Next candidate corpus**: TBD. PyTorch (deep nn.Module subclass
   trees) and Trio (async-mixin patterns) are both predicted high. We
   could also re-run FastAPI *with* Starlette and Pydantic vendored
   to verify the falsifier reverts to a hit count when bases are
   pulled in-repo.

## Method (for repeating this run)

```sh
git clone --depth 1 --branch rel_2_0_36 https://github.com/sqlalchemy/sqlalchemy /tmp/sa
cd /tmp/sa
abyss index   # ~8.4 s cold on this host
# read the resolver log for L0e=N
abyss stats
abyss callers DeclarativeBase   # → 121 inherit refs at 95%
abyss callers ColumnElement     # → 13 (would be 30+ with B3 fixed)
# spot-check lib-only L0e:
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

The dogfood writeup spells out the per-tier counts, the full
4-corpus comparison, and the B3/B4/B5/B6/B7 reproducers.
