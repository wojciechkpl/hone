#!/usr/bin/env bash
# Step P1.2 — build the read-only harness bundle mounted at /opt/harness:
#   bin/hone            Hone at ${HC_HONE_REF}, built in a clean worktree, glibc ≤ 2.31
#   node/ + pi/         Node ${HC_NODE_VERSION} (official tarball) + Pi ${HC_PI_VERSION}
#   pi-ext/             Pi's example guardrail extensions (pi-guarded arm)
#   bin/*.py            in-container helpers (snapshot, selftest agent)
#   manifest.json       versions + hashes (copied into the pre-registration)
#
# Usage: scripts/01_build_bundle.sh [--helpers-only] [--skip-hone] [--skip-pi]
source "$(dirname "$0")/lib.sh"
load_env
need docker; need git; need curl

helpers_only=0 skip_hone=0 skip_pi=0
for a in "$@"; do
  case "$a" in
    --helpers-only) helpers_only=1 ;;
    --skip-hone) skip_hone=1 ;;
    --skip-pi) skip_pi=1 ;;
    *) die "unknown flag $a" ;;
  esac
done

B="${HC_BUNDLE_DIR}"
mkdir -p "${B}/bin" "${B}/pi-ext"
install -m 0755 "${HC_ROOT}"/harness_bin/*.py "${B}/bin/"
log "helpers installed into ${B}/bin"
if (( helpers_only )); then
  printf '{"helpers_only": true}\n' > "${B}/manifest.json"
  exit 0
fi

# ---------------------------------------------------------------- Hone
hone_sha=""
if (( ! skip_hone )); then
  hone_sha="$(git -C "${REPO_ROOT}" rev-parse "${HC_HONE_REF}^{commit}")"
  wt="${HC_ROOT}/.build/hone-src"
  rm -rf "${wt}"; git -C "${REPO_ROOT}" worktree prune
  git -C "${REPO_ROOT}" worktree add --detach "${wt}" "${hone_sha}" >/dev/null
  trap 'git -C "${REPO_ROOT}" worktree remove --force "${wt}" >/dev/null 2>&1 || true' EXIT
  log "building hone ${hone_sha:0:12} in ${HC_RUST_IMAGE} (target dir in docker volume hc-cargo-target)"
  docker run --rm \
    -v "${wt}:/src:ro" -v hc-cargo-target:/target -v hc-cargo-registry:/usr/local/cargo/registry \
    -v "${B}/bin:/out" -w /src -e CARGO_TARGET_DIR=/target -e OWNER="$(id -u):$(id -g)" \
    "${HC_RUST_IMAGE}" bash -c '
      set -e
      apt-get update -qq && apt-get install -y -qq pkg-config libssl-dev git >/dev/null
      cargo build --release --locked -p hone-cli --bin hone
      install -m 0755 /target/release/hone /out/hone
      chown "${OWNER}" /out/hone
      ldd --version | head -1'
  "${B}/bin/hone" --version >/dev/null 2>&1 || warn "host cannot run bin/hone directly (fine if host glibc differs)"
fi

# ------------------------------------------------------------ Node + Pi
pi_bin_rel=""
if (( ! skip_pi )); then
  nv="v${HC_NODE_VERSION}"; tarball="node-${nv}-linux-x64.tar.xz"
  dl="${HC_ROOT}/.build/dl"; mkdir -p "${dl}"
  if [[ ! -x "${B}/node/bin/node" ]]; then
    log "downloading Node ${nv}"
    curl -fsSLo "${dl}/${tarball}" "https://nodejs.org/dist/${nv}/${tarball}"
    curl -fsSLo "${dl}/SHASUMS256.txt" "https://nodejs.org/dist/${nv}/SHASUMS256.txt"
    (cd "${dl}" && grep " ${tarball}\$" SHASUMS256.txt | sha256sum -c -) || die "Node checksum mismatch"
    rm -rf "${B}/node"; mkdir -p "${B}/node"
    tar -xJf "${dl}/${tarball}" -C "${B}/node" --strip-components=1
  fi
  log "installing @mariozechner/pi-coding-agent@${HC_PI_VERSION} (inside ubuntu:22.04 for glibc parity)"
  docker run --rm -v "${B}:/opt/harness" --user "$(id -u):$(id -g)" \
    -e HOME=/tmp -e npm_config_cache=/tmp/.npm ubuntu:22.04 \
    /opt/harness/node/bin/node /opt/harness/node/lib/node_modules/npm/bin/npm-cli.js \
    install --prefix /opt/harness/pi --no-audit --no-fund --omit=dev \
    "@mariozechner/pi-coding-agent@${HC_PI_VERSION}"
  pkg="${B}/pi/node_modules/@mariozechner/pi-coding-agent"
  pi_bin_rel="$(python3 -c 'import json,sys; b=json.load(open(sys.argv[1]))["bin"]; print(b["pi"] if isinstance(b,dict) else b)' "${pkg}/package.json")"
  cat > "${B}/bin/pi" << EOF
#!/bin/sh
exec /opt/harness/node/bin/node /opt/harness/pi/node_modules/@mariozechner/pi-coding-agent/${pi_bin_rel} "\$@"
EOF
  chmod 0755 "${B}/bin/pi"

  for ext in permission-gate protected-paths confirm-destructive; do
    if [[ -f "${pkg}/examples/extensions/${ext}.ts" ]]; then
      cp "${pkg}/examples/extensions/${ext}.ts" "${B}/pi-ext/"
    else
      curl -fsSLo "${B}/pi-ext/${ext}.ts" \
        "https://raw.githubusercontent.com/badlogic/pi-mono/${HC_PI_EXT_REF}/packages/coding-agent/examples/extensions/${ext}.ts"
    fi
  done
  log "pi wrapper + guardrail extensions ready"
fi

# ------------------------------------------------------------ manifest
python3 - "${B}" "${hone_sha}" << 'PY'
import hashlib, json, os, pathlib, sys
b = pathlib.Path(sys.argv[1])
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest() if p.exists() else None
m = {
    "hone_ref": os.environ.get("HC_HONE_REF"), "hone_sha": sys.argv[2] or None,
    "hone_bin_sha256": sha(b / "bin" / "hone"),
    "pi_version": os.environ.get("HC_PI_VERSION"), "node_version": os.environ.get("HC_NODE_VERSION"),
    "pi_ext_sha256": {p.name: sha(p) for p in sorted((b / "pi-ext").glob("*.ts"))},
    "helpers_sha256": {p.name: sha(p) for p in sorted((b / "bin").glob("*.py"))},
    "rust_image": os.environ.get("HC_RUST_IMAGE"),
}
(b / "manifest.json").write_text(json.dumps(m, indent=2) + "\n")
print(json.dumps(m, indent=2))
PY
log "bundle ready at ${B}"
