#!/usr/bin/env python3
"""Check every corpus project with sqlsift and record the diagnostics.

Usage: python3 tests/corpus/run.py [NAME ...]

Modes (see README.md):
  raw       the query files as they are in the repository (dbt models: Jinja
            templates, refs are unknown tables), schema from the manifest's `schema`
            ("dbt-run": catalog.json as DDL, sources also as <source>.<table>)
  catalog   dbt models checked with `--dbt-catalog` (sqlsift's catalog.json support)
            and no schema files; the catalog's types are renamed to the dialect's
            type names ("dbt-run" projects only)
  resolved  dbt models with `{{ ref() }}`, `{{ source() }}`, `{{ var() }}` naming a
            relation and `{{ this }}` replaced by the relation name from dbt's
            manifest.json, schema = every relation of catalog.json. This is what
            dbt catalog support can achieve ("dbt-run" projects only)
  compiled  the SQL dbt compiled for PostgreSQL, schema = catalog.json ("dbt-run"
            projects only, postgresql only). dbt ran every model successfully on
            PostgreSQL, so every diagnostic other than a parse error is a false
            positive.

Writes out/results/<project>/<mode>/<dialect>.json ({file: [diagnostic]}) and
out/summary.json.
"""

import glob
import json
import os
import re
import shutil
import sys
from collections import Counter
from pathlib import Path

import yaml

from corpus import (
    OUT,
    WORK,
    catalog_for_dialect,
    catalog_tables,
    create_table,
    load_dbt_manifest,
    model_files,
    project_root,
    projects,
    quote,
    relation_parts,
    run_sqlsift,
    write_ddl,
)

RESULTS = OUT / "results"
SCHEMAS = OUT / "schemas"


class Loader(yaml.SafeLoader):
    """YAML loader for bigquery-etl's custom tags.

    `!include-field` / `!include-fields` become {"__include": tag, ...mapping}; other
    tags (`!include-field-description`) are dropped.
    """


def _custom_tag(loader, suffix, node):
    if suffix in ("include-field", "include-fields") and isinstance(node, yaml.MappingNode):
        return {"__include": suffix, **loader.construct_mapping(node, deep=True)}
    return None


Loader.add_multi_constructor("!", _custom_tag)


def load_yaml(path):
    try:
        return yaml.load(Path(path).read_text(errors="replace"), Loader=Loader) or {}
    except Exception:
        return {}


def templating_of(p):
    return p.get("templating") or ("jinja" if p["kind"] == "dbt" or p["schema"] == "bq-yaml" else "none")


# --- dbt-run projects --------------------------------------------------------------


