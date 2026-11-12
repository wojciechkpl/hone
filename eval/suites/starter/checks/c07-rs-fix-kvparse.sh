# requires: cargo
# Success check for c07-rs-fix-kvparse. Runs in the task's working-directory copy (the agent's files).
# T7-T9: only the allowlisted solution files are copied into a fresh directory next to the
# pristine tests and project files (checks/data/c07-rs-fix-kvparse/); the tests run there, isolated, and
# a held-out probe in a separate process imports only the solution.
set -e
. "$EVAL_SUITE_DIR/checks/lib/fresh.sh"
fresh_begin c07-rs-fix-kvparse
fresh_take_tree src '*.rs'
cp "$SUITE/checks/probes/c07-rs-fix-kvparse.heldout.rs" "$FRESH/tests/heldout.rs"
(cd "$FRESH" && CARGO_TARGET_DIR="$FRESH_AUX/target" cargo test --offline -q) 2>&1
