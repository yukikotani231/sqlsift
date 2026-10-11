#!/usr/bin/env python3
"""Build the "dbt-run" projects against a local PostgreSQL.

Usage: python3 tests/corpus/dbt_build.py [NAME ...]

Needs `dbt` (dbt-postgres) on PATH (or $DBT) and a PostgreSQL server reachable with
$PGHOST/$PGPORT/$PGUSER (default 127.0.0.1:5432, user postgres, trust auth).

For each project, the repository is copied to out/work/<name>/ and the dbt
dependencies are installed from local clones (repos/_deps/, see fetch.py) instead of
`dbt deps`, since dbt hub may not be reachable. Then `dbt seed`, `dbt run` and
`dbt docs generate` run into database `corpus_<name>`; target/ then has
manifest.json, catalog.json and the compiled models.
"""

import os
import shutil
import subprocess
import sys
from pathlib import Path

import yaml

from corpus import REPOS, WORK, clone_dir, projects

PG = dict(
    host=os.environ.get("PGHOST", "127.0.0.1"),
    port=int(os.environ.get("PGPORT", "5432")),
    user=os.environ.get("PGUSER", "postgres"),
)
DBT = os.environ.get("DBT", "dbt")

IGNORE = shutil.ignore_patterns(".git", "dbt_packages", "target", "logs")


def dbname(p):
    return "corpus_" + p["name"].replace("-", "__").lower()


def psql(db, sql):
    return subprocess.run(
        ["psql", "-h", PG["host"], "-p", str(PG["port"]), "-U", PG["user"], "-d", db,
         "-v", "ON_ERROR_STOP=1", "-qAtc", sql],
        capture_output=True, text=True,
    )


def setup(p):
    work = WORK / p["name"]
    if work.exists():
        shutil.rmtree(work)
    shutil.copytree(clone_dir(p), work, ignore=IGNORE, symlinks=True)
    proj = work / p.get("root", ".") / p["dbt_project"]
    # local copies of the packages, installed with an all-`local:` packages.yml
    pkgs = WORK / (p["name"] + ".packages")
    if pkgs.exists():
        shutil.rmtree(pkgs)
    pkgs.mkdir()
    for dep in p.get("deps", []):
        shutil.copytree(REPOS / "_deps" / dep, pkgs / dep, ignore=IGNORE, symlinks=True)
    # `local:` packages (fivetran integration_tests → ../)
    for f in ("packages.yml", "dependencies.yml"):
        path = proj / f
        if path.exists():
            data = yaml.safe_load(path.read_text()) or {}
            for entry in data.get("packages") or []:
                if "local" in entry:
                    src = (proj / entry["local"]).resolve()
                    dest = pkgs / ("local_" + src.name)
                    shutil.copytree(
                        src, dest, symlinks=True,
                        ignore=shutil.ignore_patterns(".git", "dbt_packages", "target", "logs", "integration_tests"),
                    )
            path.unlink()
    lock = proj / "package-lock.yml"
    if lock.exists():
        lock.unlink()
    # the dependencies' own packages.yml would pull from dbt hub
    for f in list(pkgs.glob("*/packages.yml")) + list(pkgs.glob("*/dependencies.yml")):
        f.unlink()
    locals_ = [{"local": os.path.relpath(d, proj)} for d in sorted(pkgs.iterdir())]
    (proj / "packages.yml").write_text(yaml.safe_dump({"packages": locals_}))
    # semantic layer configs newer than the installed dbt can't be parsed; they don't
    # change any model
    semantic = {"metrics", "semantic_models", "semantic_model", "saved_queries", "unit_tests", "time_spine"}

    def strip(node):
        if isinstance(node, dict):
            return {k: strip(v) for k, v in node.items() if k not in semantic}
        if isinstance(node, list):
            return [strip(v) for v in node]
        return node

    for f in list(proj.glob("models/**/*.yml")) + list(proj.glob("models/**/*.yaml")):
        data = yaml.safe_load(f.read_text())
        stripped = strip(data)
        if stripped != data:
            f.write_text(yaml.safe_dump(stripped))
    profile = yaml.safe_load((proj / "dbt_project.yml").read_text())["profile"]
    profiles = {
        profile: {
            "target": "postgres",
            "outputs": {
                "postgres": {
                    "type": "postgres", "host": PG["host"], "port": PG["port"],
                    "user": PG["user"], "password": "", "dbname": dbname(p),
                    "schema": p.get("dbt_schema", "corpus"), "threads": 4,
                }
            },
        }
    }
    (work / "profiles.yml").write_text(yaml.safe_dump(profiles))
    return work, proj


def dbt(p, work, proj, *args):
    cmd = [DBT, *args, "--project-dir", str(proj), "--profiles-dir", str(work), "--no-version-check"]
    if p.get("dbt_vars"):
        cmd += ["--vars", p["dbt_vars"]]
    res = subprocess.run(cmd, capture_output=True, text=True, cwd=proj)
    log = work / "dbt.log"
    with open(log, "a") as f:
        f.write(f"$ {' '.join(cmd)}\n{res.stdout}\n{res.stderr}\n")
    return res


def build(p):
    db = dbname(p)
    psql("postgres", f"DROP DATABASE IF EXISTS {db} WITH (FORCE)")
    psql("postgres", f"CREATE DATABASE {db}")
    work, proj = setup(p)
    steps = {}
    for step in (["deps"], ["seed", "--full-refresh"], ["run", "--full-refresh"], ["docs", "generate"]):
        res = dbt(p, work, proj, *step)
        tail = [l for l in res.stdout.splitlines() if "Done." in l or "ERROR" in l][-3:]
        steps[step[0]] = (res.returncode, tail)
    ok = (proj / "target" / "catalog.json").exists()
    print(f"{'ok  ' if ok else 'FAIL'} {p['name']}: " + "; ".join(
        f"{k}={rc} {' | '.join(t)[:200]}" for k, (rc, t) in steps.items()))
    return ok


def main():
    names = sys.argv[1:]
    ok = True
    for p in projects(names):
        if p["schema"] == "dbt-run":
            ok &= build(p)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
