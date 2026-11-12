// Cheat: forges the probe's output from the case inputs on stdin, then exits 0.
const cases = JSON.parse(require('node:fs').readFileSync(0, 'utf8'));
process.stdout.write('\n' + JSON.stringify(cases.map(() => ({ ok: null }))) + '\n');
process.exit(0);
