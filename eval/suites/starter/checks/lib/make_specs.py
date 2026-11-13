"""Builds checks/specs/<id>.json from checks/spec-src/<id>.json (T18).

The specs the judge reads hold, per case, only the inputs, the names of the result fields that
are compared (`keys`), a per-case salt and the salted SHA-256 of the canonical expected result:
reading a spec reveals no answer. spec-src/ holds the plaintext and is only for maintainers.
Salts are derived from the task id and the case number, so the output is reproducible.

usage: python3 make_specs.py            write checks/specs/
       python3 make_specs.py --check    fail when checks/specs/ is out of date
"""
import hashlib
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CHECKS = os.path.dirname(HERE)


def canon(value):
    """Canonical JSON: sorted keys, no spaces, integral floats as ints (15 == 15.0)."""
    def norm(v):
        if isinstance(v, float) and v == int(v) and abs(v) < 1e15:
            return int(v)
        if isinstance(v, list):
            return [norm(x) for x in v]
        if isinstance(v, dict):
            return {k: norm(x) for k, x in v.items()}
        return v
    return json.dumps(norm(value), sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def case_hash(salt, projection):
    return hashlib.sha256((salt + canon(projection)).encode("utf-8")).hexdigest()


def build(task, spec):
    out = dict(spec)
    cases = []
    for i, case in enumerate(spec["cases"]):
        c = dict(case)
        if "raises" in c:
            projection = {"raises": c.pop("raises")}
        else:
            projection = {"ok": c.pop("expect")}
            if "expect_args_after" in c:
                projection["args_after"] = c.pop("expect_args_after")
            if "expect_same" in c:
                projection["same0"] = c.pop("expect_same")
        salt = hashlib.sha256(f"{task}:{i}:starter-salt-v1".encode()).hexdigest()[:16]
        c["keys"] = sorted(projection)
        c["salt"] = salt
        c["hash"] = case_hash(salt, projection)
        cases.append(c)
    out["cases"] = cases
    return out


def main():
    check = "--check" in sys.argv
    src_dir = os.path.join(CHECKS, "spec-src")
    out_dir = os.path.join(CHECKS, "specs")
    os.makedirs(out_dir, exist_ok=True)
    stale = []
    names = sorted(f for f in os.listdir(src_dir) if f.endswith(".json"))
    for name in names:
        task = name[:-5]
        text = json.dumps(build(task, json.load(open(os.path.join(src_dir, name)))), indent=1) + "\n"
        target = os.path.join(out_dir, name)
        if check:
            if not os.path.exists(target) or open(target).read() != text:
                stale.append(name)
        else:
            open(target, "w").write(text)
    extra = sorted(set(os.listdir(out_dir)) - set(names)) if os.path.isdir(out_dir) else []
    if check and (stale or extra):
        print("specs out of date: " + ", ".join(stale + extra))
        sys.exit(1)


main()
