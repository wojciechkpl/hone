#!/usr/bin/env python3
"""Context-task check helper: only the target file(s) of the manifest may differ.

usage: manifest.py MANIFEST.json   (run in the working-directory copy)

Exits 1 when a listed file other than the target was changed or deleted, or when
the target is unchanged. New files are allowed.
"""
import hashlib
import json
import sys


def sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def main():
    manifest = json.load(open(sys.argv[1]))
    targets = set(manifest["target"])
    problems = []
    for rel, digest in manifest["files"].items():
        try:
            now = sha(rel)
        except FileNotFoundError:
            problems.append(f"deleted: {rel}")
            continue
        if rel in targets:
            if now == digest:
                problems.append(f"the file that needs the change was not modified: {rel}")
        elif now != digest:
            problems.append(f"changed a file that should stay as it was: {rel}")
    if problems:
        print("\n".join(problems))
        sys.exit(1)
    print("only the expected file changed")


main()
