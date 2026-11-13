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
# env:   VERIFY_KEEP=1 keeps the temp copies; VERIFY_VERBOSE=1 prints the check output;
#        VERIFY_TIMEOUT=SECONDS caps every single check run (default 150). A check that
#        runs over is killed (with its child processes) and the task FAILs.
#
# Besides the original/reference pair, every task is also run against:
#   - "tests deleted": when checks/data/<id>/ holds the project's pristine tests, the copy
#     with those files removed must still FAIL (the check restores them itself);
#   - checks/cheats/<id>/<name>/ overlays on the untouched task: the check must FAIL;
#   - checks/alt/<id>/<name>/ overlays on the untouched task (other correct solutions):
#     the check must PASS.
# These print as "OK    <id>/<name>" lines, not as task lines.
#
# A reference directory may hold a `.remove` file: one path per line, removed from the
# copy before the overlay (for tasks whose solution renames files).
# A memory task may have checks/<id>.facts: literal strings that must appear in its
# memory fixture and must NOT appear in the working directory or in the prompt.

suite=$(cd "$(dirname "$0")" && pwd)
tasks="$suite/tasks.toml"
failures=0
integrity_check=1
passed=0
skipped=0
only="$*"

[ -f "$tasks" ] || { echo "FAIL: $tasks is missing"; exit 1; }

work=$(mktemp -d)
cleanup() { chmod -R u+w "$work" 2>/dev/null; [ -n "$VERIFY_KEEP" ] || rm -rf "$work"; }
trap cleanup EXIT

# T20: a hash of checks/ and reference/ (the directories tasks.toml declares in [eval] integrity)
# before the run and after every check; any change fails the run ("suite modified during the run").
integrity_hash() {
  python3 -I -B -c '
import hashlib, os, sys
root, h = sys.argv[1], hashlib.sha256()
for d in sys.argv[2:]:
    for dirpath, dirnames, filenames in os.walk(os.path.join(root, d)):
        dirnames.sort()
        for f in sorted(filenames):
            p = os.path.join(dirpath, f)
            h.update(os.path.relpath(p, root).encode() + b"\0")
            try:
                h.update(hashlib.sha256(open(p, "rb").read()).digest())
            except OSError:
                h.update(b"unreadable")
print(h.hexdigest())
' "$suite" checks reference
}
integrity_base=$(integrity_hash)
mkdir -p "$work/tmp"

# T18: checks/specs/ is generated from checks/spec-src/ (plaintext, maintainers only)
if ! python3 -I -B "$suite/checks/lib/make_specs.py" --check >"$work/specs.out" 2>&1; then
  echo "FAIL  $(cat "$work/specs.out")"
  failures=$((failures + 1))
fi

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

# T11. Each check (and its watchdog) runs as the leader of its own process group, started with
# perl's setpgrp or python's os.setpgrp (job control `set -m` is unavailable in a script without a
# terminal, e.g. dash in a container). A timeout kills the check's group; when the check ends first
# the watchdog's group is killed and the check's group is swept for background leftovers, so a
# check that starts sleepers leaves nothing running. Without perl and python3 it falls back to
# killing the process tree (best effort).
if command -v perl >/dev/null 2>&1; then
  grouped() { exec perl -e 'setpgrp(0, 0); exec @ARGV or die "exec: $!"' "$@"; }
elif command -v python3 >/dev/null 2>&1; then
  grouped() { exec python3 -c 'import os, sys; os.setpgrp(); os.execvp(sys.argv[1], sys.argv[1:])' "$@"; }
else
  grouped() { exec "$@"; }
fi

kill_group() { # leader pid: its whole group, then the pid itself
  kill -KILL -- "-$1" 2>/dev/null || /bin/kill -KILL -- "-$1" 2>/dev/null || true
  kill -KILL "$1" 2>/dev/null || true
  # fallback without groups: its children
  for child in $(pgrep -P "$1" 2>/dev/null); do kill_group "$child"; done
}

run_check() { # dir command
  rm -f "$work/timed_out"
  ( cd "$1" && EVAL_SUITE_DIR="$suite" TMPDIR="$work/tmp" grouped sh -c "$2" ) >"$work/check.out" 2>&1 </dev/null &
  check_pid=$!
  (
    grouped sh -c 'sleep "$1"; : >"$2"; kill -KILL -- "-$3" 2>/dev/null || /bin/kill -KILL -- "-$3" 2>/dev/null; kill -KILL "$3" 2>/dev/null' \
      watchdog "${VERIFY_TIMEOUT:-150}" "$work/timed_out" "$check_pid"
  ) >/dev/null 2>&1 &
  watchdog_pid=$!
  wait "$check_pid" 2>/dev/null
  status=$?
  kill_group "$watchdog_pid"
  wait "$watchdog_pid" 2>/dev/null
  kill_group "$check_pid"
  if [ -n "$integrity_check" ] && [ "$(integrity_hash)" != "$integrity_base" ]; then
    : >"$work/tampered"
    echo "suite modified during the run" >>"$work/check.out"
    return 125
  fi
  if [ -f "$work/timed_out" ]; then
    echo "the check did not finish within ${VERIFY_TIMEOUT:-150}s and was killed" >>"$work/check.out"
    return 124
  fi
  return "$status"
}

