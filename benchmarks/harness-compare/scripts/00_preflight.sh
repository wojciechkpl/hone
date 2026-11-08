#!/usr/bin/env bash
# Step P1.1 — verify the Ubuntu + RTX 5090 host before anything else.
# Usage: scripts/00_preflight.sh            (exit 1 on any FAIL)
source "$(dirname "$0")/lib.sh"
load_env

fails=0 warns=0
pass() { printf '  PASS  %s\n' "$*"; }
fail() { printf '  FAIL  %s\n' "$*"; fails=$((fails + 1)); }
note() { printf '  WARN  %s\n' "$*"; warns=$((warns + 1)); }

echo "== host"
[[ "$(uname -s)" == "Linux" ]] && pass "Linux" || fail "not Linux ($(uname -s)); SWE-bench images are x86_64 Linux"
[[ "$(uname -m)" == "x86_64" ]] && pass "x86_64" || fail "arch $(uname -m)"
kver="$(uname -r | cut -d. -f1-2)"
if printf '5.13\n%s\n' "${kver}" | sort -V -C 2>/dev/null; then pass "kernel ${kver}"; else fail "kernel ${kver} < 5.13 (no Landlock)"; fi
if printf '6.7\n%s\n' "${kver}" | sort -V -C 2>/dev/null; then pass "kernel ≥ 6.7 (Landlock ABI 4 network rules, Phase 2)"; else note "kernel < 6.7: no Landlock network rules for Phase 2"; fi
if grep -qw landlock /sys/kernel/security/lsm 2>/dev/null; then pass "Landlock LSM active"; else fail "Landlock not in /sys/kernel/security/lsm (Hone's sandbox will fail closed)"; fi

echo "== tools"
for c in docker git curl uv python3; do command -v "$c" >/dev/null && pass "$c" || fail "$c missing"; done
python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)' && pass "python ≥ 3.11" || fail "python < 3.11"

echo "== GPU"
if command -v nvidia-smi >/dev/null; then
  gpu="$(nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader | head -1)"
  pass "nvidia-smi: ${gpu}"
  mem_mib="$(nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits | head -1 | tr -d ' ')"
  (( mem_mib >= 30000 )) && pass "VRAM ${mem_mib} MiB" || fail "VRAM ${mem_mib} MiB < 30 GiB"
  [[ "${gpu}" == *5090* ]] || note "GPU is not an RTX 5090; the plan's VRAM math assumes 32 GB"
else
  fail "nvidia-smi not found"
fi

echo "== docker"
if docker info >/dev/null 2>&1; then
  pass "docker daemon reachable"
  dver="$(docker version --format '{{.Server.Version}}' 2>/dev/null || echo 0)"
  if printf '25.0.5\n%s\n' "${dver}" | sort -V -C; then pass "docker ${dver} (≥ 25.0.5)"
  else fail "docker ${dver}: internal networks leak DNS before 25.0.5/26.0 (CVE-2024-29018)"; fi
  docker info --format '{{json .Runtimes}}' | grep -q nvidia && pass "nvidia container runtime" \
    || note "nvidia runtime not listed; check nvidia-container-toolkit (docker run --gpus all ...)"
  root="$(docker info --format '{{.DockerRootDir}}')"
  free_gb="$(df -BG --output=avail "${root}" 2>/dev/null | tail -1 | tr -dc 0-9)"
  if [[ -n "${free_gb}" ]]; then
    (( free_gb >= 200 )) && pass "${free_gb} GB free at ${root}" \
      || fail "${free_gb} GB free at ${root}; need ≥ 200 GB (40 SWE-bench images + model + logs)"
  fi
  seccomp="$(docker info --format '{{.SecurityOptions}}')"
  [[ "${seccomp}" == *seccomp* ]] && note "default seccomp active — smoke step 04 checks Landlock inside containers" || pass "no seccomp"
else
  fail "docker daemon not reachable"
fi

echo "== ports"
for p in "${HC_LLM_PORT}" "${HC_PROXY_PORT}" "${HC_SINK_PORT}"; do
  if (exec 3<>"/dev/tcp/127.0.0.1/${p}") 2>/dev/null; then
    note "port ${p} already in use (fine if it is our own stack)"
  else
    pass "port ${p} free"
  fi
done

echo "== config"
grep -q CHANGE-ME "${HC_ENV_FILE}" && note "placeholders (CHANGE-ME) left in ${HC_ENV_FILE}" || pass "no placeholders in ${HC_ENV_FILE}"

echo
echo "preflight: ${fails} fail, ${warns} warn"
(( fails == 0 ))
