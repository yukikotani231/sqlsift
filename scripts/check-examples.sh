#!/usr/bin/env bash
# Runs sqlsift on every example project in examples/ and checks the result:
#
# 1. `sqlsift check` in the example's directory (its sqlsift.toml) must pass
#    with no diagnostics at all, warnings included.
# 2. `sqlsift check broken/...` must fail and report exactly the diagnostics
#    listed in the example's expected-diagnostics.txt, one
#    `<file>:<line>:<column> <code>` per line (`#` starts a comment).
#
# Usage: scripts/check-examples.sh [example ...]
# The binary is $SQLSIFT, or target/debug/sqlsift (run `cargo build` first).

set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
sqlsift="${SQLSIFT:-$root/target/debug/sqlsift}"

if [[ ! -x "$sqlsift" ]]; then
  echo "sqlsift binary not found at $sqlsift (run \`cargo build\` or set SQLSIFT)" >&2
  exit 2
fi
if ! command -v jq >/dev/null; then
  echo "jq is required" >&2
  exit 2
fi

# Prints one `<file>:<line>:<column> <code>` line per diagnostic of a JSON report.
diagnostics() {
  jq -r '.files[] | .file as $f | .diagnostics[] | "\($f):\(.line):\(.column) \(.code)"' | sort
}

if [[ $# -gt 0 ]]; then
  examples=("$@")
else
  examples=()
  for dir in "$root"/examples/*/; do
    examples+=("$(basename "$dir")")
  done
fi

failed=0
for name in "${examples[@]}"; do
  dir="$root/examples/$name"
  echo "== examples/$name"
  if [[ ! -f "$dir/sqlsift.toml" ]]; then
    echo "  FAIL: no sqlsift.toml"
    failed=1
    continue
  fi
  cd "$dir" || exit 2

  # 1. The project as configured is clean.
  report="$("$sqlsift" check --quiet --format json)"
  status=$?
  found="$(diagnostics <<<"$report")"
  if [[ $status -ne 0 || -n "$found" ]]; then
    echo "  FAIL: \`sqlsift check\` exited with $status and reported:"
    sed 's/^/    /' <<<"${found:-<no diagnostics>}"
    failed=1
  else
    echo "  ok: sqlsift check is clean"
  fi

  # 2. The intentionally broken files report exactly the expected diagnostics.
  if [[ -d broken ]]; then
    mapfile -t broken_files < <(find broken -type f | sort)
    report="$("$sqlsift" check --quiet --format json "${broken_files[@]}")"
    status=$?
    found="$(diagnostics <<<"$report")"
    expected="$(grep -v -e '^#' -e '^[[:space:]]*$' expected-diagnostics.txt 2>/dev/null | sort)"
    if [[ $status -ne 1 || "$found" != "$expected" ]]; then
      echo "  FAIL: \`sqlsift check ${broken_files[*]}\` exited with $status; diff (expected vs found):"
      diff <(echo "$expected") <(echo "$found") | sed 's/^/    /'
      failed=1
    else
      echo "  ok: broken/ reports $(wc -l <<<"$found") expected diagnostic(s)"
    fi
  fi
done

if [[ $failed -ne 0 ]]; then
  echo "Some examples failed. Update the example, or its expected-diagnostics.txt and README if the change is intended."
  exit 1
fi
echo "All examples passed."
