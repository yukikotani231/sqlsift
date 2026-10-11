#!/usr/bin/env python3
"""Summarize corpus results (run.py, mutate.py) as Markdown tables.

Usage: python3 tests/corpus/report.py > out/report-tables.md

Every diagnostic on the unmodified corpus is a suspected false positive, since the
projects run in production. `triage()` sorts them into classes by rules written after
looking at each class by hand (see the corpus report):

  fp       sqlsift is wrong (a false positive)
  bug      the SQL is really broken
  schema   the schema given to sqlsift is incomplete (expected without full DDL)
  harness  an artifact of how this harness builds the schema
"""

import json
import re
from collections import Counter, defaultdict

from corpus import OUT, load_manifest
from run import RESULTS

MUT = OUT / "mutations"

# (class id, verdict, predicate(project, dialect, diagnostic, source line))
RULES = [
    ("dbt-type-macro-arith", "fp", lambda p, dl, d, line: "___dbt_type" in d["message"]),
    ("dbt-loop-alias", "fp", lambda p, dl, d, line: p["kind"] == "dbt" and "Table or alias" in d["message"]),
    ("bq-struct-star", "fp", lambda p, dl, d, line: dl == "bigquery" and "Table or alias" in d["message"]
     and re.search(re.escape(re.findall(r"'([^']*)'", d["message"])[0]) + r"\.\*", line or "")),
    ("bq-unpivot", "fp", lambda p, dl, d, line: dl == "bigquery" and d["code"] == "E0002" and "conversions_v1" in d["file"]),
    ("harness-bq-project", "harness", lambda p, dl, d, line: "google_ads_campaign_cost_breakdowns" in d["file"]),
    ("harness-include-field", "harness", lambda p, dl, d, line: "apple_logical_subscriptions_history" in d["file"]),
    ("bq-using-unaliased-subquery", "fp", lambda p, dl, d, line: dl == "bigquery" and "Set operation column count" in d["message"]),
    ("bq-script-variables", "fp", lambda p, dl, d, line: dl == "bigquery" and d["code"] == "E0002" and "storage_billing_model" in d["file"]),
    ("bq-groupby-alias", "fp", lambda p, dl, d, line: dl == "bigquery" and d["code"] == "E0006"),
    ("sf-recursive-cte", "fp", lambda p, dl, d, line: dl == "snowflake" and "sensitive_data_movements" in d["message"]),
    ("bug-missing-alias", "bug", lambda p, dl, d, line: "IMPL-IS-18" in d["file"]),
    ("sf-procedure-temp-table", "schema", lambda p, dl, d, line: "Table 'output'" in d["message"]),
    ("rs-datepart-sysdate", "fp", lambda p, dl, d, line: dl == "redshift" and d["code"] == "E0002"),
    ("rs-ddl-attributes", "fp", lambda p, dl, d, line: p["name"] == "amazon-redshift-utils-tpc" and d["code"] == "E0001"),
    ("schema-missing", "schema", lambda p, dl, d, line: d["code"] == "E0001" and d["message"].startswith("Table '")),
]


def triage(p, dialect, d, line):
    for cls, verdict, pred in RULES:
        try:
            if pred(p, dialect, d, line):
                return cls, verdict
        except IndexError:
            continue
    return "untriaged", "untriaged"


def source_line(p, mode, dialect, f, n):
    from show import source_root

    try:
        return (source_root(p, mode, dialect) / f).read_text(errors="replace").splitlines()[n - 1]
    except (OSError, IndexError):
        return ""


def load(p, mode, dialect):
    f = RESULTS / p["name"] / mode / f"{dialect}.json"
    return json.loads(f.read_text()) if f.exists() else None


def pct(a, b):
    return f"{100 * a / b:.0f}%" if b else "-"


