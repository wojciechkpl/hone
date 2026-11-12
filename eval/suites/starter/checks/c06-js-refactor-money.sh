# requires: node
# Success check for c06-js-refactor-money. Runs in the task's working-directory copy (the agent's files).
# T7-T9: only the allowlisted solution files are copied into a fresh directory next to the
# pristine tests and project files (checks/data/c06-js-refactor-money/); the tests run there, isolated, and
# a held-out probe in a separate process imports only the solution.
set -e
. "$EVAL_SUITE_DIR/checks/lib/fresh.sh"
fresh_begin c06-js-refactor-money
fresh_take report.js
(cd "$FRESH" && node --test) 2>&1 || exit 1
(cd "$FRESH" && node - <<'JS'
const fs = require('node:fs');
let src = fs.readFileSync('report.js', 'utf8');
if (!/(function\s+formatMoney\s*\(|\b(?:const|let|var)\s+formatMoney\s*=)/.test(src)) {
  console.log('formatMoney(amount) is missing');
  process.exit(1);
}
src = src.replace(/\bconst\s+formatMoney\b/, 'let formatMoney');
src += '\n;module.exports.__swap = (f) => { formatMoney = f; };\n';
const module_ = { exports: {} };
new Function('module', 'exports', 'require', src)(module_, module_.exports, require);
const m = module_.exports;
if (typeof m.__swap !== 'function') { console.log('could not swap formatMoney'); process.exit(1); }
m.__swap((amount) => 'X<' + amount + '>');
const probes = [
  ['formatPrice', m.formatPrice(3), 'X<3>'],
  ['formatRefund', m.formatRefund(2.5), 'X<2.5>'],
  ['formatTotal', m.formatTotal([1, 2.25]), 'X<3.25>'],
];
const bad = probes.filter(([, got, want]) => !String(got).includes(want));
if (bad.length) {
  for (const [n, got, want] of bad) console.log(`${n} returned ${got}; it must use formatMoney (expected to contain ${want})`);
  process.exit(1);
}
console.log('ok');
JS
)
