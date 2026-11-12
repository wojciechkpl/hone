# requires: python3
# Success check for n04-fill-config. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 - <<'PY'
import configparser
c = configparser.ConfigParser()
c.read("service.ini")
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
