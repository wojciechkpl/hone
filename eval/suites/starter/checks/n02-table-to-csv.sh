# requires: python3
# Success check for n02-table-to-csv. Runs in the task's working-directory copy.
set -e
# The verdict is computed by python3 -I (no cwd on sys.path, no PYTHON* environment) that only
# reads the agent's files as data; its cwd is outside the copy.
COPY=$(pwd)
cd "$(mktemp -d)"
python3 -I -B - "$COPY" <<'PY'
import sys
copy = sys.argv[1]
import os
if not os.path.isfile(copy + "/quarterly.csv"):
    print("quarterly.csv is missing")
    sys.exit(1)
import csv, os
suite = os.environ["EVAL_SUITE_DIR"]
def rows(path):
    with open(path, newline="", encoding="utf-8") as f:
        return [[c for c in r] for r in csv.reader(f) if r]
got = rows(copy + "/quarterly.csv")
want = rows(os.path.join(suite, "checks/data/n02-expected.csv"))
assert got == want, f"csv differs:\n got  {got}\n want {want}"
print("ok")
PY
