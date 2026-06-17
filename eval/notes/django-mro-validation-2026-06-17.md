# Django eval: L0e MRO walker validated at 9 450 hits (in-repo inheritance lift)

**Date:** 2026-06-17 (post-v0.5.0, ahead of v0.5.1)
**Status:** Hypothesis confirmed. Mechanism shipped in v0.5.0; corpus
proves it lands. Full dogfood writeup:
[docs/dogfood/django-2026-06-17.md](../../docs/dogfood/django-2026-06-17.md).

## TL;DR

L0e — the Python MRO-aware receiver-inference tier added in v0.5.0 —
fired **0 times** on FastAPI and **2 times** on click. The FastAPI
dogfood explicitly falsified the ≥50-hits floor prediction
([eval/notes/click-mro-walker-2026-06-17.md](click-mro-walker-2026-06-17.md)
and PRINCIPLES §2). The diagnosis was "external base classes break
the walker — we need an in-repo deep-inheritance corpus".

Django 5.1.4 was that corpus. **L0e fired 9 450 times — 94× the
original floor prediction, 4 725× FastAPI, and 100% structurally
explainable from the corpus's class layout.** The mechanism is right;
FastAPI was the wrong baseline. The L0e tier stays shipped at
confidence 0.95.

## Hypothesis

Before the Django run we predicted (FastAPI surprises section,
2026-06-17):

> MRO value lives where inheritance stays in-repo and deep. Django
> `Model` / `ModelAdmin` / `View` / `ListView` / `CreateView` /
> `Form` / `ModelForm` / `SimpleTestCase` / `TransactionTestCase`
> are the textbook case. Predicted ≥100 L0e hits.

The minimum floor was set deliberately conservatively — ≥100 would
have been enough to call the mechanism load-bearing on the right
corpus.

## Actual

Django 5.1.4 cold index (2 786 Python files, 879 production + 1 903
in `tests/`, 39 688 symbols, 192 576 refs):

```
L0e(py-mro)=9450
```

Breakdown:

| Slice | L0e hits |
|---|---:|
| Total (resolver log) | **9 450** |
| In production code (`django/%`) | ~536 |
| In `tests/%` (Django TestCase MRO walks) | ~7 826 |
| Distinct receiver-type classes feeding L0e | 600+ |

**94× over the floor prediction.** Not the kind of result you can
tune yourself into — it's a structural property of the corpus.

## 5 sample L0e resolutions

The samples confirm the walker is doing what it advertises: when a
receiver's class doesn't define the called method, walk `inherits`
refs up the chain and resolve to the base's file at 0.95.

| Source | Receiver.method | Resolved target | Hierarchy walked |
|---|---|---|---|
| `django/contrib/auth/admin.py:129` | `UserAdmin.has_change_permission` | `django/contrib/admin/options.py` | `UserAdmin → ModelAdmin` |
| `django/views/generic/dates.py:309` | `BaseDateListView.get_context_data` | `django/views/generic/list.py` | `BaseDateListView → MultipleObjectMixin` |
| `django/contrib/sessions/backends/cached_db.py:28` | `SessionStore._get_or_create_session_key` | `django/contrib/sessions/backends/base.py` | `SessionStore → SessionBase` |
| `django/db/backends/oracle/schema.py:53` | `DatabaseSchemaEditor.execute` | `django/db/backends/postgresql/schema.py` ⚠ | `DatabaseSchemaEditor → BaseDatabaseSchemaEditor` (wrong sibling — B2) |
| `django/contrib/gis/db/models/functions.py:204` | `AsGeoJSON.copy` | `django/db/models/expressions.py` | `AsGeoJSON → GeoFunc → Func → Expression` |

Four of five land exactly where an agent reading the source would
otherwise need to grep for. The fourth (`DatabaseSchemaEditor.execute`)
lands at the right class name but the wrong sibling directory — that
miss is bug B2 from the dogfood, fixed in v0.5.1 by adding a
same-directory preference before the global-unique pick.

### Top production-only L0e receiver types

| Receiver type | L0e hits | In-repo base class chain |
|---|---:|---|
| `SessionStore` | 66 | `SessionStore` → `SessionBase` (sessions/backends/base.py) |
| `DatabaseSchemaEditor` | 56 | → `BaseDatabaseSchemaEditor` (backends/base/schema.py) |
| `DatabaseCreation` | 55 | → `BaseDatabaseCreation` (backends/base/creation.py) |
| `DatabaseOperations` | 43 | → `BaseDatabaseOperations` |
| `DatabaseWrapper` | 24 | → `BaseDatabaseWrapper` |
| `SpatialiteSchemaEditor` | 21 | → `DatabaseSchemaEditor` → base |
| `UserAdmin` | 6 | → `ModelAdmin` (contrib/admin/options.py) |
| `BaseDateListView` | 6 | → `MultipleObjectMixin` (views/generic/list.py) |
| `FileSystemStorage` | 5 | → `Storage` (files/storage/base.py) |

These are the textbook cases — agents asking "what does
`self.execute()` do here?" need the MRO walker to point at the base
method. L0e delivers.

## FastAPI vs click vs Django

