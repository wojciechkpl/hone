# requires: cargo
# Success check for c08-rs-add-roman. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
target="$(mktemp -d)"
trap 'rm -rf "$target"' EXIT
CARGO_TARGET_DIR="$target" cargo test --offline -q 2>&1