def main():
    manifest = load_manifest()
    lock = json.loads((OUT / "lock.json").read_text()) if (OUT / "lock.json").exists() else {}
    projects = manifest["project"]
    classes = Counter()
    class_projects = defaultdict(set)
    class_dialects = defaultdict(set)
    untriaged = []

    print("## Corpus\n")
    print("| project | warehouse SQL | kind | schema | license | commit |")
    print("|---|---|---|---|---|---|")
    for p in projects:
        sha = lock.get(p.get("clone", p["name"]), p.get("sha", ""))[:10]
        print(f"| [{p['name']}]({p['url']}) | {', '.join(p['dialects'])} | {p['kind']} | {p['schema']} | {p['license']} | `{sha}` |")

    print("\n## False positives per project\n")
    print("Primary dialect = first listed. `parsed` = files without E1000. `diag` = diagnostics other than E1000. "
          "Verdicts: fp = sqlsift false positive, bug = genuinely broken SQL, schema = incomplete schema, harness = artifact of this harness.\n")
    print("| project | mode | dialect | files | parsed | diag | fp | bug | schema | harness |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    per_dialect = defaultdict(Counter)
    for p in projects:
        for mode in ("raw", "catalog", "compiled"):
            for dialect in p["dialects"]:
                res = load(p, mode, dialect)
                if res is None:
                    continue
                files = len(res)
                parsed = sum(1 for ds in res.values() if not any(d["code"] == "E1000" for d in ds))
                verdicts = Counter()
                for f, ds in res.items():
                    for d in ds:
                        if d["code"] == "E1000":
                            continue
                        d = dict(d, file=f)
                        cls, verdict = triage(p, dialect, d, source_line(p, mode, dialect, f, d["line"]))
                        verdicts[verdict] += 1
                        if verdict == "untriaged":
                            untriaged.append((p["name"], mode, dialect, f, d["line"], d["message"]))
                        if mode != "compiled" and (mode == "catalog" or p["schema"] != "dbt-run"):
                            classes[cls] += 1
                            class_projects[cls].add(p["name"])
                            class_dialects[cls].add(dialect)
                diag = sum(verdicts.values())
                key = "catalog" if mode == "catalog" else mode
                if mode in ("catalog",) or (mode == "raw" and p["schema"] != "dbt-run") or mode == "compiled":
                    pd = per_dialect[(dialect, "dbt" if p["kind"] == "dbt" else "sql")]
                    pd.update(files=files, parsed=parsed, diag=diag, **verdicts)
                if dialect == p["dialects"][0] or mode == "catalog" and diag:
                    print(f"| {p['name']} | {key} | {dialect} | {files} | {parsed} ({pct(parsed, files)}) | {diag} | "
                          f"{verdicts['fp']} | {verdicts['bug']} | {verdicts['schema']} | {verdicts['harness']} |")

    print("\n## Per dialect\n")
    print("dbt projects: `catalog` mode (`--dbt-catalog`) plus PostgreSQL `compiled` mode; plain SQL projects: `raw`.\n")
    print("| dialect | corpus | files | parsed | diag | fp | bug | schema | harness |")
    print("|---|---|---|---|---|---|---|---|---|")
    for (dialect, kind), c in sorted(per_dialect.items()):
        print(f"| {dialect} | {kind} | {c['files']} | {c['parsed']} ({pct(c['parsed'], c['files'])}) | {c['diag']} | "
              f"{c['fp']} | {c['bug']} | {c['schema']} | {c['harness']} |")

    print("\n## Diagnostic classes (raw/catalog runs, all dialects)\n")
    print("| class | verdict | diagnostics | dialects | projects |")
    print("|---|---|---|---|---|")
    verdict_of = {cls: v for cls, v, _ in RULES}
    for cls, n in classes.most_common():
        print(f"| {cls} | {verdict_of.get(cls, 'untriaged')} | {n} | {', '.join(sorted(class_dialects[cls]))} | "
              f"{', '.join(sorted(class_projects[cls]))} |")
    if untriaged:
        print("\nUntriaged:\n")
        for u in untriaged[:50]:
            print(f"- {u}")

    mutation_tables()


def mutation_tables():
    files = sorted(MUT.glob("*.json"))
    if not files:
        return
    print("\n## Mutation testing (false negatives)\n")
    print("Only mutants PostgreSQL rejects (`EXPLAIN` of the compiled mutant fails) count. "
          "`compiled` = sqlsift on dbt's compiled SQL (PostgreSQL); `catalog` = the Jinja model with "
          "`--dbt-catalog`; `raw` = the Jinja model with the sources as DDL and refs unknown.\n")
    print("| project | mutants | real bugs | compiled | catalog (pg) | raw (pg) |")
    print("|---|---|---|---|---|---|")
    kinds = defaultdict(Counter)
    dialects = defaultdict(Counter)
    reasons = Counter()
    total = Counter()
    for f in files:
        recs = json.loads(f.read_text())
        bugs = [r for r in recs if r.get("pg_error")]
        c = Counter(
            compiled=sum(bool(r.get("compiled_pg")) for r in bugs),
            catalog=sum(bool(r.get("catalog", {}).get("postgresql")) for r in bugs),
            raw=sum(bool(r.get("raw", {}).get("postgresql")) for r in bugs),
        )
        total.update(mutants=len(recs), bugs=len(bugs), **c)
        print(f"| {f.stem} | {len(recs)} | {len(bugs)} | {c['compiled']} ({pct(c['compiled'], len(bugs))}) | "
              f"{c['catalog']} ({pct(c['catalog'], len(bugs))}) | {c['raw']} ({pct(c['raw'], len(bugs))}) |")
        orig_cat = load({"name": f.stem}, "catalog", "postgresql") or {}
        for r in bugs:
            k = kinds[r["kind"]]
            k["bugs"] += 1
            k["compiled"] += bool(r.get("compiled_pg"))
            k["catalog"] += bool(r.get("catalog", {}).get("postgresql"))
            k["raw"] += bool(r.get("raw", {}).get("postgresql"))
            for dialect, found in r.get("catalog", {}).items():
                dialects[dialect]["bugs"] += 1
                dialects[dialect]["catalog"] += bool(found)
                dialects[dialect]["raw"] += bool(r.get("raw", {}).get(dialect))
            # why the catalog mode missed it
            if not r.get("catalog", {}).get("postgresql"):
                parse_fail = any(d["code"] == "E1000" for d in orig_cat.get(r["model"], []))
                if parse_fail:
                    reasons["model does not parse (template macro)"] += 1
                elif r.get("compiled_pg"):
                    reasons["found in compiled SQL, not in template (unknown: var()/macro relation, templated name)"] += 1
                else:
                    reasons[f"missed in compiled SQL too ({r['kind']})"] += 1
    b = total["bugs"]
    print(f"| **total** | {total['mutants']} | {b} | {total['compiled']} ({pct(total['compiled'], b)}) | "
          f"{total['catalog']} ({pct(total['catalog'], b)}) | {total['raw']} ({pct(total['raw'], b)}) |")
    print("\n| mutation | real bugs | compiled | catalog (pg) | raw (pg) |")
    print("|---|---|---|---|---|")
    for kind, c in sorted(kinds.items()):
        print(f"| {kind} | {c['bugs']} | {c['compiled']} ({pct(c['compiled'], c['bugs'])}) | "
              f"{c['catalog']} ({pct(c['catalog'], c['bugs'])}) | {c['raw']} ({pct(c['raw'], c['bugs'])}) |")
    print("\n| dialect | real bugs | catalog | raw |")
    print("|---|---|---|---|")
    for dialect, c in sorted(dialects.items()):
        print(f"| {dialect} | {c['bugs']} | {c['catalog']} ({pct(c['catalog'], c['bugs'])}) | {c['raw']} ({pct(c['raw'], c['bugs'])}) |")
    print("\nWhy `catalog` (PostgreSQL) missed a real bug:\n")
    print("| reason | mutants |")
    print("|---|---|")
    for reason, n in reasons.most_common():
        print(f"| {reason} | {n} |")


if __name__ == "__main__":
    main()
