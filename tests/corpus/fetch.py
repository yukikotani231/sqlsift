#!/usr/bin/env python3
"""Clone every project (and dbt package dependency) of manifest.toml.

Usage: python3 tests/corpus/fetch.py [NAME ...]

Shallow clones only (`git clone --depth 1`) into tests/corpus/repos/ (projects) and
tests/corpus/repos/_deps/ (dependencies). Writes tests/corpus/out/lock.json with the
commit each clone is at, and warns when it differs from the `sha` in the manifest.
"""

import json
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

from corpus import OUT, REPOS, clone_dir, load_manifest


def fetch(item):
    name, url, ref, dest = item
    if not dest.exists():
        dest.parent.mkdir(parents=True, exist_ok=True)
        cmd = ["git", "clone", "-q", "--depth", "1"]
        if ref:
            cmd += ["--branch", ref]
        res = subprocess.run(cmd + [url, str(dest)], capture_output=True, text=True)
        if res.returncode != 0:
            return name, None, res.stderr.strip()
    sha = subprocess.run(
        ["git", "-C", str(dest), "rev-parse", "HEAD"], capture_output=True, text=True
    ).stdout.strip()
    return name, sha, None


def main():
    wanted = set(sys.argv[1:])
    manifest = load_manifest()
    items, pins = {}, {}
    for p in manifest["project"]:
        if wanted and p["name"] not in wanted:
            continue
        dest = clone_dir(p)
        items[dest.name] = (dest.name, p["url"], None, dest)
        if p.get("sha"):
            pins[dest.name] = p["sha"]
    for d in manifest.get("dependency", []):
        if not wanted or d["name"] in wanted:
            name = "_deps/" + d["name"]
            items[name] = (name, d["url"], d.get("ref"), REPOS / "_deps" / d["name"])
    REPOS.mkdir(exist_ok=True)
    OUT.mkdir(exist_ok=True)
    lock_path = OUT / "lock.json"
    lock = json.loads(lock_path.read_text()) if lock_path.exists() else {}
    failed = 0
    with ThreadPoolExecutor(max_workers=8) as pool:
        for name, sha, err in pool.map(fetch, items.values()):
            if err:
                failed += 1
                print(f"FAIL {name}: {err}", file=sys.stderr)
                continue
            lock[name] = sha
            pin = pins.get(name, "")
            note = "" if not pin or sha.startswith(pin) else f" (manifest has {pin})"
            print(f"ok   {name} {sha[:12]}{note}")
    lock_path.write_text(json.dumps(lock, indent=2, sort_keys=True) + "\n")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