timed_out() { [ -f "$work/timed_out" ]; }

fresh_copy() { # (re)creates $copy as the untouched working dir
  rm -rf "$copy"
  mkdir -p "$copy"
  cp -R "$suite/$wd/." "$copy/"
}

overlay() { # dir: apply a solution (its .remove list first) on $copy
  if [ -f "$1/.remove" ]; then
    while IFS= read -r path; do
      [ -n "$path" ] && rm -f "$copy/$path"
    done <"$1/.remove"
  fi
  cp -R "$1/." "$copy/"
  rm -f "$copy/.remove"
}

# T11 self-test: a check that starts background sleepers must leave nothing running, both when
# it is killed by the timeout and when it ends by itself.
leftovers() { ps -axo command 2>/dev/null | grep -c '[s]leep 31[123]'; }
if [ -z "$only" ]; then
  mkdir -p "$work/wd-selftest"
  VERIFY_TIMEOUT=2 run_check "$work/wd-selftest" '( sleep 311 ) & sh -c "sleep 312" & sleep 313'
  if ! timed_out; then
    echo "FAIL  watchdog: a hanging check was not killed by the timeout"
    echo "x" >>"$work/failed"
  elif [ "$(leftovers)" != 0 ]; then
    echo "FAIL  watchdog: $(leftovers) sleeper(s) left running after a timeout"
    echo "x" >>"$work/failed"
  else
    echo "OK    watchdog: a timed-out check leaves nothing running"
  fi
  VERIFY_TIMEOUT=30 run_check "$work/wd-selftest" '( sleep 311 ) & sh -c "sleep 312" & exit 0'
  if [ "$(leftovers)" != 0 ]; then
    echo "FAIL  watchdog: $(leftovers) background process(es) left after a check ended"
    echo "x" >>"$work/failed"
  else
    echo "OK    watchdog: a finished check leaves no background process"
  fi
fi

# T25: the suite must load under `hone eval` (including `[eval] integrity = ["checks", "reference"]`);
# `--check` validates without running anything. The binary is $HONE_BIN when set; otherwise `hone`
# on PATH, but only when it supports `eval --check` (an older hone on PATH is skipped with a NOTE).
if [ -z "$only" ]; then
  hone_bin=""
  hone_note=""
  if [ -n "$HONE_BIN" ]; then
    if [ -x "$HONE_BIN" ]; then
      hone_bin=$HONE_BIN
    else
      echo "FAIL  HONE_BIN=$HONE_BIN is not an executable file"
      echo "x" >>"$work/failed"
    fi
  elif command -v hone >/dev/null 2>&1; then
    found=$(command -v hone)
    if "$found" eval --help 2>/dev/null | grep -q -- "--check"; then
      hone_bin=$found
    else
      hone_note="NOTE  $found has no \`hone eval --check\` (an older build): skipping the hone eval --check test; set HONE_BIN to a current build"
    fi
  else
    hone_note="NOTE  hone is not on PATH (and HONE_BIN is not set): skipping the hone eval --check test"
  fi
  if [ -n "$hone_bin" ]; then
    if (cd "$work" && "$hone_bin" eval "$tasks" --check) >"$work/hone-check.out" 2>&1; then
      echo "OK    hone eval --check accepts the suite ($(tail -1 "$work/hone-check.out"))"
    elif grep -qE "needs an API key|no model:|requires a provider" "$work/hone-check.out"; then
      # a provider-setup problem of this machine (no key / no default model), not of the suite
      echo "NOTE  hone eval --check skipped: no provider is configured ($(tail -1 "$work/hone-check.out" | cut -c1-100))"
    else
      echo "FAIL  hone eval --check rejects the suite: $(tail -3 "$work/hone-check.out" | tr '\n' ' ')"
      echo "x" >>"$work/failed"
    fi
  elif [ -n "$hone_note" ]; then
    echo "$hone_note"
  fi
fi

# T24 self-test: the launcher's sweep removes a stale read-only bundle and keeps a fresh one
if [ -z "$only" ]; then
  mkdir -p "$work/tmp/evalchk.stale/checks" "$work/tmp/evalchk.fresh"
  touch -t 200001010000 "$work/tmp/evalchk.stale"
  chmod -R a-w "$work/tmp/evalchk.stale"
  python3 -I -B "$suite/checks/lib/sweep.py" "$work/tmp"
  if [ -e "$work/tmp/evalchk.stale" ] || [ ! -d "$work/tmp/evalchk.fresh" ]; then
    echo "FAIL  sweep: a stale bundle was kept or a fresh one removed"
    echo "x" >>"$work/failed"
  else
    echo "OK    sweep: a stale read-only bundle is removed, a fresh one is kept"
  fi
  chmod -R u+w "$work/tmp/evalchk.fresh"
  rmdir "$work/tmp/evalchk.fresh" 2>/dev/null || true