class DbtProject:
    """A dbt-run project after dbt_build.py: its manifest, catalog and paths."""

    def __init__(self, p):
        self.p = p
        self.work = WORK / p["name"]
        self.proj = self.work / p.get("root", ".") / p["dbt_project"]
        self.target = self.proj / "target"
        self.ok = (self.target / "catalog.json").exists()
        if not self.ok:
            return
        self.manifest = load_dbt_manifest(self.target)
        self.tables = catalog_tables(self.target / "catalog.json")
        self.root_name = self.manifest["metadata"].get("project_name") or yaml.safe_load(
            (self.proj / "dbt_project.yml").read_text()
        )["name"]
        self.models = {}  # name -> [node]
        for node in self.manifest["nodes"].values():
            if node["resource_type"] in ("model", "seed", "snapshot"):
                self.models.setdefault(node["name"], []).append(node)
        self.sources = {}
        for node in self.manifest["sources"].values():
            self.sources[(node["source_name"], node["name"])] = node
        self.vars = self._vars()

    def _vars(self):
        """Var values of the root project and its packages that may name relations."""
        out = {}
        files = [self.proj / "dbt_project.yml"] + list((self.proj / "dbt_packages").glob("*/dbt_project.yml"))
        for f in files:
            data = load_yaml(f).get("vars") or {}
            stack = [data]
            while stack:
                d = stack.pop()
                for k, v in d.items():
                    if isinstance(v, dict):
                        stack.append(v)
                    elif isinstance(v, str):
                        out.setdefault(k, v)
        # the root project's own vars win
        for k, v in (load_yaml(self.proj / "dbt_project.yml").get("vars") or {}).items():
            if isinstance(v, str):
                out[k] = v
        return out

    def raw_path(self, node):
        """Path of a node's file relative to the repository clone."""
        if node["package_name"] == self.root_name:
            base = Path(self.p.get("root", ".")) / self.p["dbt_project"]
        else:
            base = Path(self.p.get("root", "."))
        return os.path.normpath(base / node["original_file_path"])

    def project_models(self):
        """Model nodes whose file is one of the project's query files."""
        wanted = set(model_files(self.p))
        nodes = []
        for node in self.manifest["nodes"].values():
            if node["resource_type"] != "model":
                continue
            rel = os.path.relpath(self.raw_path(node), self.p.get("root", "."))
            if rel in wanted:
                nodes.append((rel, node))
        return sorted(nodes, key=lambda x: x[0])

    def built(self, node):
        """Whether dbt built the model (it is in the catalog)."""
        return relation_parts(node) in self.tables

    def relation(self, node, dialect):
        if node.get("config", {}).get("materialized") == "ephemeral":
            return None
        schema, name = relation_parts(node)
        if (schema, name) not in self.tables:
            return None
        return f"{quote(schema, dialect)}.{quote(name, dialect)}"

    def resolve_tag(self, expr, this_node, dialect, depth=0):
        """Relation name for the expression inside a `{{ }}` tag, or None."""
        expr = expr.strip()
        m = re.fullmatch(r"ref\(\s*['\"]([^'\"]+)['\"]\s*(?:,\s*['\"]([^'\"]+)['\"]\s*)?(?:,\s*v(?:ersion)?\s*=\s*\S+\s*)?\)", expr)
        if m:
            name = m.group(2) or m.group(1)
            nodes = self.models.get(name, [])
            if m.group(2):
                nodes = [n for n in nodes if n["package_name"] == m.group(1)] or nodes
            return self.relation(nodes[0], dialect) if nodes else None
        m = re.fullmatch(r"source\(\s*['\"]([^'\"]+)['\"]\s*,\s*['\"]([^'\"]+)['\"]\s*\)", expr)
        if m:
            node = self.sources.get((m.group(1), m.group(2)))
            if not node:
                return None
            schema, name = node["schema"], node.get("identifier") or node["name"]
            if (schema, name) not in self.tables:
                return None
            return f"{quote(schema, dialect)}.{quote(name, dialect)}"
        m = re.fullmatch(r"var\(\s*['\"]([^'\"]+)['\"].*\)", expr, re.S)
        if m and depth < 3:
            value = self.vars.get(m.group(1))
            if isinstance(value, str):
                inner = re.fullmatch(r"\s*\{\{(.*)\}\}\s*", value, re.S)
                if inner:
                    return self.resolve_tag(inner.group(1), this_node, dialect, depth + 1)
            return None
        if expr == "this":
            return self.relation(this_node, dialect)
        return None

    def resolve_text(self, text, node, dialect):
        def repl(m):
            rel = self.resolve_tag(m.group(1), node, dialect)
            return rel if rel else m.group(0)

        return re.sub(r"\{\{(.*?)\}\}", repl, text, flags=re.S)

    def ddl(self, dialect, with_source_aliases=False):
        aliases = {}
        if with_source_aliases:
            # sqlsift reads `{{ source('a', 'b') }}` as table a.b
            for (src, name), node in self.sources.items():
                key = (node["schema"], node.get("identifier") or node["name"])
                if key in self.tables:
                    aliases[(src, name)] = self.tables[key]
        path = SCHEMAS / self.p["name"] / f"catalog-{dialect}{'-raw' if with_source_aliases else ''}.sql"
        return write_ddl(path, self.tables, dialect, aliases)


# --- schema from .yml declarations -------------------------------------------------


