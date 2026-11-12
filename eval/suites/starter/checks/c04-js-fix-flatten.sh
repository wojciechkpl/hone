# requires: node
# Success check for c04-js-fix-flatten. Runs in the task's working-directory copy (the agent's files).
# T7-T9: only the allowlisted solution files are copied into a fresh directory next to the
# pristine tests and project files (checks/data/c04-js-fix-flatten/); the tests run there, isolated, and
# a held-out probe in a separate process imports only the solution.
set -e
. "$EVAL_SUITE_DIR/checks/lib/fresh.sh"
fresh_begin c04-js-fix-flatten
fresh_take flatten.js
(cd "$FRESH" && node --test) 2>&1 || exit 1
probe_node flatten.js c04-js-fix-flatten.json || exit 1
