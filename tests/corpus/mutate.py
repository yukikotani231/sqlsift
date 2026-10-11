#!/usr/bin/env python3
"""Mutation testing: inject realistic breakages into clean models, see what sqlsift finds.

Usage: python3 tests/corpus/mutate.py [NAME ...]

For each "dbt-run" project (after dbt_build.py and run.py), every model gets a few
mutants, made on the Jinja template outside template tags, strings and comments:

  column-typo   a column name of a relation the model reads is misspelled
  cte-typo      a reference to a CTE of the model is misspelled (wrong table)
  ambiguous     the model's first `from <relation> [alias]` gets a self cross join
                (`cross join <relation> as zz_dup`), so unqualified columns of that
                relation become ambiguous
  type          `<column> = DATE '2001-01-01' and ` is inserted after an ON/WHERE
                whose condition starts with a column

The mutants are added to the dbt project as extra models and compiled by dbt; then
PostgreSQL is the oracle: a mutant is a real bug when `EXPLAIN` of its compiled SQL
fails. sqlsift checks each mutant in the three modes of run.py (raw and resolved for
every dialect, compiled for PostgreSQL); a mutant is detected in a mode when sqlsift
reports a diagnostic on it that it doesn't report on the original model.

Writes out/mutations/<project>.json.
"""

import glob
import json
import os
import random
import re
import shutil
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

from corpus import OUT, WORK, jinja_spans, project_root, projects, run_sqlsift
from dbt_build import DBT, PG, dbname, dbt
from run import RESULTS, SCHEMAS, DbtProject, templating_of

MUT = OUT / "mutations"
PER_KIND = {"column-typo": 2, "cte-typo": 1, "ambiguous": 1, "type": 1}
KEYWORDS = set(
    """select from where and or not as on join left right full inner outer cross using
    group by order having limit offset union all distinct case when then else end is null
    in exists between like ilike with recursive over partition rows range unbounded
    preceding following current row asc desc nulls first last true false cast interval
    lateral natural window qualify filter within date timestamp time zone at values
    insert into update delete set returning any some extract coalesce if""".split()
)

TOKEN = re.compile(
    r"""(?P<comment>--[^\n]*|/\*.*?\*/)
      |(?P<string>'(?:[^']|'')*')
      |(?P<qident>"[^"]*"|`[^`]*`)
      |(?P<ident>[A-Za-z_][A-Za-z0-9_$]*)
      |(?P<num>\d+(?:\.\d+)?)
      |(?P<ws>\s+)
      |(?P<other>.)""",
    re.S | re.X,
)


def tokens(text):
    """[(kind, start, end, value)] of `text`, Jinja tags as kind 'jinja'."""
    out = []
    spans = jinja_spans(text)
    pos = 0
    for start, end in spans + [(len(text), len(text))]:
        for m in TOKEN.finditer(text, pos, start):
            out.append((m.lastgroup, m.start(), m.end(), m.group()))
        if start < len(text):
            out.append(("jinja", start, end, text[start:end]))
        pos = end
    return out


def significant(toks):
    return [t for t in toks if t[0] not in ("ws", "comment")]


