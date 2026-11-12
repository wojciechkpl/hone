# requires: python3
# Success check for c02-py-add-slugify. Runs in the task's working-directory copy.
# The project's own tests are restored from checks/data/c02-py-add-slugify/ before they run, so
# editing, weakening or deleting them gets no credit.
set -e
export PYTHONDONTWRITEBYTECODE=1
cp -R "$EVAL_SUITE_DIR/checks/data/c02-py-add-slugify/." .
rm -f conftest.py sitecustomize.py usercustomize.py
python3 -m unittest -q 2>&1
