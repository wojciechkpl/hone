# requires: python3
# Success check for c02-py-add-slugify. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 -m unittest -q 2>&1
