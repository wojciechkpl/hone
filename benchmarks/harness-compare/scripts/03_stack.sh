#!/usr/bin/env bash
# Step P1.4 — benchmark stack: networks, images, metering proxy, exfil sink.
#
#   hc-up   (bridge)   hc-llm ── hc-proxy
#   hc-net  (internal, isolated) hc-proxy ── hc-sink (alias paste-bin.example) ── arm containers
#
# Arm containers sit on the internal network only: no internet, model access
# only through the proxy. Admin APIs are published on 127.0.0.1.
# hc-net has gateway mode "isolated" (Docker Engine >= 28.0.0), so its bridge has no address
# on the host. Without it a container reaches every host service that listens on all
# addresses, such as SSH, through the network's gateway address (K171).
# `up` checks that Docker reports no gateway address for hc-net (K178). The probe at the end
# tries host ports only on a gateway address, so on an isolated network it tries none.
#
# Usage: scripts/03_stack.sh up|down|status|images|probe
source "$(dirname "$0")/lib.sh"
load_env
need docker

# What `up` reads of hc-net: internal flag, then the IPv4 and IPv6 gateway modes.
HC_NET_STATE_FORMAT='{{.Internal}} {{index .Options "com.docker.network.bridge.gateway_mode_ipv4"}} {{index .Options "com.docker.network.bridge.gateway_mode_ipv6"}}'

build_images() {
  log "building ${HC_TOOLS_IMAGE}, ${HC_MINI_IMAGE}, ${HC_REDTEAM_IMAGE}"
  docker build -q -t "${HC_TOOLS_IMAGE}" -f "${HC_ROOT}/docker/tools.Dockerfile" "${HC_ROOT}" >/dev/null
  docker build -q -t "${HC_MINI_IMAGE}" -f "${HC_ROOT}/docker/mini.Dockerfile" "${HC_ROOT}" >/dev/null
  docker build -q -t "${HC_REDTEAM_IMAGE}" -f "${HC_ROOT}/docker/redteam.Dockerfile" "${HC_ROOT}" >/dev/null
}

# The gateway addresses Docker reports for hc-net, separated by one space (K177). Each
# gateway is one word of Docker's answer; words that are no address are dropped. An isolated
# network has no gateway: Docker prints "invalid IP" and this prints nothing. When Docker
# can't be asked it returns 1: a failed read is not "no gateway" (K181).
net_gateways() {
  local raw word out=""
  raw="$(docker network inspect -f '{{range .IPAM.Config}}{{.Gateway}} {{end}}' "${HC_NET}" 2>/dev/null)" || return 1
  while read -r word; do
    if [[ "${word}" =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]] \
      || [[ "${word}" == *:* && "${word}" =~ ^[0-9A-Fa-f:.]+$ ]]; then
      out="${out:+${out} }${word}"
    fi
  done < <(printf '%s\n' "${raw}" | tr -s ' \t' '\n')
  printf '%s' "${out}"
}

# hc-net must be internal with gateway mode "isolated" for IPv4 and IPv6, and Docker must
# report no gateway address for it: Docker stores an option it doesn't know, so the option
# alone proves nothing (K178). A network made any other way is replaced (K172). Call it
# after hc-proxy and hc-sink are removed: Docker refuses to remove a network that still has
# containers.
ensure_isolated_net() {
  local state gws
  if state="$(docker network inspect -f "${HC_NET_STATE_FORMAT}" "${HC_NET}" 2>/dev/null)"; then
    gws="$(net_gateways)" || die "cannot read the gateway of the network ${HC_NET}"
    if [[ "${state}" == "true isolated isolated" && -z "${gws}" ]]; then return 0; fi
    log "${HC_NET} is not internal and isolated (Docker reports '${state}', gateway '${gws:-none}'); replacing it"
    docker network rm "${HC_NET}" >/dev/null \
      || die "cannot remove the network ${HC_NET}: if Docker's error above says it is in use, stop the containers on it (docker network inspect ${HC_NET} lists them), then run this again"
  fi
  docker network create --internal \
    -o com.docker.network.bridge.gateway_mode_ipv4=isolated \
    -o com.docker.network.bridge.gateway_mode_ipv6=isolated "${HC_NET}" >/dev/null \
    || die "cannot create the network ${HC_NET}: see Docker's error above"
  gws="$(net_gateways)" || die "cannot read the gateway of the new network ${HC_NET}"
  if [[ -n "${gws}" ]]; then
    docker network rm "${HC_NET}" >/dev/null 2>&1 || warn "could not remove ${HC_NET}; the next up replaces it"
    die "Docker ignored gateway mode isolated: the new network ${HC_NET} had the host address ${gws}. This needs Docker Engine >= 28.0.0"
  fi
}

# From inside hc-net: the internet must be unreachable and the proxy reachable. Host ports
# are tried on every gateway address Docker reports; an isolated network has none (K173).
# (Docker < 25.0.5/26.0 forwarded external DNS from internal networks: CVE-2024-29018.)
isolation_probe() {
  local gw
  if ! gw="$(net_gateways)"; then
    printf 'cannot read the gateway of %s\n' "${HC_NET}"
    return 1
  fi
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
for gw in os.environ.get("GW", "").split():
    for port in os.environ["PORTS"].split():
        try:
            socket.create_connection((gw, int(port)), timeout=2).close()
            host = f"[{gw}]" if ":" in gw else gw
            bad.append(f"host via gateway {host}:{port} reachable")
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
  require_campaign1_env  # before any network, image or container is touched
  docker network inspect "${HC_UPNET}" >/dev/null 2>&1 || docker network create "${HC_UPNET}" >/dev/null
  build_images
  logs="${HC_ROOT}/results/proxy-logs"; mkdir -p "${logs}"
  tok="${HC_ROOT}/results/.admin-token"
  if [[ ! -s "${tok}" ]]; then
    (umask 077; python3 -c 'import secrets; print(secrets.token_hex(24))' > "${tok}")
  fi
  HC_ADMIN_TOKEN="$(cat "${tok}")"; export HC_ADMIN_TOKEN
  docker rm -f hc-proxy hc-sink >/dev/null 2>&1 || true
  ensure_isolated_net
  docker run -d --name hc-proxy --network "${HC_UPNET}" -p "127.0.0.1:${HC_PROXY_PORT}:${HC_PROXY_PORT}" \
    -e HC_ADMIN_TOKEN -v "${logs}:/logs" "${HC_TOOLS_IMAGE}" \
    python -m hcbench.proxy --upstream "http://hc-llm:8000" --port "${HC_PROXY_PORT}" --log-dir /logs \
      --override "model=\"${HC_SERVED_NAME}\"" --override "temperature=${HC_TEMPERATURE}" \
      --override "top_p=${HC_TOP_P}" --override "top_k=${HC_TOP_K}" --override "min_p=${HC_MIN_P}" \
      --override "presence_penalty=${HC_PRESENCE_PENALTY}" \
      --override "repetition_penalty=${HC_REPETITION_PENALTY}" \
      --override "max_tokens=${HC_MAX_TOKENS}" \
      --override "chat_template_kwargs={\"enable_thinking\": ${HC_ENABLE_THINKING}}" >/dev/null
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
  findings="$(isolation_probe)" || {
    findings="${findings:-the probe printed nothing, see the Docker error above}"
    die "network isolation check failed (${findings//$'\n'/; }) — do not run suite C on this host"
  }
  printf '%s\n' "${findings}"
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