fi

# T20 self-test: a check that modifies the suite must be detected (the file is restored at once).
if [ -z "$only" ]; then
  cp "$suite/checks/lib/judge.py" "$work/judge.bak"
  run_check "$work/wd-selftest" "echo '# tampered' >> \"$suite/checks/lib/judge.py\""
  cp "$work/judge.bak" "$suite/checks/lib/judge.py"
  if [ -f "$work/tampered" ]; then
    echo "OK    integrity: a check that rewrites the suite is detected"
  else
    echo "FAIL  integrity: a change to checks/lib/judge.py went unnoticed"
    echo "x" >>"$work/failed"
  fi
  rm -f "$work/tampered"
fi

printf '%s\n' "$list" | while IFS="$US" read -r id wd chk fx prompt; do
  [ -n "$id" ] || continue
  if [ -f "$work/tampered" ]; then
    echo "INFRA $id: suite modified during the run"
    echo "x" >>"$work/failed"
    continue
  fi
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
  fresh_copy

  if run_check "$copy" "$chk"; then
    echo "FAIL  $id: the check passes on the untouched task"
    [ -n "$VERIFY_VERBOSE" ] && sed 's/^/        /' "$work/check.out"
    echo "x" >>"$work/failed"
    continue
  fi
  if timed_out; then
    echo "FAIL  $id: the check timed out on the untouched task"
    echo "x" >>"$work/failed"
    continue
  fi
  before=$(tail -3 "$work/check.out" | tr '\n' ' ' | cut -c1-110)

  overlay "$ref"

  if run_check "$copy" "$chk"; then
    echo "PASS  $id   (fails before: $before)"
    echo "x" >>"$work/passed"
  else
    echo "FAIL  $id: the check fails with the reference solution"
    sed 's/^/        /' "$work/check.out" | tail -15
    echo "x" >>"$work/failed"
    continue
  fi

  # Cheats must fail, other correct solutions must pass.
  extra_fail() { echo "FAIL  $id/$1: $2"; echo "x" >>"$work/failed"; }
  pristine="$suite/checks/data/$id"
  if [ -d "$pristine" ]; then
    fresh_copy
    (cd "$pristine" && find . -type f) | while IFS= read -r f; do rm -f "$copy/$f"; done
    if run_check "$copy" "$chk" && ! timed_out; then
      extra_fail tests-deleted "the check passes when the project's tests are deleted"
    else
      echo "OK    $id/tests-deleted (fails as it should)"
    fi
  fi
  for dir in "$suite/checks/cheats/$id"/*/; do
    [ -d "$dir" ] || continue
    name=$(basename "$dir")
    fresh_copy
    overlay "$dir"
    started=$(date +%s)
    if run_check "$copy" "$chk" || timed_out; then
      extra_fail "$name" "the check accepts this cheat"
    elif [ $(( $(date +%s) - started )) -gt 60 ]; then
      extra_fail "$name" "the cheat held the check up for $(( $(date +%s) - started )) s"
    else
      echo "OK    $id/$name (cheat rejected)"
    fi
    pkill -f 'sleep 33[0-9]' 2>/dev/null || true
  done
  for dir in "$suite/checks/alt/$id"/*/; do
    [ -d "$dir" ] || continue
    name=$(basename "$dir")
    fresh_copy
    overlay "$dir"
    if run_check "$copy" "$chk"; then
      echo "OK    $id/$name (alternative solution accepted)"
    else
      extra_fail "$name" "the check rejects a correct alternative solution"
      sed 's/^/        /' "$work/check.out" | tail -10
    fi
  done
done

if [ -f "$work/tampered" ]; then
  echo "FAIL  suite modified during the run"
  echo "x" >>"$work/failed"
fi

# T15: no cheat may have written into the suite (the "rewrite check files" cheat drops a PWNED
# marker wherever it can find the suite path)
if find "$suite" -name PWNED 2>/dev/null | grep -q .; then
  echo "FAIL  a cheat wrote into the suite: $(find "$suite" -name PWNED | head -1)"
  echo "x" >>"$work/failed"
else
  echo "OK    no cheat could write into the suite"
fi

# the loop above runs in a subshell: count through marker files
count() { [ -f "$work/$1" ] && wc -l <"$work/$1" | tr -d ' ' || echo 0; }
failed_n=$(count failed)
passed=$(count passed)
skipped=$(count skipped)
failures=$((failures + failed_n))

echo
echo "starter suite: $passed passed, $failures failed, $skipped skipped"
[ "$failures" -eq 0 ]