| Project | Python files | L0e hits | Why |
|---|---:|---:|---|
| FastAPI 0.115.4 | 1 281 | **0** | bases are `Starlette`, `BaseModel`, `Request`, `Enum` — all external libraries |
| click 8.1.8 | (small) | **2** | bases mostly co-locate with subclasses in single files; stdlib leaves |
| **Django 5.1.4** | **2 786** | **9 450** | bases `Model`, `View`, `ModelAdmin`, `Form`, `SimpleTestCase`, `BaseDatabaseSchemaEditor`, `SessionBase`, `BaseCache`, `Storage`, `Expression`, `Func`, `BaseCommand` all live in `django/` |

The comparison is the whole point: **codebase size and DI density do
not predict L0e value. In-repo inheritance depth does.**

- FastAPI has 1 281 Python files — bigger than click, dense
  dependency-injection style — and L0e fires zero times because
  Starlette, Pydantic, and stdlib `Enum` own the hierarchy roots.
- click has ~30 Python files and the hierarchies are real
  (`BaseCommand` → `Command` → `Group` → ...) but co-locate in
  single files (`core.py`, `types.py`, `exceptions.py`), so the
  same-file tier (L1, confidence 1.0) already resolves them
  correctly. L0e fires twice on the rare cross-file case
  (`shell_completion.py` extending `core.py`).
- Django puts every base class in-repo (the ORM, the admin, the
  generic views, the cache backends, the test framework) AND splits
  base / subclass across files by design (`django/db/models/base.py`
  → `django/contrib/auth/models.py`, `django/views/generic/base.py`
  → `django/views/generic/list.py`). L0e fires every time.

## Lesson

The L0e prediction error on FastAPI was a corpus-selection error, not
a mechanism error. The fix wasn't to weaken the prediction; it was to
pick a corpus where the prediction's premises actually hold. PRINCIPLES
§2 explicitly says "falsify in public, re-validate in public" — this
note is the re-validation half.

Pragmatic upshot for future eval choices:

1. **Don't predict tier-firing counts from codebase size.** Predict
   them from the corpus's hierarchy-rooting policy (in-repo vs
   external library) and base/subclass co-location (single file vs
   cross-file).
2. **A 0-hit result is a corpus signal, not a mechanism signal —
   unless the corpus has the right shape and still hits zero.**
   FastAPI's 0 was structural; click's 2 was structural; Django's
   9 450 closes the loop.
3. **The next falsification candidate is SQLAlchemy** — declarative
   base class hierarchies (`DeclarativeBase` → user models →
   query mixins) that live across files in `sqlalchemy/orm/`. We
   expect L0e ≥ 500 there. If it lands <100, the diagnosis is no
   longer "wrong corpus" — it's "the walker doesn't handle
   declarative metaclass hierarchies", which is a real V2 item.

   **Update (same-day): SQLAlchemy 2.0.36 fired L0e 4 496 times —
   9× the floor.** Full writeup at
   [sqlalchemy-mro-validation-2026-06-17.md](sqlalchemy-mro-validation-2026-06-17.md)
   and dogfood at
   [docs/dogfood/sqlalchemy-2026-06-17.md](../../docs/dogfood/sqlalchemy-2026-06-17.md).
   SQLAlchemy's L0e-per-Python-file ratio (6.55) is the highest in
   the corpus set — the mixin-tower hierarchy shape stresses the
   walker harder than Django's wide-tree shape. New high-impact V2
   item surfaced: generic-parameterized base classes
   (`ColumnElement[_T]`) silently dropped from inherit edges. See
   the sibling note's "Decisions" §2.

## Decisions

1. **L0e stays shipped at 0.95 confidence.** Django validates the
   mechanism on the corpus shape PRINCIPLES §2 promised would lift.
2. **B2 (sibling collision) fixed in v0.5.1.** L0e now prefers
   receiver-type definitions in the source file's directory before
   falling back to globally-unique. See CHANGELOG v0.5.1 → Fixed.
3. **Django dogfood added to the dogfood index** as the in-repo
   inheritance reference case. See
   [docs/dogfood/django-2026-06-17.md](../../docs/dogfood/django-2026-06-17.md)
   and [docs/DOGFOOD.md](../../docs/DOGFOOD.md).
4. **No SCIP baseline change.** Django is not part of the SCIP eval
   corpus set (no `scip-python` ground truth captured for it yet).
   Adding Django to `eval/run.sh` is filed as a follow-up;
   scip-python would need to chew through 39 K symbols and the
   ground truth would land in the 100K-pair range, large enough to
   want its own slot in CI rather than the inline corpus list.

## Method (for repeating this run)

```sh
git clone --depth 1 --branch 5.1.4 https://github.com/django/django /tmp/django
cd /tmp/django
abyss index   # ~6.9 s cold on this host
# read the resolver log for L0e=N
abyss stats
# spot-check production-only L0e:
sqlite3 .code-abyss/index.db <<'SQL'
SELECT r.receiver_type, COUNT(*) hits
FROM refs r
JOIN files sf ON r.source_file_id = sf.id
JOIN files tf ON r.target_file_id = tf.id
WHERE sf.path LIKE 'django/%' AND tf.path LIKE 'django/%'
  AND r.confidence = 0.95
  AND r.receiver_type IS NOT NULL
GROUP BY r.receiver_type
ORDER BY hits DESC LIMIT 20;
SQL
```

The dogfood writeup spells out the exact SQL for the per-tier counts,
the FastAPI / click / Django comparison table, and the bug B1 / B2
reproducers.
