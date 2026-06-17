# Notes: Django MRO validation (2026-06-17)

L0e fired **0 times** on FastAPI; the prediction was falsified. The
next-corpus prediction was Django — `Model` / `View` / `ModelAdmin` /
`Form` / `SimpleTestCase` hierarchies all live in-repo. **L0e fired
9 450 times on Django 5.1.4 — 94× the floor prediction.** Mechanism
validated; FastAPI was the wrong baseline.

Full note:

{{#include ../../../eval/notes/django-mro-validation-2026-06-17.md}}
