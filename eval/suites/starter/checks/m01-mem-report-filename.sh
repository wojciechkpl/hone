# requires: python3
# Success check for m01-mem-report-filename. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 - <<'PY'
import datetime, sys
sys.path.insert(0, ".")
import reports
got = reports.make_report_filename(datetime.date(2026, 3, 9))
assert got == "report_20260309_platform.txt", got
got = reports.make_report_filename(datetime.date(2025, 12, 1))
assert got == "report_20251201_platform.txt", got
print("ok")
PY
