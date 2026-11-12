# requires: python3
# Success check for x03-ctx-blank-timeout. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 - <<'PY'
import sys
sys.path.insert(0, ".")
from app.settings.loader import load_settings
assert load_settings("name=x\ntimeout=\n")["timeout"] == 30
assert load_settings("timeout=   \n")["timeout"] == 30
assert load_settings("timeout=5\n")["timeout"] == 5
assert load_settings("")["timeout"] == 30
print("ok")
PY
python3 "$EVAL_SUITE_DIR/checks/lib/manifest.py" "$EVAL_SUITE_DIR/checks/data/x03-ctx-blank-timeout.manifest.json"
