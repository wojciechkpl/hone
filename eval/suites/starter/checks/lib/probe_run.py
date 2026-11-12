"""Held-out probe runner (Python solutions). argv: fresh_dir module cases.json out.json

Imports ONLY the solution module from fresh_dir, calls each case and writes the results to
out.json; it compares nothing itself (probe_compare.py does, in another process).
"""
import json
import sys

fresh, module, cases_path, out_path = sys.argv[1:5]
sys.path.insert(0, fresh)
cases = json.load(open(cases_path))["cases"]
mod = __import__(module)
results = []
for case in cases:
    try:
        value = getattr(mod, case["fn"])(*case["args"])
        results.append({"ok": json.loads(json.dumps(value))})
    except Exception as e:  # noqa: BLE001 - any failure is a result
        results.append({"raises": type(e).__name__})
with open(out_path, "w") as f:
    json.dump(results, f)
