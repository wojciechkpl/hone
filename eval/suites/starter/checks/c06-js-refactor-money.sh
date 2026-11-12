# requires: python3 node
# Success check for c06-js-refactor-money (T13/T14 of docs/plans/eval-starter-suite-spec.md). Runs in the agent's copy.
# The verdict comes from lib/judge.py, a python3 -I parent that runs no agent code: agent code runs
# only in child processes that read the case inputs on stdin; the expectations stay in the parent.
set -e
exec python3 -I -B "$EVAL_SUITE_DIR/checks/lib/judge.py" c06-js-refactor-money
