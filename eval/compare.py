#!/usr/bin/env python3
"""Compare abyss call-graph resolution against SCIP ground truth.

Method:
  1. SCIP gives, for each reference occurrence (file, line, symbol), the file
     where that symbol is defined (in-repo symbols only).
  2. abyss gives, for each extracted call ref (file, line, target_name), the
     file it resolved the target to, plus a confidence score.
  3. Join on (file, line, name). For every joined pair the prediction is
     correct iff abyss's target file == SCIP's definition file.

Metrics (per confidence tier and overall):
  precision = correct / predicted          (when abyss commits to an answer)
  recall    = correct / ground-truth pairs (how much of the known graph we get)

Usage: compare.py <repo_dir> [--json]
"""

import json
import sqlite3
import sys
from collections import defaultdict


def scip_symbol_name(symbol: str) -> str | None:
    """Extract the trailing identifier from a SCIP symbol string.

    e.g. '... `github.com/gin-gonic/gin`/Context#JSON().' -> 'JSON'
         '... `pkg`/New().'                               -> 'New'
         '... `pkg`/Engine#'                              -> 'Engine'
    """
    if symbol.startswith("local "):
        return None
    last = symbol.rstrip(".").split("/")[-1]
    # method on a type: Context#JSON()
    if "#" in last:
        last = last.split("#")[-1] or last.split("#")[0]
    if last.endswith("()"):
        last = last[:-2]
    last = last.strip("`.")
    return last or None


def load_scip(path: str):
    with open(path) as f:
        data = json.load(f)

    defs = {}  # symbol -> definition file (in-repo)
    refs = defaultdict(list)  # (file, line) -> [symbol]
    for doc in data.get("documents", []):
        rel = doc["relative_path"]
        for occ in doc.get("occurrences", []):
            sym = occ.get("symbol", "")
            if sym.startswith("local "):
                continue
            roles = occ.get("symbol_roles", 0)
            line = occ["range"][0]
            if roles & 1:  # definition
                defs[sym] = rel
            else:
                refs[(rel, line)].append(sym)
    return defs, refs


def load_abyss(db_path: str):
    conn = sqlite3.connect(db_path)
    rows = conn.execute(
        """SELECT sf.path, r.source_line, r.target_name, tf.path, r.confidence
           FROM refs r
           JOIN files sf ON r.source_file_id = sf.id
           LEFT JOIN files tf ON r.target_file_id = tf.id
           WHERE r.kind = 'call'"""
    ).fetchall()
    conn.close()
    return rows


def main():
    repo = sys.argv[1].rstrip("/")
    as_json = "--json" in sys.argv

    defs, scip_refs = load_scip(f"{repo}/scip.json")
    abyss_refs = load_abyss(f"{repo}/.code-abyss/index.db")

    tiers = [1.0, 0.95, 0.9, 0.8, 0.5]
    stats = {t: {"correct": 0, "wrong": 0} for t in tiers}
    truth_pairs = 0
    unresolved = 0
    no_ground_truth = 0

    for src, line, name, target, conf in abyss_refs:
        # Ground truth: a SCIP reference at the same location whose extracted
        # name matches and whose definition is in-repo.
        gt_file = None
        for sym in scip_refs.get((src, line), []):
            if scip_symbol_name(sym) == name and sym in defs:
                gt_file = defs[sym]
                break
        if gt_file is None:
            no_ground_truth += 1
            continue
        truth_pairs += 1
        if target is None or conf == 0.0:
            unresolved += 1
            continue
        tier = min(tiers, key=lambda t: abs(t - conf))
        if target == gt_file:
            stats[tier]["correct"] += 1
        else:
            stats[tier]["wrong"] += 1

    def agg(min_conf):
        c = sum(s["correct"] for t, s in stats.items() if t >= min_conf)
        w = sum(s["wrong"] for t, s in stats.items() if t >= min_conf)
        prec = c / (c + w) if c + w else 0.0
        rec = c / truth_pairs if truth_pairs else 0.0
        return c, w, prec, rec

    out = {
        "repo": repo.split("/")[-1],
        "ground_truth_pairs": truth_pairs,
        "abyss_call_refs_without_scip_truth": no_ground_truth,
        "unresolved": unresolved,
        "tiers": {
            str(t): {
                **s,
                "precision": round(s["correct"] / (s["correct"] + s["wrong"]), 4)
                if s["correct"] + s["wrong"]
                else None,
            }
            for t, s in stats.items()
        },
    }
    for label, mc in [("gated@0.7", 0.7), ("all", 0.0)]:
        c, w, p, r = agg(mc)
        out[label] = {
            "predicted": c + w,
            "correct": c,
            "precision": round(p, 4),
            "recall": round(r, 4),
        }

    if as_json:
        print(json.dumps(out, indent=2))
        return

    print(f"=== {out['repo']} — abyss vs SCIP ground truth ===")
    print(f"ground-truth call pairs: {truth_pairs}   unresolved by abyss: {unresolved}")
    print(f"{'tier':>6} {'correct':>8} {'wrong':>6} {'precision':>10}")
    for t in tiers:
        s = stats[t]
        tot = s["correct"] + s["wrong"]
        p = f"{s['correct'] / tot:.1%}" if tot else "—"
        print(f"{t:>6} {s['correct']:>8} {s['wrong']:>6} {p:>10}")
    for label in ["gated@0.7", "all"]:
        m = out[label]
        print(
            f"{label:>10}: precision {m['precision']:.1%}  recall {m['recall']:.1%}"
            f"  ({m['correct']}/{m['predicted']} predicted, {truth_pairs} truth)"
        )


if __name__ == "__main__":
    main()