def typo(name, avoid):
    for cand in (name[: len(name) // 2] + name[len(name) // 2 + 1 :], name + "x", name + "_zz"):
        if len(cand) >= 2 and cand.lower() not in avoid:
            return cand
    return name + "_zz"


def relation_end(sig, i):
    """Index of the last token of the FROM item starting at sig[i] (with its alias)."""
    j = i
    if j + 1 < len(sig) and sig[j + 1][3].lower() == "as" and j + 2 < len(sig) and sig[j + 2][0] == "ident":
        return j + 2
    if j + 1 < len(sig) and sig[j + 1][0] == "ident" and sig[j + 1][3].lower() not in KEYWORDS:
        return j + 1
    return j


def mutants(text, known_columns, rng):
    """[(kind, description, mutated text)] for a model's template text."""
    toks = tokens(text)
    sig = significant(toks)
    lower = [t[3].lower() for t in sig]
    out = defaultdict(list)
    ctes = set()
    for i, t in enumerate(sig):
        # `name as (` or `name (cols) as (` defines a CTE
        if t[0] == "ident" and i + 2 < len(sig) and lower[i + 1] == "as" and sig[i + 2][3] == "(":
            if i == 0 or lower[i - 1] in ("with", ",", "recursive"):
                ctes.add(t[3].lower())
    avoid = set(known_columns) | ctes | KEYWORDS
    for i, t in enumerate(sig):
        prev = lower[i - 1] if i else ""
        nxt = sig[i + 1][3] if i + 1 < len(sig) else ""
        if t[0] != "ident":
            continue
        name = t[3].lower()
        # column-typo: a known column used as a column (not an alias, function or qualifier)
        if (
            name in known_columns
            and name not in KEYWORDS
            and name not in ctes
            and prev not in ("as", "from", "join", "with", "table", "into")
            and nxt not in ("(", ".")
        ):
            new = typo(t[3], avoid)
            out["column-typo"].append((f"{t[3]} -> {new}", text[: t[1]] + new + text[t[2] :]))
        # cte-typo: `from cte` / `join cte`
        if name in ctes and prev in ("from", "join"):
            new = typo(t[3], avoid)
            out["cte-typo"].append((f"{t[3]} -> {new}", text[: t[1]] + new + text[t[2] :]))
    for i, t in enumerate(sig):
        # ambiguous: first from-item that is a template relation or a CTE
        if lower[i] == "from" and i + 1 < len(sig):
            item = sig[i + 1]
            if item[0] == "jinja" and re.search(r"\b(ref|source|var)\s*\(", item[3]) or (
                item[0] == "ident" and item[3].lower() in ctes
            ):
                end = sig[relation_end(sig, i + 1)][2]
                ins = f" cross join {item[3]} as zz_dup"
                out["ambiguous"].append((f"self join after {item[3][:40]}", text[:end] + ins + text[end:]))
        # type: `on <col>` / `where <col>`
        if lower[i] in ("on", "where") and i + 1 < len(sig) and sig[i + 1][0] == "ident":
            j = i + 1
            col = sig[j][3]
            if j + 2 < len(sig) and sig[j + 1][3] == "." and sig[j + 2][0] == "ident":
                col = f"{col}.{sig[j + 2][3]}"
                j += 2
            if sig[i + 1][3].lower() in KEYWORDS or (j + 1 < len(sig) and sig[j + 1][3] == "("):
                continue
            if col.split(".")[-1].lower() not in known_columns:
                continue
            ins = f" {col} = DATE '2001-01-01' and"
            pos = sig[i][2]
            out["type"].append((f"{col} = DATE", text[:pos] + ins + text[pos:]))
    result = []
    for kind, items in out.items():
        rng.shuffle(items)
        result += [(kind, d, m) for d, m in items[: PER_KIND[kind]]]
    return result


def explain(db, sql):
    sql = sql.strip().rstrip(";")
    res = subprocess.run(
        ["psql", "-h", PG["host"], "-p", str(PG["port"]), "-U", PG["user"], "-d", db, "-v",
         "ON_ERROR_STOP=1", "-qAt"],
        input=f"EXPLAIN {sql};\n", capture_output=True, text=True,
    )
    if res.returncode == 0:
        return None
    err = [l for l in res.stderr.splitlines() if "ERROR" in l]
    return (err[0] if err else res.stderr.strip())[:300]


def key(d):
    return (d["code"], d["line"], d["message"])


def new_diagnostics(orig, mutant):
    seen = defaultdict(int)
    for d in orig:
        seen[key(d)] += 1
    new = []
    for d in mutant:
        if seen[key(d)]:
            seen[key(d)] -= 1
        elif d["code"] != "E1000":
            new.append(d)
    return new


def mutate_project(p):
    dbtp = DbtProject(p)
    if not dbtp.ok:
        print(f"{p['name']}: not built")
        return None
    rng = random.Random(p["name"])
    root = project_root(p)
    nodes = dbtp.project_models()
    # the package the models come from, inside the dbt project
    records = []
    mut_models = {}  # mutant model name -> (record index, package dir)
    for rel, node in nodes:
        text = (root / rel).read_text(errors="replace")
        deps = node["depends_on"]["nodes"]
        cols = set()
        for uid in deps:
            dep = dbtp.manifest["nodes"].get(uid) or dbtp.manifest["sources"].get(uid)
            if not dep:
                continue
            schema = dep["schema"]
            name = dep.get("alias") or dep.get("identifier") or dep["name"]
            cols |= {c.lower() for c, _ in dbtp.tables.get((schema, name), [])}
        for n, (kind, desc, mtext) in enumerate(mutants(text, cols, rng)):
            mname = f"{node['name']}__mut{n}"
            records.append(dict(model=rel, node=node["unique_id"], kind=kind, desc=desc, name=mname))
            mut_models[mname] = (len(records) - 1, mtext, node)
    if not records:
        return []
    # add the mutants to the dbt project (next to the package's models) and compile
    pkg_dirs = {}
    for mname, (idx, mtext, node) in mut_models.items():
        pkg = node["package_name"]
        if pkg not in pkg_dirs:
            pkg_dirs[pkg] = locate_package(dbtp, pkg)
        dest = pkg_dirs[pkg] / "models" / "zz_corpus_mutants" / f"{mname}.sql"
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(mtext)
    work = WORK / p["name"]
    res = dbt(p, work, dbtp.proj, "compile", "--select", "zz_corpus_mutants", "--target-path", "target_mutants")
    for d in pkg_dirs.values():
        shutil.rmtree(d / "models" / "zz_corpus_mutants", ignore_errors=True)
    compiled_root = dbtp.proj / "target_mutants" / "compiled"
    db = dbname(p)
    # the oracle only counts for models whose original compiled SQL PostgreSQL accepts
    orig_ok = {}
    for rel, node in nodes:
        cpath = dbtp.target / "compiled" / node["package_name"] / node["original_file_path"]
        orig_ok[rel] = cpath.exists() and explain(db, cpath.read_text()) is None
    # mutant files for sqlsift
    mdir = MUT / p["name"]
    if mdir.exists():
        shutil.rmtree(mdir)
    raw_files, comp_files = {}, {}
    for mname, (idx, mtext, node) in mut_models.items():
        r = records[idx]
        rawp = mdir / "raw" / r["model"]
        rawp = rawp.with_name(f"{mname}.sql")
        rawp.parent.mkdir(parents=True, exist_ok=True)
        rawp.write_text(mtext)
        raw_files[idx] = os.path.relpath(rawp, mdir / "raw")
        cp = compiled_root / node["package_name"] / "models" / "zz_corpus_mutants" / f"{mname}.sql"
        r["original_ok"] = orig_ok.get(r["model"], False)
        if cp.exists() and r["original_ok"]:
            csql = cp.read_text()
            dest = mdir / "compiled" / f"{mname}.sql"
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(csql)
            comp_files[idx] = os.path.relpath(dest, mdir / "compiled")
            r["pg_error"] = explain(db, csql)
            r["compiled"] = True
        else:
            r["compiled"] = False
            r["pg_error"] = None
    # originals
    orig = {}
    for mode in ("raw", "catalog", "resolved", "compiled"):
        for dialect in p["dialects"]:
            f = RESULTS / p["name"] / mode / f"{dialect}.json"
            if f.exists():
                orig[(mode, dialect)] = json.loads(f.read_text())
    tmpl = templating_of(p)
    for dialect in p["dialects"]:
        # raw
        schema = [dbtp.ddl(dialect, with_source_aliases=True)]
        res = run_sqlsift(list(raw_files.values()), schema, dialect, tmpl, cwd=mdir / "raw")
        for idx, f in raw_files.items():
            o = orig[("raw", dialect)].get(records[idx]["model"], [])
            records[idx].setdefault("raw", {})[dialect] = summarize_new(new_diagnostics(o, res[f]))
        # catalog (--dbt-catalog)
        cat = SCHEMAS / p["name"] / f"catalog-{dialect}.json"
        res = run_sqlsift(list(raw_files.values()), [], dialect, tmpl, cwd=mdir / "raw", extra=["--dbt-catalog", str(cat)])
        for idx, f in raw_files.items():
            o = orig[("catalog", dialect)].get(records[idx]["model"], [])
            records[idx].setdefault("catalog", {})[dialect] = summarize_new(new_diagnostics(o, res[f]))
        # resolved
        rdir = mdir / "resolved" / dialect
        rfiles = {}
        for mname, (idx, mtext, node) in mut_models.items():
            dest = rdir / raw_files[idx]
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(dbtp.resolve_text(mtext, node, dialect))
            rfiles[idx] = raw_files[idx]
        res = run_sqlsift(list(rfiles.values()), [dbtp.ddl(dialect)], dialect, tmpl, cwd=rdir)
        for idx, f in rfiles.items():
            o = orig[("resolved", dialect)].get(records[idx]["model"], [])
            records[idx].setdefault("resolved", {})[dialect] = summarize_new(new_diagnostics(o, res[f]))
    if comp_files:
        res = run_sqlsift(list(comp_files.values()), [dbtp.ddl("postgresql")], "postgresql", "none", cwd=mdir / "compiled")
        comp_orig = orig.get(("compiled", "postgresql"), {})
        for idx, f in comp_files.items():
            node = dbtp.manifest["nodes"][records[idx]["node"]]
            okey = os.path.join(node["package_name"], node["original_file_path"])
            o = comp_orig.get(okey, [])
            records[idx]["compiled_pg"] = summarize_new(new_diagnostics(o, res[f]))
    (MUT / f"{p['name']}.json").write_text(json.dumps(records, indent=1))
    report(p["name"], records)
    return records


def summarize_new(new):
    return [f"{d['code']}:{d['line']}: {d['message']}" for d in new]


def locate_package(dbtp, pkg):
    if pkg == dbtp.root_name:
        return dbtp.proj
    for d in (dbtp.proj / "dbt_packages").iterdir():
        f = d / "dbt_project.yml"
        if f.exists() and re.search(rf"^name:\s*['\"]?{re.escape(pkg)}['\"]?\s*$", f.read_text(), re.M):
            return d.resolve()
    raise RuntimeError(f"package {pkg} not found")


def report(name, records):
    by = defaultdict(lambda: [0, 0, 0, 0])
    for r in records:
        if not r.get("pg_error"):
            continue
        b = by[r["kind"]]
        b[0] += 1
        b[1] += bool(r.get("compiled_pg"))
        b[2] += bool(r.get("resolved", {}).get("postgresql"))
        b[3] += bool(r.get("catalog", {}).get("postgresql"))
    total = len(records)
    bugs = sum(1 for r in records if r.get("pg_error"))
    print(f"{name}: {total} mutants, {bugs} rejected by PostgreSQL; detected (compiled/resolved/catalog, pg): "
          + ", ".join(f"{k} {v[1]}/{v[2]}/{v[3]} of {v[0]}" for k, v in sorted(by.items())))


def mutate_sql_project(p, limit=400):
    """Plain SQL projects: no oracle, so only mutants that are bugs whatever the
    schema: misspelled CTE references, and misspelled columns of tables in the schema
    (a name the schema has for a column of some table, in a file that reads that table).
    Results are "presumed bugs"."""
    from run import bq_yaml_ddl

    root = project_root(p)
    dialect = p["dialects"][0]
    rng = random.Random(p["name"])
    if p["schema"] == "bq-yaml":
        schema = bq_yaml_ddl(p, dialect)
    elif p["schema"] == "ddl":
        schema = sorted({f for g in p["schema_files"] for f in glob.glob(str(root / g), recursive=True)})
    else:
        schema = []
    tables = {}
    for s in schema[:1] if p["schema"] == "bq-yaml" else []:
        for m in re.finditer(r"CREATE TABLE ([^\s(]+) \((.*?)\n\);", Path(s).read_text(), re.S):
            name = m.group(1).split(".")[-1].strip("`")
            tables[name.lower()] = {c.split()[0].strip("`").lower() for c in m.group(2).split(",\n")}
    orig = json.loads((RESULTS / p["name"] / "raw" / f"{dialect}.json").read_text())
    clean = [f for f, ds in orig.items() if not ds]
    rng.shuffle(clean)
    records, texts = [], {}
    for rel in clean[:limit]:
        text = (root / rel).read_text(errors="replace")
        cols = set()
        for t, tcols in tables.items():
            if re.search(rf"\b{re.escape(t)}\b", text, re.I):
                cols |= tcols
        for n, (kind, desc, mtext) in enumerate(mutants(text, cols, rng)):
            if kind not in ("column-typo", "cte-typo"):
                continue
            name = f"{len(records):05d}.sql"
            records.append(dict(model=rel, kind=kind, desc=desc, name=name))
            texts[name] = mtext
    mdir = MUT / p["name"]
    if mdir.exists():
        shutil.rmtree(mdir)
    mdir.mkdir(parents=True)
    for name, t in texts.items():
        (mdir / name).write_text(t)
    res = run_sqlsift(sorted(texts), schema, dialect, templating_of(p), cwd=mdir)
    for r in records:
        r["presumed_bug"] = True
        r.setdefault("raw", {})[dialect] = summarize_new(new_diagnostics(orig[r["model"]], res[r["name"]]))
    (MUT / f"{p['name']}.json").write_text(json.dumps(records, indent=1))
    by = defaultdict(Counter)
    for r in records:
        by[r["kind"]]["n"] += 1
        by[r["kind"]]["found"] += bool(r["raw"][dialect])
    print(f"{p['name']}: " + ", ".join(f"{k} {v['found']}/{v['n']}" for k, v in sorted(by.items())))


def main():
    MUT.mkdir(parents=True, exist_ok=True)
    for p in projects(sys.argv[1:]):
        if p["schema"] == "dbt-run":
            mutate_project(p)
        elif p["kind"] == "sql":
            mutate_sql_project(p)


if __name__ == "__main__":
    main()
