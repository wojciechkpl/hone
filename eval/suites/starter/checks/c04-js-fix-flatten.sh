# requires: node
# Success check for c04-js-fix-flatten. Runs in the task's working-directory copy.
# The project's own tests are restored from checks/data/c04-js-fix-flatten/ before they run, so
# editing, weakening or deleting them gets no credit.
set -e
export PYTHONDONTWRITEBYTECODE=1
cp -R "$EVAL_SUITE_DIR/checks/data/c04-js-fix-flatten/." .
node --test 2>&1
