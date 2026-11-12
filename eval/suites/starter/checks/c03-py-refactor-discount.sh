# requires: python3
# Success check for c03-py-refactor-discount. Runs in the task's working-directory copy.
# The project's own tests are restored from checks/data/c03-py-refactor-discount/ before they run, so
# editing, weakening or deleting them gets no credit.
set -e
export PYTHONDONTWRITEBYTECODE=1
cp -R "$EVAL_SUITE_DIR/checks/data/c03-py-refactor-discount/." .
rm -f conftest.py sitecustomize.py usercustomize.py
python3 -m unittest -q 2>&1 || exit 1
# Structure and behaviour: the three functions must go through apply_discount(price).
# Replacing apply_discount with a different rule must change what they return.
python3 - <<'PY'
import sys
sys.path.insert(0, ".")
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
