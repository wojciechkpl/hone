# requires: node python3
# Success check for x02-ctx-long-names. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
node -e "
const { formatPlayerName } = require('./src/leaderboard/format');
const assert = require('node:assert');
assert.strictEqual(formatPlayerName('Bartholomew Jones'), 'Bartholomew…');
assert.strictEqual(formatPlayerName('Bartholomew Jones').length, 12);
assert.strictEqual(formatPlayerName('Twelve chars'), 'Twelve chars');
assert.strictEqual(formatPlayerName('Ana'), 'Ana');
console.log('ok');
" || exit 1
python3 "$EVAL_SUITE_DIR/checks/lib/manifest.py" "$EVAL_SUITE_DIR/checks/data/x02-ctx-long-names.manifest.json"
