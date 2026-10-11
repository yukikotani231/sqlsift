"""Shared helpers of the real-world corpus harness (see README.md)."""

import glob
import json
import os
import re
import subprocess
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPOS = HERE / "repos"
OUT = HERE / "out"
WORK = OUT / "work"
ROOT = HERE.parent.parent

DIALECTS = ["postgresql", "snowflake", "bigquery", "redshift", "databricks"]


def load_manifest():
    with open(HERE / "manifest.toml", "rb") as f:
        return tomllib.load(f)


def projects(names=()):
    return [p for p in load_manifest()["project"] if not names or p["name"] in names]


def clone_dir(p):
    return REPOS / p.get("clone", p["name"])


def project_root(p):
    return clone_dir(p) / p.get("root", ".")


def sqlsift_bin():
    """The sqlsift binary to test: $SQLSIFT, else target/release/sqlsift of this repo."""
    return os.environ.get("SQLSIFT") or str(ROOT / "target" / "release" / "sqlsift")


SKIP_DIRS = ("/dbt_packages/", "/target/", "/macros/", "/.git/")


def model_files(p, base=None):
    """Query files of a project (relative to `base`, default the project root)."""
    base = Path(base or project_root(p))
    files = set()
    for pattern in p["models"]:
        for f in glob.glob(str(base / pattern), recursive=True):
            rel = os.path.relpath(f, base)
            if any(s in "/" + rel for s in SKIP_DIRS):
                continue
            files.add(rel)
    for pattern in p.get("exclude", []):
        files -= {os.path.relpath(f, base) for f in glob.glob(str(base / pattern), recursive=True)}
    return sorted(files)


# --- running sqlsift ---------------------------------------------------------------


def run_sqlsift(files, schema_files, dialect, templating, cwd, chunk=400, extra=()):
    """Check `files` (relative to `cwd`); returns {file: [diagnostic, ...]}.

    Every file is in the result (files without diagnostics map to []).
    """
    if not schema_files:
        schema_files = [empty_schema()]
    result = {f: [] for f in files}
    for i in range(0, len(files), chunk):
        part = files[i : i + chunk]
        cmd = [
            sqlsift_bin(),
            "check",
            "--format",
            "json",
            "--max-errors",
            "100000",
            "--dialect",
            dialect,
            "--templating",
            templating,
            # no sqlsift.toml of the repo under test (there is none, but be explicit)
            "--config",
            str(HERE / "sqlsift-corpus.toml"),
        ]
        for s in schema_files:
            cmd += ["--schema", str(s)]
        cmd += list(extra) + part
        res = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
        out = res.stdout
        start = out.find("{")
        if start < 0:
            raise RuntimeError(f"sqlsift failed: {res.stderr[:2000]}")
        data, _ = json.JSONDecoder().raw_decode(out[start:])
        for f in data["files"]:
            result[os.path.normpath(f["file"])] = f["diagnostics"]
    return result


def empty_schema():
    path = OUT / "schemas" / "empty.sql"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("-- no schema\n")
    return path


# --- schema generation -------------------------------------------------------------

IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
# Words that need quoting as a column name in some dialect's CREATE TABLE
RESERVED = {
    "all", "and", "any", "array", "as", "asc", "between", "both", "by", "case", "cast",
    "check", "collate", "column", "constraint", "create", "cross", "current_date",
    "current_time", "current_timestamp", "current_user", "default", "desc", "distinct",
    "else", "end", "except", "exists", "false", "fetch", "for", "foreign", "from", "full",
    "grant", "group", "having", "in", "inner", "intersect", "interval", "into", "is",
    "join", "lateral", "leading", "left", "like", "limit", "natural", "not", "null",
    "of", "offset", "on", "or", "order", "outer", "primary", "qualify", "references",
    "right", "rows", "select", "set", "some", "table", "then", "to", "trailing", "true",
    "union", "unique", "user", "using", "when", "where", "window", "with", "hash",
    "range", "groups", "merge", "proto", "struct", "within", "rollup", "cube",
}


def quote(name, dialect):
    if IDENT.match(name) and name.lower() not in RESERVED and name == name.lower():
        return name
    if dialect in ("bigquery", "databricks", "mysql"):
        return "`" + name.replace("`", "") + "`"
    return '"' + name.replace('"', '""') + '"'


