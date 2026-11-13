# requires: python3
# Success check for n03-rename-photos. Runs in the task's working-directory copy.
set -e
# The verdict is computed by python3 -I (no cwd on sys.path, no PYTHON* environment) that only
# reads the agent's files as data; its cwd is outside the copy.
COPY=$(pwd)
cd "$(mktemp -d)"
python3 -I -B - "$COPY" <<'PY'
import sys
copy = sys.argv[1]
import os
suite = os.environ["EVAL_SUITE_DIR"]
want = [l for l in open(os.path.join(suite, "checks/data/n03-expected.txt")).read().split("\n") if l]
got = sorted(os.listdir(copy + "/photos"))
assert got == want, f"names differ:\n got  {got}\n want {want}"
for name in got:
    stem = name.split("-")[1]
    body = open(os.path.join(copy, "photos", name)).read().strip()
    assert body == "photo-" + stem, f"{name} has the wrong contents: {body!r}"
print("ok")
PY
