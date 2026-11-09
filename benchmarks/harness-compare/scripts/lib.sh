#!/usr/bin/env bash
# Shared helpers for hcbench scripts. Source, don't execute.
set -euo pipefail

HC_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export HC_ROOT
REPO_ROOT="$(cd "${HC_ROOT}/../.." && pwd)"
export REPO_ROOT

log()  { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
warn() { printf '[%s] WARN: %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
die()  { printf '[%s] ERROR: %s\n' "$(date +%H:%M:%S)" "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"; }

load_env() {
  local env_file="${HC_ENV_FILE:-${HC_ROOT}/config/bench.env}"
  if [[ ! -f "${env_file}" ]]; then
    warn "${env_file} not found; using bench.env.example (copy it and edit before real runs)"
    env_file="${HC_ROOT}/config/bench.env.example"
  fi
  # Variables already in the environment win over the file (e.g. HC_SERVER=mock).
  local saved
  saved="$(env | grep -E '^HC_[A-Z0-9_]+=' || true)"
  set -a
  # shellcheck disable=SC1090
  source "${env_file}"
  set +a
  local k v
  while IFS='=' read -r k v; do
    [[ -n "${k}" ]] && export "${k}=${v}"
  done <<< "${saved}"
  export HC_ENV_FILE="${env_file}"
  # Admin token for the proxy/sink admin APIs (created by 03_stack.sh up). Arm containers
  # never see it, so an agent cannot re-register its run or wipe the sink.
  if [[ -z "${HC_ADMIN_TOKEN:-}" && -f "${HC_ROOT}/results/.admin-token" ]]; then
    HC_ADMIN_TOKEN="$(cat "${HC_ROOT}/results/.admin-token")"
    export HC_ADMIN_TOKEN
  fi
}

admin_curl() { # curl with the admin bearer token
  curl -fsS -H "Authorization: Bearer ${HC_ADMIN_TOKEN:-}" "$@"
}

# Run hcbench Python with its deps via uv (no global installs).
hc_py() {
  need uv
  (cd "${HC_ROOT}" && uv run --no-project -q --python 3.12 --with "aiohttp>=3.9" ${HC_PY_WITH:-} python -m "$@")
}

container_running() { [[ "$(docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null || true)" == "true" ]]; }

wait_http() { # url timeout_s
  local url="$1" deadline=$((SECONDS + ${2:-600}))
  until curl -fs -o /dev/null "${url}"; do
    (( SECONDS > deadline )) && return 1
    sleep 5
  done
}

campaign_dir() { # name -> absolute path under results/
  local name="${1:?campaign name required}"
  printf '%s/results/%s' "${HC_ROOT}" "${name}"
}
