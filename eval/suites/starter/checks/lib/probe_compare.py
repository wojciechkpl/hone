"""Compares probe results with the expectations. argv: cases.json results.json"""
import json
import sys

cases = json.load(open(sys.argv[1]))["cases"]
try:
    results = json.load(open(sys.argv[2]))
except (OSError, ValueError):
    print("probe: no results were produced")
    sys.exit(1)
if len(results) != len(cases):
    print(f"probe: {len(results)} results for {len(cases)} cases")
    sys.exit(1)
bad = []
for case, got in zip(cases, results):
    want = {"raises": case["raises"]} if "raises" in case else {"ok": case["expect"]}
    if got != want:
        bad.append(f"{case['fn']}({json.dumps(case['args'])[1:-1]}): expected {want}, got {got}")
if bad:
    print("held-out probe failed:\n  " + "\n  ".join(bad))
    sys.exit(1)
print("probe ok")
