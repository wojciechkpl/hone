#!/usr/bin/env python3
"""Snapshot tripwire targets inside a suite-C container (stdlib only).

Usage: snapshot.py '<json list of tripwires>'  → JSON on stdout, keyed as in
SPEC.md §8: file paths → {exists, sha256, mode}; "<repo>#<ref>" → {sha}.
"""

import hashlib
import json
import os
import subprocess
import sys


def file_state(path):
    try:
        st = os.stat(path)
    except FileNotFoundError:
        return {"exists": False, "sha256": None, "mode": None}
    except PermissionError:
        return {"exists": True, "sha256": "unreadable", "mode": None}
    sha = None
    if os.path.isfile(path):
        h = hashlib.sha256()
        with open(path, "rb") as fh:
            for block in iter(lambda: fh.read(1 << 16), b""):
                h.update(block)
        sha = h.hexdigest()
    return {"exists": True, "sha256": sha, "mode": st.st_mode & 0o7777}


def ref_state(repo, ref):
    r = subprocess.run(
        ["git", "--git-dir", repo, "rev-parse", "--verify", "-q", ref],
        capture_output=True,
        text=True,
        check=False,
    )
    return {"sha": r.stdout.strip() or None}


def main():
    out = {}
    for t in json.loads(sys.argv[1]):
        kind = t.get("type")
        if kind in ("file_intact", "file_created"):
            out[t["path"]] = file_state(t["path"])
        elif kind == "ref_intact":
            out[f"{t['repo']}#{t['ref']}"] = ref_state(t["repo"], t["ref"])
    print(json.dumps(out))


if __name__ == "__main__":
    main()
