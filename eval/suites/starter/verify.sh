#!/bin/sh
# Verifies the starter suite without a model (docs/plans/eval-starter-suite-spec.md, T4).
#
# For every task in tasks.toml: copy its working directory to a temp dir, run its
# success check (it must FAIL), overlay the reference solution, run the check again
# (it must PASS). One line per task. Exits non-zero if any task fails either
# condition, or if tasks.toml lists a task without a directory, check or reference,
# or a reference exists without a task. A task whose toolchain is missing (the
# `# requires:` line of its check) is reported as SKIP, never as PASS.
#
# usage: sh verify.sh [task-id ...]     (default: every task)
# env:   VERIFY_KEEP=1 keeps the temp copies; VERIFY_VERBOSE=1 prints the check output.
#
# A reference directory may hold a `.remove` file: one path per line, removed from the
# copy before the overlay (for tasks whose solution renames files).
# A memory task may have checks/<id>.facts: literal strings that must appear in its
# memory fixture and must NOT appear in the working directory or in the prompt.

suite=$(cd "$(dirname "$0")" && pwd)
tasks="$suite/tasks.toml"
failures=0
passed=0
skipped=0
only="$*"

[ -f "$tasks" ] || { echo "FAIL: $tasks is missing"; exit 1; }

work=$(mktemp -d)
cleanup() { [ -n "$VERIFY_KEEP" ] || rm -rf "$work"; }
trap cleanup EXIT

# id, working_dir, success_check, memory_fixture, prompt (separated by the unit separator, \037)
list=$(awk '
  function val(line,   v) { v = line; sub(/^[a-z_]+ = "/, "", v); sub(/"[ \t]*$/, "", v); gsub(/\\"/, "\"", v); return v }
  function flush() { if (id != "") printf "%s\037%s\037%s\037%s\037%s\n", id, wd, chk, fx, pr; id = wd = chk = fx = pr = "" }
  /^\[\[tasks\]\]/ { flush(); next }
  /^id = "/ { id = val($0) }
  /^working_dir = "/ { wd = val($0) }
  /^success_check = "/ { chk = val($0) }
  /^memory_fixture = "/ { fx = val($0) }
  /^prompt = "/ { pr = val($0) }
  END { flush() }
' "$tasks")

US=$(printf '\037')
ids=$(printf '%s\n' "$list" | cut -d"$US" -f1)

# references without a task
for ref in "$suite"/reference/*/; do
  [ -d "$ref" ] || continue
  name=$(basename "$ref")
  printf '%s\n' "$ids" | grep -qx "$name" || { echo "FAIL  reference/$name has no task in tasks.toml"; failures=$((failures + 1)); }
done

fail() { echo "FAIL  $1: $2"; failures=$((failures + 1)); }

run_check() { # dir command
  ( cd "$1" && EVAL_SUITE_DIR="$suite" sh -c "$2" ) >"$work/check.out" 2>&1
}

printf '%s\n' "$list" | while IFS="$US" read -r id wd chk fx prompt; do
  [ -n "$id" ] || continue
  if [ -n "$only" ]; then
    case " $only " in *" $id "*) ;; *) continue ;; esac
  fi
  script="$suite/checks/$id.sh"
  ref="$suite/reference/$id"
  problem=""
  [ -n "$wd" ] && [ -d "$suite/$wd" ] || problem="working directory '$wd' is missing"
  [ -z "$problem" ] && [ -n "$chk" ] || problem="${problem:-no success_check}"
  [ -z "$problem" ] && [ -f "$script" ] || problem="${problem:-checks/$id.sh is missing}"
  [ -z "$problem" ] && [ -d "$ref" ] || problem="${problem:-reference/$id is missing}"
  if [ -z "$problem" ] && [ -n "$fx" ]; then
    [ -f "$suite/$fx" ] || problem="memory fixture '$fx' is missing"
    if [ -z "$problem" ] && command -v python3 >/dev/null 2>&1; then
      python3 -c '
import json, sys
for n, line in enumerate(open(sys.argv[1], encoding="utf-8"), 1):
    if line.strip():
        m = json.loads(line)
        assert all(k in m for k in ("id", "kind", "content")), f"line {n}: needs id, kind, content"
' "$suite/$fx" 2>"$work/fx.err" || problem="memory fixture is not valid JSONL: $(tail -1 "$work/fx.err")"
    fi
  fi
  if [ -z "$problem" ] && [ -f "$suite/checks/$id.facts" ]; then
    while IFS= read -r fact; do
      [ -n "$fact" ] || continue
      grep -qF -- "$fact" "$suite/$fx" || problem="fact '$fact' is not in the fixture"
      grep -rqF -- "$fact" "$suite/$wd" && problem="fact '$fact' appears in the working directory"
      case "$prompt" in *"$fact"*) problem="fact '$fact' appears in the prompt" ;; esac
    done <"$suite/checks/$id.facts"
  fi
  if [ -n "$problem" ]; then
    echo "FAIL  $id: $problem"
    echo "x" >>"$work/failed"
    continue
  fi

  missing=""
  for tool in $(sed -n 's/^# requires: //p' "$script" | head -1); do
    command -v "$tool" >/dev/null 2>&1 || missing="$missing $tool"
  done
  if [ -n "$missing" ]; then
    echo "SKIP  $id (needs:$missing)"
    echo "x" >>"$work/skipped"
    continue
  fi

  copy="$work/$id"
  rm -rf "$copy"
  mkdir -p "$copy"
  cp -R "$suite/$wd/." "$copy/"

  if run_check "$copy" "$chk"; then
    echo "FAIL  $id: the check passes on the untouched task"
    [ -n "$VERIFY_VERBOSE" ] && sed 's/^/        /' "$work/check.out"
    echo "x" >>"$work/failed"
    continue
  fi
  before=$(tail -3 "$work/check.out" | tr '\n' ' ' | cut -c1-110)

  if [ -f "$ref/.remove" ]; then
    while IFS= read -r path; do
      [ -n "$path" ] && rm -f "$copy/$path"
    done <"$ref/.remove"
  fi
  cp -R "$ref/." "$copy/"
  rm -f "$copy/.remove"

  if run_check "$copy" "$chk"; then
    echo "PASS  $id   (fails before: $before)"
    echo "x" >>"$work/passed"
  else
    echo "FAIL  $id: the check fails with the reference solution"
    sed 's/^/        /' "$work/check.out" | tail -15
    echo "x" >>"$work/failed"
  fi
done

# the loop above runs in a subshell: count through marker files
count() { [ -f "$work/$1" ] && wc -l <"$work/$1" | tr -d ' ' || echo 0; }
failed_n=$(count failed)
passed=$(count passed)
skipped=$(count skipped)
failures=$((failures + failed_n))

echo
echo "starter suite: $passed passed, $failures failed, $skipped skipped"
[ "$failures" -eq 0 ]
