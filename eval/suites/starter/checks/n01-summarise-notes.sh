# requires: grep
# Success check for n01-summarise-notes. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
[ -f summary.md ] || { echo "summary.md is missing"; exit 1; }
for fact in "April 14" "42,000" "Priya" "Tomás" "Lisbon" "March 31"; do
  grep -iqF -- "$fact" summary.md || { echo "summary.md does not mention: $fact"; exit 1; }
done
