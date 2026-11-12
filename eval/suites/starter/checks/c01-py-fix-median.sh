# requires: python3
# Success check for c01-py-fix-median. Runs in the task's working-directory copy (the agent's files).
# T7-T9: only the allowlisted solution files are copied into a fresh directory next to the
# pristine tests and project files (checks/data/c01-py-fix-median/); the tests run there, isolated, and
# a held-out probe in a separate process imports only the solution.
set -e
. "$EVAL_SUITE_DIR/checks/lib/fresh.sh"
fresh_begin c01-py-fix-median
fresh_take stats.py
(cd "$FRESH" && py_isolated -m unittest discover -s . -t . -p 'test_*.py' -q) 2>&1 || exit 1
probe_python stats c01-py-fix-median.json || exit 1
