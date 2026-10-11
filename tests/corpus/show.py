#!/usr/bin/env python3
"""Print the diagnostics of a corpus run with their source line, for triage.

Usage: python3 tests/corpus/show.py PROJECT MODE DIALECT [CODE] [--classes] [--limit N]

--classes groups diagnostics by message with names replaced by '_' and prints one
example of each class.
"""

import json
import re
import sys
from collections import defaultdict

from corpus import OUT, WORK, project_root, projects


def source_root(p, mode, dialect):
    if mode in ("raw", "catalog"):
        return project_root(p)
    if mode == "resolved":
        return OUT / "resolved" / p["name"] / dialect
    work = WORK / p["name"] / p.get("root", ".") / p["dbt_project"]
    return work / "target" / "compiled"


def message_class(d):
    msg = re.sub(r"'[^']*'", "_", d["message"])
    msg = re.sub(r"\d+", "N", msg)
    return f"{d['code']} {msg}"


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    classes = "--classes" in sys.argv
    limit = 10**9
    if "--limit" in sys.argv:
        limit = int(sys.argv[sys.argv.index("--limit") + 1])
        args.remove(str(limit))
    name, mode, dialect = args[:3]
    code = args[3] if len(args) > 3 else None
    p = projects([name])[0]
    data = json.loads((OUT / "results" / name / mode / f"{dialect}.json").read_text())
    root = source_root(p, mode, dialect)
    groups = defaultdict(list)
    for f, ds in sorted(data.items()):
        for d in ds:
            if code and d["code"] != code:
                continue
            groups[message_class(d) if classes else len(groups)].append((f, d))
    shown = 0
    for key, items in sorted(groups.items(), key=lambda kv: -len(kv[1]) if classes else 0):
        f, d = items[0]
        try:
            line = (root / f).read_text(errors="replace").splitlines()[d["line"] - 1].strip()
        except (OSError, IndexError):
            line = "?"
        prefix = f"[{len(items)}x] " if classes else ""
        print(f"{prefix}{f}:{d['line']}:{d['column']} {d['code']} {d['message']}\n    | {line[:200]}")
        shown += 1
        if shown >= limit:
            break


if __name__ == "__main__":
    main()
