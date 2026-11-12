# requires: python3
# Success check for m03-mem-deadline-format. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 - <<'PY'
import datetime, sys
sys.path.insert(0, ".")
import deadlines
for day, want in [(datetime.date(2026, 3, 9), "09-Mar-2026"), (datetime.date(2026, 12, 1), "01-Dec-2026"), (datetime.date(2027, 8, 31), "31-Aug-2027")]:
    got = deadlines.format_deadline(day)
    assert got == want, (day, got)
print("ok")
PY
