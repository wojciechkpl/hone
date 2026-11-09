#!/usr/bin/env bash
# Step P1.3 — serve ONE model for every arm (container hc-llm on network hc-up).
# Usage: scripts/02_serve_model.sh up|down|status|logs
#        HC_SERVER=mock scripts/02_serve_model.sh up    # no-GPU selftest upstream
source "$(dirname "$0")/lib.sh"
load_env
need docker

cmd="${1:-status}"
docker network inspect "${HC_UPNET}" >/dev/null 2>&1 || docker network create "${HC_UPNET}" >/dev/null

case "${cmd}" in
up)
  container_running hc-llm && { log "hc-llm already running"; exit 0; }
  docker rm -f hc-llm >/dev/null 2>&1 || true
  case "${HC_SERVER}" in
  vllm)
    [[ "${HC_MODEL_HF}" == *CHANGE-ME* || "${HC_VLLM_IMAGE}" == *CHANGE-ME* ]] && die "set HC_MODEL_HF and HC_VLLM_IMAGE in bench.env"
    rev=(); [[ -n "${HC_MODEL_REVISION}" && "${HC_MODEL_REVISION}" != *CHANGE-ME* ]] && rev=(--revision "${HC_MODEL_REVISION}")
    docker run -d --name hc-llm --network "${HC_UPNET}" --gpus all --ipc=host \
      -p "127.0.0.1:${HC_LLM_PORT}:8000" -v "${HC_HF_CACHE}:/root/.cache/huggingface" \
      "${HC_VLLM_IMAGE}" \
      --model "${HC_MODEL_HF}" "${rev[@]}" --served-model-name "${HC_SERVED_NAME}" \
      --max-model-len "${HC_MAX_MODEL_LEN}" --gpu-memory-utilization "${HC_GPU_MEM_UTIL}" \
      --kv-cache-dtype "${HC_KV_CACHE_DTYPE}" --max-num-seqs "${HC_MAX_NUM_SEQS}" \
      --enable-auto-tool-choice --tool-call-parser "${HC_TOOL_PARSER}" \
      --generation-config vllm \
      --seed 0 >/dev/null
    # --generation-config vllm: ignore the model's generation_config.json defaults (e.g.
    # repetition_penalty 1.05) so the only sampling knobs are the ones the proxy forces.
    ;;
  llamacpp)
    docker run -d --name hc-llm --network "${HC_UPNET}" --gpus all \
      -p "127.0.0.1:${HC_LLM_PORT}:8000" -v "$(dirname "${HC_GGUF_PATH}"):/models:ro" \
      "${HC_LLAMACPP_IMAGE}" \
      -m "/models/$(basename "${HC_GGUF_PATH}")" --alias "${HC_SERVED_NAME}" --jinja \
      -c "$(( HC_MAX_MODEL_LEN * HC_MAX_NUM_SEQS ))" -np "${HC_MAX_NUM_SEQS}" -ngl 999 \
      --host 0.0.0.0 --port 8000 --seed 0 --repeat-penalty 1.0 >/dev/null
    ;;
  mock)
    docker build -q -t "${HC_TOOLS_IMAGE}" -f "${HC_ROOT}/docker/tools.Dockerfile" "${HC_ROOT}" >/dev/null
    docker run -d --name hc-llm --network "${HC_UPNET}" -p "127.0.0.1:${HC_LLM_PORT}:8000" \
      "${HC_TOOLS_IMAGE}" python -m hcbench.mockllm --port 8000 >/dev/null
    ;;
  *) die "unknown HC_SERVER=${HC_SERVER}" ;;
  esac
  log "waiting for hc-llm on :${HC_LLM_PORT} (model load can take minutes)"
  wait_http "http://127.0.0.1:${HC_LLM_PORT}/v1/models" 1800 || { docker logs --tail 50 hc-llm; die "hc-llm not ready"; }
  curl -fsS "http://127.0.0.1:${HC_LLM_PORT}/v1/models"; echo
  ;;
down) docker rm -f hc-llm >/dev/null 2>&1 || true; log "hc-llm removed" ;;
status) container_running hc-llm && curl -fsS "http://127.0.0.1:${HC_LLM_PORT}/v1/models" && echo || echo "hc-llm not running" ;;
logs) docker logs --tail "${2:-100}" -f hc-llm ;;
*) die "usage: $0 up|down|status|logs" ;;
esac
