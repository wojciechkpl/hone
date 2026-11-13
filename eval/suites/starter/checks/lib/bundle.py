"""Builds the private bundle one judge run needs (T19). usage: bundle.py <suite> <bundle> <task>

Copies judge.py, the task's hashed spec, the assets the spec names, its base directory and its
manifest into <bundle> with the suite's relative layout, so the judge can run from there without
knowing where the suite is.
"""
import json
import os
import shutil
import sys

suite, bundle, task = sys.argv[1:4]
spec = json.load(open(os.path.join(suite, "checks", "specs", task + ".json")))


def copy(rel):
    src = os.path.join(suite, rel)
    dst = os.path.join(bundle, rel)
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    if os.path.isdir(src):
        shutil.copytree(src, dst, dirs_exist_ok=True)
    else:
        shutil.copy(src, dst)


copy("checks/lib/judge.py")
copy(f"checks/specs/{task}.json")
for name in spec.get("assets", []):
    copy(f"checks/lib/{name}")
if spec.get("base"):
    copy(spec["base"])
if spec.get("manifest"):
    copy("checks/data/" + spec["manifest"])
