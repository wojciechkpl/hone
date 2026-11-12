# requires: python3
# Success check for c03-py-refactor-discount. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 -m unittest -q 2>&1 || exit 1
grep -q '^def apply_discount' pricing.py || { echo "apply_discount(price) is missing"; exit 1; }
[ "$(grep -c '0\.85' pricing.py)" -eq 1 ] || { echo "the discount rule must appear exactly once"; exit 1; }
for f in member_price bundle_price clearance_price; do
  grep -q "^def $f" pricing.py || { echo "$f was removed"; exit 1; }
done
