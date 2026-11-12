# requires: python3
# Success check for n04-fill-config. Runs in the task's working-directory copy.
set -e
# The verdict is computed by python3 -I (no cwd on sys.path, no PYTHON* environment) that only
# reads the agent's files as data; its cwd is outside the copy.
COPY=$(pwd)
cd "$(mktemp -d)"
python3 -I -B - "$COPY" <<'PY'
import sys
copy = sys.argv[1]
import configparser
c = configparser.ConfigParser()
c.read(copy + "/service.ini")
want = {
    ("database", "host"): "db-staging.internal",
    ("database", "port"): "5433",
    ("retry", "attempts"): "4",
    ("owner", "name"): "Priya",
    ("features", "new_checkout"): "on",
    ("features", "old_checkout"): "off",
}
for (sec, key), v in want.items():
    got = c.get(sec, key, fallback=None)
    assert got == v, f"[{sec}] {key} = {got!r}, want {v!r}"
print("ok")
PY
