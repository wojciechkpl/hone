# requires: python3 node
# Success check for c04-js-fix-flatten (T13/T14 of docs/plans/eval-starter-suite-spec.md). Runs in the agent's copy.
# The verdict comes from lib/judge.py, a python3 -I parent that runs no agent code, launched from a
# private bundle without the suite path; agent code runs only in child processes (T13, T19).
set -e
. "$EVAL_SUITE_DIR/checks/lib/launch.sh"
launch_judge c04-js-fix-flatten
