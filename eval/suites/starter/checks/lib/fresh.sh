# Sourced by the coding checks (c01-c08), T7/T8 of docs/plans/eval-starter-suite-spec.md.
#
#   . "$EVAL_SUITE_DIR/checks/lib/fresh.sh"
#   fresh_begin <task-id>        # FRESH = new temp dir holding the pristine tests/project files
#   fresh_take stats.py          # copy regular files from the agent's copy (cwd); symlinks skipped
#   fresh_take_tree src '*.rs'   # same for every matching regular file under a directory
#
# Only the files an allowlist names ever reach FRESH, so nothing else the agent left in its copy
# (shadow modules, __pycache__, .cargo/, build.rs, symlinks, ...) takes part in the run.
# Before any agent code runs, EVAL_SUITE_DIR and the interpreter hooks are removed from the
# environment (SUITE keeps the suite path for the check's own use).

SUITE=$EVAL_SUITE_DIR
unset EVAL_SUITE_DIR NODE_OPTIONS NODE_PATH PYTHONPATH PYTHONSTARTUP PYTHONHOME PYTHONUSERBASE \
  PYTHONPYCACHEPREFIX PYTHONSAFEPATH RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER RUSTFLAGS \
  CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_RUSTFLAGS CARGO_BUILD_RUSTC_WRAPPER CARGO_BUILD_TARGET \
  LD_PRELOAD DYLD_INSERT_LIBRARIES
export PYTHONDONTWRITEBYTECODE=1

fresh_begin() { # task id
  FRESH_ID=$1
  FRESH=$(mktemp -d)
  FRESH_AUX=$(mktemp -d)
  trap 'rm -rf "$FRESH" "$FRESH_AUX"' EXIT
  [ -d "$SUITE/checks/data/$FRESH_ID" ] || { echo "no pristine data for $FRESH_ID"; exit 1; }
  cp -R "$SUITE/checks/data/$FRESH_ID/." "$FRESH/"
}

fresh_take() {
  for p in "$@"; do
    if [ -f "$p" ] && [ ! -L "$p" ]; then
      mkdir -p "$FRESH/$(dirname "$p")"
      cp "$p" "$FRESH/$p"
    fi
  done
}

fresh_take_tree() { # dir, name pattern
  [ -d "$1" ] && [ ! -L "$1" ] || return 0
  find "$1" -type f -name "$2" | while IFS= read -r f; do
    mkdir -p "$FRESH/$(dirname "$f")"
    cp "$f" "$FRESH/$f"
  done
}

# Python: isolated (-I: no cwd, no environment, no user site), fresh bytecode cache prefix.
py_isolated() {
  python3 -I -B -X "pycache_prefix=$FRESH_AUX/pyc" "$@"
}

# Out-of-process probe with held-out inputs (T9): the solution is only imported by a separate
# interpreter that writes its results to a file; a third process compares them.
#   probe_python <module> <cases.json>      probe_node <module-file> <cases.json>
probe_python() {
  py_isolated "$SUITE/checks/lib/probe_run.py" "$FRESH" "$1" "$SUITE/checks/probes/$2" "$FRESH_AUX/probe.json" \
    || { echo "probe: the solution could not be run"; return 1; }
  python3 -I -B "$SUITE/checks/lib/probe_compare.py" "$SUITE/checks/probes/$2" "$FRESH_AUX/probe.json"
}

probe_node() {
  (cd "$FRESH" && node "$SUITE/checks/lib/probe_run.js" "$FRESH/$1" "$SUITE/checks/probes/$2" "$FRESH_AUX/probe.json") \
    || { echo "probe: the solution could not be run"; return 1; }
  python3 -I -B "$SUITE/checks/lib/probe_compare.py" "$SUITE/checks/probes/$2" "$FRESH_AUX/probe.json"
}
