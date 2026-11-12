# requires: cargo
# Success check for c08-rs-add-roman. Runs in the task's working-directory copy (the agent's files).
# T7-T9: only the allowlisted solution files are copied into a fresh directory next to the
# pristine tests and project files (checks/data/c08-rs-add-roman/); the tests run there, isolated, and
# a held-out probe in a separate process imports only the solution.
set -e
. "$EVAL_SUITE_DIR/checks/lib/fresh.sh"
fresh_begin c08-rs-add-roman
fresh_take_tree src '*.rs'
cp "$SUITE/checks/probes/c08-rs-add-roman.heldout.rs" "$FRESH/tests/heldout.rs"
(cd "$FRESH" && CARGO_TARGET_DIR="$FRESH_AUX/target" cargo test --offline -q) 2>&1
