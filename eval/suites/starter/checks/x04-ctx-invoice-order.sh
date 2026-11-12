# requires: node python3
# Success check for x04-ctx-invoice-order. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
node -e "
const { orderInvoices } = require('./src/billing/export/order');
const assert = require('node:assert');
const input = [
  { id: 1, date: '2026-01-05' },
  { id: 2, date: '2026-03-01' },
  { id: 3, date: '2026-02-11' },
];
assert.deepStrictEqual(orderInvoices(input).map((i) => i.id), [2, 3, 1]);
assert.deepStrictEqual(input.map((i) => i.id), [1, 2, 3], 'the input must not be mutated');
console.log('ok');
" || exit 1
python3 "$EVAL_SUITE_DIR/checks/lib/manifest.py" "$EVAL_SUITE_DIR/checks/data/x04-ctx-invoice-order.manifest.json"