def map_type(pg_type, dialect):
    """A PostgreSQL catalog type (or a dbt `data_type`) as a type of `dialect`."""
    t = (pg_type or "").lower().strip()
    if not t:
        return None
    base = re.sub(r"\(.*\)", "", t).strip()
    family = None
    if base in ("integer", "int", "int4", "smallint", "int2", "bigint", "int8", "int64", "number", "byteint", "tinyint"):
        family = "int"
    elif base in ("numeric", "decimal", "bignumeric", "money"):
        family = "numeric"
    elif base in ("double precision", "real", "float", "float4", "float8", "float64", "double"):
        family = "float"
    elif base in ("text", "character varying", "varchar", "character", "char", "string", "name", "bpchar", "citext", "uuid"):
        family = "string"
    elif base in ("boolean", "bool"):
        family = "bool"
    elif base == "date":
        family = "date"
    elif base.startswith("timestamp") or base in ("datetime", "timestamptz", "timestamp_ntz", "timestamp_tz", "timestamp_ltz"):
        family = "timestamp_tz" if ("with time zone" in base or base in ("timestamptz", "timestamp_tz", "timestamp_ltz")) else "timestamp"
    elif base.startswith("time"):
        family = "time"
    elif base in ("json", "jsonb", "variant", "super", "object"):
        family = "json"
    elif base.endswith("[]") or base == "array" or base.startswith("array"):
        family = "array"
    elif base == "interval":
        family = "interval"
    if family is None:
        return None
    table = {
        "postgresql": dict(int="BIGINT", numeric="NUMERIC", float="DOUBLE PRECISION", string="TEXT", bool="BOOLEAN", date="DATE", timestamp="TIMESTAMP", timestamp_tz="TIMESTAMPTZ", time="TIME", json="JSONB", array="TEXT[]", interval="INTERVAL"),
        "redshift": dict(int="BIGINT", numeric="NUMERIC", float="DOUBLE PRECISION", string="VARCHAR", bool="BOOLEAN", date="DATE", timestamp="TIMESTAMP", timestamp_tz="TIMESTAMPTZ", time="TIME", json="SUPER", array="SUPER", interval="INTERVAL"),
        "snowflake": dict(int="NUMBER", numeric="NUMBER", float="FLOAT", string="VARCHAR", bool="BOOLEAN", date="DATE", timestamp="TIMESTAMP_NTZ", timestamp_tz="TIMESTAMP_TZ", time="TIME", json="VARIANT", array="ARRAY", interval=None),
        "bigquery": dict(int="INT64", numeric="NUMERIC", float="FLOAT64", string="STRING", bool="BOOL", date="DATE", timestamp="TIMESTAMP", timestamp_tz="TIMESTAMP", time="TIME", json="JSON", array="ARRAY<STRING>", interval="INTERVAL"),
        "databricks": dict(int="BIGINT", numeric="DECIMAL(38,6)", float="DOUBLE", string="STRING", bool="BOOLEAN", date="DATE", timestamp="TIMESTAMP", timestamp_tz="TIMESTAMP", time="STRING", json="STRING", array="ARRAY<STRING>", interval=None),
    }
    return table.get(dialect, table["postgresql"]).get(family)


UNKNOWN_TYPE = "BIT"


def create_table(schema, name, columns, dialect, unknown_type=None):
    """`CREATE TABLE schema.name (...)`; `columns` is [(name, type or None)].

    A column of unknown type gets `unknown_type`, by default BIT: a type sqlsift has
    no type rules for (SqlType::Unknown), so it is never part of a type mismatch.
    """
    cols = []
    for col, typ in columns:
        t = map_type(typ, dialect) if typ else None
        if t is None:
            t = unknown_type or UNKNOWN_TYPE
        cols.append(f"  {quote(col, dialect)} {t}")
    if not cols:
        return ""
    qual = f"{quote(schema, dialect)}." if schema else ""
    return f"CREATE TABLE {qual}{quote(name, dialect)} (\n" + ",\n".join(cols) + "\n);\n"