def yml_tables(p):
    """{(source_name, table): [(column, data_type)]} from sources in .yml files."""
    tables = {}
    root = project_root(p)
    for f in glob.glob(str(root / "**/*.yml"), recursive=True) + glob.glob(str(root / "**/*.yaml"), recursive=True):
        if any(s in f for s in ("/dbt_packages/", "/target/", "/integration_tests/")):
            continue
        for src in load_yaml(f).get("sources") or []:
            if not isinstance(src, dict):
                continue
            for t in src.get("tables") or []:
                cols = [(c["name"], c.get("data_type")) for c in t.get("columns") or [] if isinstance(c, dict) and c.get("name")]
                if cols:
                    tables[(src["name"], t["name"])] = cols
    return tables


def bq_yaml_ddl(p, dialect):
    """Schema of bigquery-etl: a table per schema.yaml (dataset.table, from the query
    directory), plus the CREATE VIEW statements of the view.sql files."""
    root = project_root(p)
    fields_of = {}
    for f in sorted(glob.glob(str(root / "sql/*/*/*/schema.yaml"))):
        table_dir = Path(f).parent
        fields_of[(table_dir.parent.parent.name, table_dir.parent.name, table_dir.name)] = (
            load_yaml(f).get("fields") or []
        )

    def expand(fields, depth=0):
        out = []
        for fld in fields:
            if not isinstance(fld, dict):
                continue
            inc = fld.get("__include")
            if inc and depth < 3:
                target = tuple(str(fld.get("table", "")).split("."))
                included = expand(fields_of.get(target, []), depth + 1)
                if inc == "include-field":
                    out += [dict(f, name=fld.get("new_name") or f["name"]) for f in included if f["name"] == fld.get("field")]
                else:
                    out += included
            elif fld.get("name"):
                out.append(fld)
        return out

    parts = []
    for (_, dataset, table), fields in fields_of.items():
        cols = []
        for fld in expand(fields):
            t = (fld.get("type") or "").upper()
            if fld.get("mode") == "REPEATED" or t in ("RECORD", "STRUCT"):
                cols.append((fld["name"], None))  # nested: type left unknown
            else:
                cols.append((fld["name"], {"INTEGER": "int64", "FLOAT": "float64", "BOOLEAN": "bool"}.get(t, t.lower())))
        parts.append(create_table(dataset, table, cols, dialect))
    path = SCHEMAS / p["name"] / f"schema-{dialect}.sql"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(parts))
    views = sorted(glob.glob(str(root / "sql/moz-fx-data-shared-prod/*/*/view.sql")))
    return [path] + views


# --- schema from DDL files ---------------------------------------------------------


def ddl_files(p):
    """The `schema_files` globs in the manifest's order (each glob's matches sorted)."""
    root = project_root(p)
    files = []
    for g in p.get("schema_files", []):
        for f in sorted(glob.glob(str(root / g), recursive=True)):
            if f not in files:
                files.append(f)
    return files


def schema_dir(p):
    """The migrations directory, or a copy of it with the manifest's `schema_rewrite`
    replacements applied (what the application does to its migrations at runtime)"""
    root = project_root(p)
    src = root / p["schema_dir"]
    if not p.get("schema_rewrite"):
        return src
    dest = SCHEMAS / p["name"] / "migrations"
    if dest.exists():
        shutil.rmtree(dest)
    for f in sorted(src.rglob("*.sql")):
        text = f.read_text(errors="replace")
        for old, new in p["schema_rewrite"]:
            text = text.replace(old, new)
        out = dest / f.relative_to(src)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(text)
    return dest


TS_SQL_TAG = re.compile(r"\bsql`((?:[^`\\]|\\.)*)`", re.S)


def ts_sql_schema(p):
    """DDL of migrations written as tagged templates in TypeScript (`sql\\`CREATE ...\\``):
    the templates of the `schema_files`, one statement each, written to a .sql file."""
    parts = []
    for f in ddl_files(p):
        for m in TS_SQL_TAG.finditer(Path(f).read_text(errors="replace")):
            body = m.group(1).strip().rstrip(";")
            if "${" not in body:
                parts.append(body + ";\n")
    path = SCHEMAS / p["name"] / "ts-schema.sql"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(parts))
    return path


# --- driver --------------------------------------------------------------------------


def save(p, mode, dialect, result, extra=None):
    d = RESULTS / p["name"] / mode
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{dialect}.json").write_text(json.dumps(result, indent=1, sort_keys=True))
    return summarize(p, mode, dialect, result, extra or {})


