# requires: python3
# Success check for c03-py-refactor-discount. Runs in the task's working-directory copy (the agent's files).
# T7-T9: only the allowlisted solution files are copied into a fresh directory next to the
# pristine tests and project files (checks/data/c03-py-refactor-discount/); the tests run there, isolated, and
# a held-out probe in a separate process imports only the solution.
set -e
. "$EVAL_SUITE_DIR/checks/lib/fresh.sh"
fresh_begin c03-py-refactor-discount
fresh_take pricing.py
(cd "$FRESH" && py_isolated -m unittest discover -s . -t . -p 'test_*.py' -q) 2>&1 || exit 1
# Structure and behaviour: the three functions must go through the module-level binding
# apply_discount (looked up when they are called). Replacing it with a different rule must
# change what they return.
py_isolated - "$FRESH" <<'PY'
import sys
sys.path.insert(0, sys.argv[1])
import pricing

for name in ("apply_discount", "member_price", "bundle_price", "clearance_price"):
    if not callable(getattr(pricing, name, None)):
        print(f"{name} is missing")
        sys.exit(1)
if pricing.apply_discount(100) != 85.0:
    print("apply_discount(100) must keep the discount rule (85.0)")
    sys.exit(1)

pricing.apply_discount = lambda price: price - 1
probes = [
    ("member_price", pricing.member_price(100), 99),
    ("bundle_price", pricing.bundle_price([100, 20]), 118),
    ("clearance_price", pricing.clearance_price(100, extra_off=5), 94),
]
bad = [f"{n} returned {got}, expected {want} when apply_discount is swapped" for n, got, want in probes if got != want]
if bad:
    print("\n".join(bad))
    print("the three functions must call apply_discount instead of repeating the rule")
    sys.exit(1)
print("ok")
PY
