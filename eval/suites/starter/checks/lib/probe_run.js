// Held-out probe runner (JavaScript solutions). argv: module-file cases.json out.json
// Requires ONLY the solution, calls each case and writes the results; probe_compare.py compares.
const fs = require('node:fs');
const [modFile, casesPath, outPath] = process.argv.slice(2);
const cases = JSON.parse(fs.readFileSync(casesPath, 'utf8')).cases;
const mod = require(modFile);
const results = [];
for (const c of cases) {
  try {
    const value = mod[c.fn](...c.args);
    results.push({ ok: JSON.parse(JSON.stringify(value === undefined ? null : value)) });
  } catch (e) {
    results.push({ raises: e && e.name ? e.name : 'Error' });
  }
}
fs.writeFileSync(outPath, JSON.stringify(results));
