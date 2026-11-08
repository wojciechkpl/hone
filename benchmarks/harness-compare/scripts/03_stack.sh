#!/usr/bin/env bash
# Step P1.4 — benchmark stack: networks, images, metering proxy, exfil sink.
#
#   hc-up   (bridge)   hc-llm ── hc-proxy
#   hc-net  (internal) hc-proxy ── hc-sink (alias paste-bin.example) ── arm containers
#
# Arm containers sit on the internal network only: no internet, model access
# only through the proxy. Admin APIs are published on 127.0.0.1.
#
# Usage: scripts/03_stack.sh up|down|status|images
source "$(dirname "$0")/lib.sh"
load_env
need docker

build_images() {
  log "building ${HC_TOOLS_IMAGE}, ${HC_MINI_IMAGE}, ${HC_REDTEAM_IMAGE}"
  docker build -q -t "${HC_TOOLS_IMAGE}" -f "${HC_ROOT}/docker/tools.Dockerfile" "${HC_ROOT}" >/dev/null
  docker build -q -t "${HC_MINI_IMAGE}" -f "${HC_ROOT}/docker/mini.Dockerfile" "${HC_ROOT}" >/dev/null
  docker build -q -t "${HC_REDTEAM_IMAGE}" -f "${HC_ROOT}/docker/redteam.Dockerfile" "${HC_ROOT}" >/dev/null
}

# From inside hc-net: the internet and the host must be unreachable, the proxy reachable.
# (Docker < 25.0.5/26.0 forwarded external DNS from internal networks: CVE-2024-29018.)
isolation_probe() {
  local gw
  gw="$(docker network inspect -f '{{range .IPAM.Config}}{{.Gateway}}{{end}}' "${HC_NET}" 2>/dev/null || true)"
  docker run --rm -i --network "${HC_NET}" -e GW="${gw}" -e PROXY_PORT="${HC_PROXY_PORT}" \
    -e PORTS="22 80 443 ${HC_LLM_PORT} ${HC_PROXY_PORT} ${HC_SINK_PORT}" "${HC_TOOLS_IMAGE}" python - << 'PY'
import os, socket, sys
bad = []
try:
    socket.getaddrinfo("example.com", 443)
    bad.append("external DNS resolves (example.com)")
except OSError:
    pass
for host in ("1.1.1.1", "8.8.8.8"):
    try:
        socket.create_connection((host, 443), timeout=3).close()
        bad.append(f"TCP to {host}:443 works")
    except OSError:
        pass
gw = os.environ.get("GW", "")
for port in os.environ["PORTS"].split() if gw else []:
    try:
        socket.create_connection((gw, int(port)), timeout=2).close()
        bad.append(f"host via gateway {gw}:{port} reachable")
    except OSError:
        pass
try:
    socket.create_connection(("hc-proxy", int(os.environ["PROXY_PORT"])), timeout=3).close()
except OSError:
    bad.append("hc-proxy NOT reachable (arms could not reach the model)")
print("\n".join(bad) or "isolation ok")
sys.exit(1 if bad else 0)
PY
}

case "${1:-status}" in
up)
  docker network inspect "${HC_UPNET}" >/dev/null 2>&1 || docker network create "${HC_UPNET}" >/dev/null
  docker network inspect "${HC_NET}" >/dev/null 2>&1 || docker network create --internal "${HC_NET}" >/dev/null
  build_images
  logs="${HC_ROOT}/results/proxy-logs"; mkdir -p "${logs}"
  tok="${HC_ROOT}/results/.admin-token"
  if [[ ! -s "${tok}" ]]; then
    (umask 077; python3 -c 'import secrets; print(secrets.token_hex(24))' > "${tok}")
  fi
  HC_ADMIN_TOKEN="$(cat "${tok}")"; export HC_ADMIN_TOKEN
  docker rm -f hc-proxy hc-sink >/dev/null 2>&1 || true
  docker run -d --name hc-proxy --network "${HC_UPNET}" -p "127.0.0.1:${HC_PROXY_PORT}:${HC_PROXY_PORT}" \
    -e HC_ADMIN_TOKEN -v "${logs}:/logs" "${HC_TOOLS_IMAGE}" \
    python -m hcbench.proxy --upstream "http://hc-llm:8000" --port "${HC_PROXY_PORT}" --log-dir /logs \
      --override "model=\"${HC_SERVED_NAME}\"" --override "temperature=${HC_TEMPERATURE}" \
      --override "top_p=${HC_TOP_P}" --override "max_tokens=${HC_MAX_TOKENS}" >/dev/null
  docker network connect --alias hc-proxy "${HC_NET}" hc-proxy
  # Published ports need a non-internal network, so the sink (like the proxy) starts on
  # hc-up for the host-side admin port, then joins hc-net under the attacker hostname.
  # Arm containers live on hc-net only, so this gives them no route outside.
  docker run -d --name hc-sink --network "${HC_UPNET}" -p "127.0.0.1:${HC_SINK_PORT}:8080" \
    -e HC_ADMIN_TOKEN "${HC_TOOLS_IMAGE}" python -m hcbench.sink --port 8080 >/dev/null
  docker network connect --alias paste-bin.example "${HC_NET}" hc-sink
  deadline=$((SECONDS + 60))
  until admin_curl -o /dev/null "http://127.0.0.1:${HC_SINK_PORT}/__hits" 2>/dev/null; do
    (( SECONDS > deadline )) && die "sink not reachable"; sleep 2
  done
  admin_curl -X POST "http://127.0.0.1:${HC_PROXY_PORT}/admin/runs" -H 'Content-Type: application/json' \
    -d '{"run_id":"stack.check.s0.ping"}' >/dev/null || die "proxy admin not reachable"
  isolation_probe || die "network isolation check failed — do not run suite C on this host"
  log "stack up: proxy :${HC_PROXY_PORT}, sink :${HC_SINK_PORT}, internal net ${HC_NET} (isolated)"
  ;;
probe) isolation_probe ;;
down)
  docker rm -f hc-proxy hc-sink >/dev/null 2>&1 || true
  docker network rm "${HC_NET}" >/dev/null 2>&1 || true
  log "stack down (hc-llm untouched; use 02_serve_model.sh down)"
  ;;
images) build_images ;;
status)
  for c in hc-llm hc-proxy hc-sink; do
    printf '%-9s %s\n' "$c" "$(container_running "$c" && echo running || echo stopped)"
  done
  ;;
*) die "usage: $0 up|down|status|images|probe" ;;
esac
