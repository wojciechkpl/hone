// Agent-code child for JavaScript tasks. argv: work_dir module-path. stdin: JSON list of case
// INPUTS. Prints one JSON list (one result per case) as its last line. It holds no expectations
// and learns nothing about the suite.
const fs = require('node:fs');
const path = require('node:path');
const [work, modulePath] = process.argv.slice(2);
const cases = JSON.parse(fs.readFileSync(0, 'utf8'));
const file = path.join(work, modulePath);

function loadPatched(attr) {
  // Replace the module-level binding `attr` at run time: `const` becomes `let`, and a setter is
  // appended to the module's own scope.
  let src = fs.readFileSync(file, 'utf8');
  const re = new RegExp(`(function\\s+${attr}\\s*\\(|\\b(?:const|let|var)\\s+${attr}\\s*=)`);
  if (!re.test(src)) throw Object.assign(new Error('no binding'), { name: 'NoBinding' });
  src = src.replace(new RegExp(`\\bconst\\s+${attr}\\b`), `let ${attr}`);
  src += `\n;module.exports.__swap = (f) => { ${attr} = f; };\n`;
  const m = { exports: {} };
  new Function('module', 'exports', 'require', src)(m, m.exports, require);
  return m.exports;
}

const WITH = { tag: (a) => 'X<' + a + '>' };
const results = [];
let loaded;
let loadError = null;
try {
  loaded = require(file);
} catch (e) {
  loadError = e;
}
for (const c of cases) {
  try {
    if (loadError) throw loadError;
    let mod = loaded;
    if (c.patch) {
      mod = loadPatched(c.patch.attr);
      mod.__swap(WITH[c.patch.with]);
    }
    const args = c.args || [];
    const value = mod[c.fn](...args);
    const r = { ok: JSON.parse(JSON.stringify(value === undefined ? null : value)) };
    r.args_after = JSON.parse(JSON.stringify(args));
    r.same0 = args.length > 0 && value === args[0];
    results.push(r);
  } catch (e) {
    results.push({ raises: e && e.name ? e.name : 'Error' });
  }
}
process.stdout.write('\n' + JSON.stringify(results) + '\n');
