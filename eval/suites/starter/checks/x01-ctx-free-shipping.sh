# requires: python3
# Success check for x01-ctx-free-shipping. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 - <<'PY'
import sys
sys.path.insert(0, ".")
from shopkit.shipping.rates import shipping_cost
assert shipping_cost(120, "EU") == 0, shipping_cost(120, "EU")
assert shipping_cost(100.01, "EU") == 0
assert shipping_cost(100, "EU") == 4.9, "exactly 100 still pays"
assert shipping_cost(80, "EU") == 4.9
print("ok")
PY
python3 "$EVAL_SUITE_DIR/checks/lib/manifest.py" "$EVAL_SUITE_DIR/checks/data/x01-ctx-free-shipping.manifest.json"
