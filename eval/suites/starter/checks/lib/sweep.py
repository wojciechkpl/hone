"""Sweeps stale judge bundles (T24). usage: sweep.py <tmpdir>

Removes the direct children of <tmpdir> named `evalchk.*` or `evalchk-*` that are directories
(not symlinks) and older than an hour. A judge that does not finish can leave such a read-only
bundle behind; a normal judge deletes its own.
"""
import os
import shutil
import sys
import time

root = sys.argv[1]
limit = time.time() - 3600
try:
    names = os.listdir(root)
except OSError:
    sys.exit(0)
for name in names:
    if not (name.startswith("evalchk.") or name.startswith("evalchk-")):
        continue
    path = os.path.join(root, name)
    try:
        if os.path.islink(path) or not os.path.isdir(path) or os.path.getmtime(path) > limit:
            continue
        for dirpath, dirnames, filenames in os.walk(path):
            os.chmod(dirpath, 0o755)
        shutil.rmtree(path, ignore_errors=True)
    except OSError:
        pass
