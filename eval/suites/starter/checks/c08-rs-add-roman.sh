# requires: cargo
# Success check for c08-rs-add-roman. Runs in the task's working-directory copy.
# The project's own tests are restored from checks/data/c08-rs-add-roman/ before they run, so
# editing, weakening or deleting them gets no credit.
set -e
export PYTHONDONTWRITEBYTECODE=1
cp -R "$EVAL_SUITE_DIR/checks/data/c08-rs-add-roman/." .
target="$(mktemp -d)"
trap 'rm -rf "$target"' EXIT
CARGO_TARGET_DIR="$target" cargo test --offline -q 2>&1
