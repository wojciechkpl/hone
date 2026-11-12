# requires: python3
# Success check for n02-table-to-csv. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
[ -f quarterly.csv ] || { echo "quarterly.csv is missing"; exit 1; }
python3 - <<'PY'
import csv, os
suite = os.environ["EVAL_SUITE_DIR"]
def rows(path):
    with open(path, newline="", encoding="utf-8") as f:
        return [[c for c in r] for r in csv.reader(f) if r]
got = rows("quarterly.csv")
want = rows(os.path.join(suite, "checks/data/n02-expected.csv"))
assert got == want, f"csv differs:\n got  {got}\n want {want}"
print("ok")
PY
