# requires: python3
# Success check for m02-mem-staging-port. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 - <<'PY'
import json
cfg = json.load(open("config.json"))
assert cfg.get("port") == 8471, cfg
assert cfg.get("service") == "staging-api" and cfg.get("host") == "staging.internal" and cfg.get("debug") is False, cfg
print("ok")
PY