# Type names as each warehouse reports them in dbt's catalog.json
CATALOG_TYPES = {
    "snowflake": dict(int="NUMBER", numeric="NUMBER", float="FLOAT", string="TEXT", bool="BOOLEAN", date="DATE", timestamp="TIMESTAMP_NTZ", timestamp_tz="TIMESTAMP_TZ", time="TIME", json="VARIANT", array="ARRAY", interval="TEXT"),
    "bigquery": dict(int="INT64", numeric="NUMERIC", float="FLOAT64", string="STRING", bool="BOOL", date="DATE", timestamp="TIMESTAMP", timestamp_tz="TIMESTAMP", time="TIME", json="JSON", array="ARRAY<STRING>", interval="INTERVAL"),
    "redshift": dict(int="bigint", numeric="numeric", float="double precision", string="character varying", bool="boolean", date="date", timestamp="timestamp without time zone", timestamp_tz="timestamp with time zone", time="time without time zone", json="super", array="super", interval="interval"),
    "databricks": dict(int="bigint", numeric="decimal(38,6)", float="double", string="string", bool="boolean", date="date", timestamp="timestamp", timestamp_tz="timestamp", time="string", json="string", array="array<string>", interval="interval"),
}


def type_family(pg_type):
    """The family (int, string, ...) of a PostgreSQL type, via map_type's table."""
    t = map_type(pg_type, "postgresql")
    reverse = {
        "BIGINT": "int", "NUMERIC": "numeric", "DOUBLE PRECISION": "float", "TEXT": "string",
        "BOOLEAN": "bool", "DATE": "date", "TIMESTAMP": "timestamp", "TIMESTAMPTZ": "timestamp_tz",
        "TIME": "time", "JSONB": "json", "TEXT[]": "array", "INTERVAL": "interval",
    }
    return reverse.get(t)


def catalog_for_dialect(catalog_path, dialect, out_path):
    """Copy of a PostgreSQL catalog.json with column types as `dialect` names them."""
    cat = json.loads(Path(catalog_path).read_text())
    if dialect in CATALOG_TYPES:
        for section in ("nodes", "sources"):
            for node in cat.get(section, {}).values():
                for col in node["columns"].values():
                    family = type_family(col["type"])
                    if family:
                        col["type"] = CATALOG_TYPES[dialect][family]
    Path(out_path).parent.mkdir(parents=True, exist_ok=True)
    Path(out_path).write_text(json.dumps(cat))
    return out_path


def catalog_tables(catalog_path):
    """{(schema, name): [(column, type)]} of a dbt target/catalog.json."""
    cat = json.loads(Path(catalog_path).read_text())
    tables = {}
    for section in ("nodes", "sources"):
        for node in cat.get(section, {}).values():
            meta = node["metadata"]
            cols = sorted(node["columns"].values(), key=lambda c: c["index"])
            tables[(meta["schema"], meta["name"])] = [(c["name"], c["type"]) for c in cols]
    return tables


def write_ddl(path, tables, dialect, extra_aliases=None):
    """Write CREATE TABLE statements for `tables` ({(schema, name): columns})."""
    parts = []
    for (schema, name), cols in sorted(tables.items()):
        parts.append(create_table(schema, name, cols, dialect))
    for (schema, name), cols in sorted((extra_aliases or {}).items()):
        if (schema, name) not in tables:
            parts.append(create_table(schema, name, cols, dialect))
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text("".join(parts))
    return path


# --- dbt manifest helpers ----------------------------------------------------------


def load_dbt_manifest(target):
    return json.loads((Path(target) / "manifest.json").read_text())


def relation_parts(node):
    return node["schema"], node.get("alias") or node.get("identifier") or node["name"]


def jinja_spans(text):
    """[(start, end)] of Jinja tags ({{ }}, {% %}, {# #}) in `text`."""
    spans = []
    i = 0
    while True:
        m = re.compile(r"\{[{%#]").search(text, i)
        if not m:
            return spans
        close = {"{{": "}}", "{%": "%}", "{#": "#}"}[m.group()]
        end = text.find(close, m.end())
        end = len(text) if end < 0 else end + 2
        spans.append((m.start(), end))
        i = end
