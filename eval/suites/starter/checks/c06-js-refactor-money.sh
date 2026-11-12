# requires: node
# Success check for c06-js-refactor-money. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
node --test 2>&1 || exit 1
grep -q 'function formatMoney' report.js || { echo "formatMoney(amount) is missing"; exit 1; }
[ "$(grep -c 'toFixed' report.js)" -eq 1 ] || { echo "the formatting must live in one place"; exit 1; }
