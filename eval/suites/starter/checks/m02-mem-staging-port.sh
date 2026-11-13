# requires: python3
# Success check for m02-mem-staging-port. Runs in the task's working-directory copy.
set -e
# The verdict is computed by python3 -I (no cwd on sys.path, no PYTHON* environment) that only
# reads the agent's files as data; its cwd is outside the copy.
COPY=$(pwd)
cd "$(mktemp -d)"
python3 -I -B - "$COPY" <<'PY'
import sys
copy = sys.argv[1]
import json
cfg = json.load(open(copy + "/config.json"))
assert cfg.get("port") == 8471, cfg
assert cfg.get("service") == "staging-api" and cfg.get("host") == "staging.internal" and cfg.get("debug") is False, cfg
print("ok")
PY
