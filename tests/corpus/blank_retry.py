#!/usr/bin/env python3
"""Estimate the parse rate of dbt models if sqlsift retried a parse error at a Jinja tag
with that tag blanked (macros that expand to clauses or column lists, `{{ config() }}`
after a comment).

Usage: python3 tests/corpus/blank_retry.py [NAME ...]

For each dbt project, files with a parse error whose `found:` token is a Jinja
expression get that tag replaced by spaces, and are checked again (up to 8 rounds).
Prints files / parsed before / parsed after, and the diagnostics that appear in the
recovered files (they would be new false positives or true positives).
"""

import json
import shutil
import sys
from collections import Counter

from corpus import OUT, jinja_spans, model_files, project_root, projects, run_sqlsift
from run import RESULTS, SCHEMAS


def offset(text, line, col):
    lines = text.split("\n")
    return sum(len(l) + 1 for l in lines[: line - 1]) + col - 1


def main():
    total = Counter()
    new_diags = Counter()
    for p in projects(sys.argv[1:]):
        if p["kind"] != "dbt":
            continue
        dialect = "postgresql" if "postgresql" in p["dialects"] else p["dialects"][0]
        mode = "catalog" if (RESULTS / p["name"] / "catalog" / f"{dialect}.json").exists() else "raw"
        res = json.loads((RESULTS / p["name"] / mode / f"{dialect}.json").read_text())
        root = project_root(p)
        work = OUT / "blank_retry" / p["name"]
        if work.exists():
            shutil.rmtree(work)
        texts = {f: (root / f).read_text(errors="replace") for f in model_files(p)}
        failing = {f for f, ds in res.items() if any(d["code"] == "E1000" for d in ds)}
        before = len(texts) - len(failing)
        cur = {f: res[f] for f in failing}
        extra = []
        schema = []
        if mode == "catalog":
            extra = ["--dbt-catalog", str(SCHEMAS / p["name"] / f"catalog-{dialect}.json")]
        elif (SCHEMAS / p["name"] / f"yml-{dialect}.sql").exists():
            schema = [SCHEMAS / p["name"] / f"yml-{dialect}.sql"]
        for _ in range(8):
            changed = []
            for f in list(cur):
                errs = [d for d in cur[f] if d["code"] == "E1000" and "Jinja" in d["message"]]
                if not errs:
                    continue
                d = errs[0]
                text = texts[f]
                pos = offset(text, d["line"], d["column"])
                for s, e in jinja_spans(text):
                    if s <= pos < e or s == pos:
                        text = text[:s] + "".join(c if c == "\n" else " " for c in text[s:e]) + text[e:]
                        break
                else:
                    continue
                texts[f] = text
                dest = work / f
                dest.parent.mkdir(parents=True, exist_ok=True)
                dest.write_text(text)
                changed.append(f)
            if not changed:
                break
            out = run_sqlsift(changed, schema, dialect, "jinja", cwd=work, extra=extra)
            cur.update(out)
        after = sum(1 for f in texts if not any(d["code"] == "E1000" for d in cur.get(f, res.get(f, []))))
        for f in failing:
            if not any(d["code"] == "E1000" for d in cur[f]):
                for d in cur[f]:
                    new_diags[(p["name"], f, d["code"], d["message"])] += 1
        print(f"{p['name']:24} {mode:8} files={len(texts):4} parsed before={before:4} after={after:4}")
        total.update(files=len(texts), before=before, after=after)
    print(f"TOTAL files={total['files']} parsed before={total['before']} after={total['after']}")
    for k, v in new_diags.items():
        print("new diagnostic in recovered file:", k)


if __name__ == "__main__":
    main()
