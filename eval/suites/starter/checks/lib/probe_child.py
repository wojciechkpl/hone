"""Agent-code child for Python tasks. argv: work_dir module. stdin: JSON list of case INPUTS.

Prints one JSON list (one result per case) as its last line. It holds no expectations and
learns nothing about the suite. Everything this file needs is imported BEFORE the solution's
directory goes on sys.path (appended, so the standard library always wins).
"""
import datetime
import importlib
import json
import sys

work, module_name = sys.argv[1], sys.argv[2]
cases = json.loads(sys.stdin.read())
sys.path.append(work)


def decode(v):
    if isinstance(v, dict) and set(v) == {"$date"}:
        return datetime.date.fromisoformat(v["$date"])
    if isinstance(v, list):
        return [decode(x) for x in v]
    return v


PATCHES = {"minus1": lambda price: price - 1}


def m04(mod, name):
    calls, sleeps = [], []

    def run(fn):
        try:
            value = mod.with_retry(fn, sleep=sleeps.append)
            return {"value": value, "raised": None}
        except Exception as e:  # noqa: BLE001
            return {"value": None, "raised": str(e)}

    if name == "m04_always_fail":
        def boom():
            calls.append(1)
            raise RuntimeError(f"fail {len(calls)}")
        r = run(boom)
    elif name == "m04_flaky":
        def flaky():
            calls.append(1)
            if len(calls) < 2:
                raise ValueError("once")
            return "value"
        r = run(flaky)
    elif name == "m04_ok":
        def fine():
            calls.append(1)
            return 7
        r = run(fine)
    else:
        raise KeyError(name)
    return {"value": r["value"], "raised": r["raised"], "calls": len(calls), "sleeps": sleeps}


results = []
mod = None
load_error = None
try:
    mod = importlib.import_module(module_name)
except Exception as e:  # noqa: BLE001
    load_error = type(e).__name__
for case in cases:
    try:
        if load_error:
            raise RuntimeError(load_error)
        if "scenario" in case:
            results.append({"ok": json.loads(json.dumps(m04(mod, case["scenario"])))})
            continue
        patch = case.get("patch")
        original = None
        if patch:
            original = getattr(mod, patch["attr"])
            setattr(mod, patch["attr"], PATCHES[patch["with"]])
        try:
            args = decode(case.get("args", []))
            kwargs = case.get("kwargs", {})
            value = getattr(mod, case["fn"])(*args, **kwargs)
            if "select" in case:
                for key in case["select"]:
                    value = value[key]
            results.append({"ok": json.loads(json.dumps(value))})
        finally:
            if patch:
                setattr(mod, patch["attr"], original)
    except Exception as e:  # noqa: BLE001
        results.append({"raises": type(e).__name__})
sys.stdout.write("\n" + json.dumps(results) + "\n")
sys.stdout.flush()
