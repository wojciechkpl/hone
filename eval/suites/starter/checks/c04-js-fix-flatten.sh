# requires: node
# Success check for c04-js-fix-flatten. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
node --test 2>&1