def summarize(p, mode, dialect, result, extra):
    files = len(result)
    parse_fail = sum(1 for ds in result.values() if any(d["code"] == "E1000" for d in ds))
    codes = Counter(d["code"] for ds in result.values() for d in ds)
    clean = sum(1 for ds in result.values() if not ds)
    row = dict(project=p["name"], mode=mode, dialect=dialect, schema=p["schema"], files=files,
               parsed=files - parse_fail, clean=clean, codes=dict(sorted(codes.items())), **extra)
    print(f"{p['name']:28} {mode:9} {dialect:11} files={files:4} parsed={files - parse_fail:4} "
          f"clean={clean:4} {dict(sorted(codes.items()))}")
    return row


def run_project(p):
    rows = []
    templating = templating_of(p)
    root = project_root(p)
    files = model_files(p)
    if not files:
        print(f"{p['name']}: no files")
        return rows
    dbtp = DbtProject(p) if p["schema"] == "dbt-run" else None
    for dialect in p["dialects"]:
        # raw
        if dbtp and dbtp.ok:
            schema = [dbtp.ddl(dialect, with_source_aliases=True)]
        elif p["schema"] == "yml":
            schema = [write_ddl(SCHEMAS / p["name"] / f"yml-{dialect}.sql", yml_tables(p), dialect)]
        elif p["schema"] == "ddl":
            schema = ddl_files(p)
        elif p["schema"] == "ts-sql":
            schema = [ts_sql_schema(p)]
        elif p["schema"] == "bq-yaml":
            schema = bq_yaml_ddl(p, dialect)
        else:
            schema = [SCHEMAS / "empty.sql"]
            schema[0].parent.mkdir(parents=True, exist_ok=True)
            schema[0].write_text("-- no schema\n")
        # a migrations directory is loaded by sqlsift itself (`--schema-dir`: filename
        # order, rollback migrations skipped), the way an app developer runs it
        extra = ["--schema-dir", str(schema_dir(p))] if p.get("schema_dir") else []
        result = run_sqlsift(files, schema, dialect, templating, cwd=root, extra=extra)
        rows.append(save(p, "raw", dialect, result))
        if not (dbtp and dbtp.ok):
            continue
        # catalog: sqlsift's own dbt catalog.json support, no schema files
        cat = catalog_for_dialect(dbtp.target / "catalog.json", dialect, SCHEMAS / p["name"] / f"catalog-{dialect}.json")
        result = run_sqlsift(files, [], dialect, templating, cwd=root, extra=["--dbt-catalog", str(cat)])
        rows.append(save(p, "catalog", dialect, result))
        # resolved
        nodes = dbtp.project_models()
        rdir = OUT / "resolved" / p["name"] / dialect
        rfiles = []
        for rel, node in nodes:
            text = (root / rel).read_text(errors="replace")
            dest = rdir / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(dbtp.resolve_text(text, node, dialect))
            rfiles.append(rel)
        result = run_sqlsift(rfiles, [dbtp.ddl(dialect)], dialect, templating, cwd=rdir)
        rows.append(save(p, "resolved", dialect, result))
        # compiled (PostgreSQL)
        if dialect == "postgresql":
            cfiles = []
            for rel, node in nodes:
                cpath = dbtp.target / "compiled" / node["package_name"] / node["original_file_path"]
                # only models dbt built (ephemeral models are inlined, not built)
                if cpath.exists() and dbtp.built(node):
                    cfiles.append(os.path.relpath(cpath, dbtp.target / "compiled"))
            result = run_sqlsift(cfiles, [dbtp.ddl(dialect)], dialect, "none", cwd=dbtp.target / "compiled")
            rows.append(save(p, "compiled", dialect, result))
    return rows


def main():
    names = sys.argv[1:]
    summary_path = OUT / "summary.json"
    old = json.loads(summary_path.read_text()) if summary_path.exists() else []
    rows = []
    for p in projects(names):
        rows += run_project(p)
    done = {(r["project"], r["mode"], r["dialect"]) for r in rows}
    rows = [r for r in old if (r["project"], r["mode"], r["dialect"]) not in done] + rows
    summary_path.write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()
