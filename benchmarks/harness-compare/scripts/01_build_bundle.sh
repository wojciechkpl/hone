#!/usr/bin/env bash
# Step P1.2 — build the read-only harness bundle mounted at /opt/harness:
#   bin/hone            Hone at ${HC_HONE_REF}, built in a clean worktree, glibc ≤ 2.31
#   node/ + pi/         Node ${HC_NODE_VERSION} (official tarball) + Pi ${HC_PI_VERSION}
#   pi-ext/             Pi's example guardrail extensions (pi-guarded arm)
#   bin/hermes + hermes/ + hermes-python/
#                       Hermes Agent ${HC_HERMES_REF} (core install, no gateway/messaging extras),
#                       built for linux/amd64 in its own python-build-standalone interpreter (uv,
#                       checksum-pinned); venv built at the final path /opt/harness/hermes/venv
#   bin/*.py            in-container helpers (snapshot, selftest agent)
#   manifest.json       versions + hashes (copied into the pre-registration)
#
# Usage: scripts/01_build_bundle.sh [--helpers-only] [--skip-hone] [--skip-pi] [--skip-hermes]
source "$(dirname "$0")/lib.sh"
load_env
need docker; need git; need curl

helpers_only=0 skip_hone=0 skip_pi=0 skip_hermes=0
for a in "$@"; do
  case "$a" in
    --helpers-only) helpers_only=1 ;;
    --skip-hone) skip_hone=1 ;;
    --skip-pi) skip_pi=1 ;;
    --skip-hermes) skip_hermes=1 ;;
    *) die "unknown flag $a" ;;
  esac
done

B="${HC_BUNDLE_DIR}"
mkdir -p "${B}/bin" "${B}/pi-ext"
install -m 0755 "${HC_ROOT}"/harness_bin/*.py "${B}/bin/"
log "helpers installed into ${B}/bin"
if (( helpers_only )); then
  # Keep every existing manifest field (hone/pi/hermes provenance); refresh the helper hashes.
  python3 - "${B}" << 'PY'
import hashlib, json, pathlib, sys
b = pathlib.Path(sys.argv[1])
try:
    m = json.loads((b / "manifest.json").read_text())
except (OSError, ValueError):
    m = {}
m["helpers_sha256"] = {
    p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((b / "bin").glob("*.py"))
}
m["helpers_only"] = True
(b / "manifest.json").write_text(json.dumps(m, indent=2) + "\n")
PY
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

# --------------------------------------------------------------- Hermes
# Arm containers are SWE-bench task images with their own Python, and the bundle is mounted
# read-only at /opt/harness. So Hermes gets a self-contained interpreter (python-build-standalone,
# installed by uv into the bundle) and a venv created at the same absolute path it runs from,
# inside a container that mounts the bundle's hermes dirs at /opt/harness/... (venvs are not
# relocatable). Only hermes/ and hermes-python/ are writable in the build container.
#
# Layout: hermes/venv (python + deps), hermes/src (editable checkout), hermes/entry/hermes (the
# console script, alone in its dir: Hermes prepends that dir to PATH in every tool shell, and
# venv/bin would shadow the task's python), (HERMES_BUNDLED_SKILLS points at
# hermes/no-bundled-skills, which must stay nonexistent: skills_sync writes into
# $HERMES_HOME/skills when the dir exists).
hermes_url="https://github.com/NousResearch/hermes-agent"
if (( ! skip_hermes )); then
  : "${HC_HERMES_REF:?HC_HERMES_REF is not set}"
  : "${HC_HERMES_SHA:?HC_HERMES_SHA is not set (full commit SHA of HC_HERMES_REF)}"
  [[ "${HC_HERMES_REF}" != *CHANGE-ME* ]] || die "HC_HERMES_REF is a placeholder"
  [[ "${HC_HERMES_SHA}" =~ ^[0-9a-f]{40}$ ]] || die "HC_HERMES_SHA must be a full 40-hex commit SHA"
  : "${HC_UV_SHA256:?HC_UV_SHA256 is not set}"
  : "${HC_HERMES_BUILD_IMAGE:?HC_HERMES_BUILD_IMAGE is not set}"
  hermes_py="${HC_HERMES_PYTHON:-3.12}"
  uv_version="${HC_UV_VERSION:-0.10.9}"

  # HC_HERMES_REF is a release tag or a full SHA (never a branch). Resolve and cross-check.
  if [[ "${HC_HERMES_REF}" =~ ^[0-9a-f]{40}$ ]]; then
    hermes_sha="${HC_HERMES_REF}"
  else
    hermes_sha="$(git ls-remote "${hermes_url}" "refs/tags/${HC_HERMES_REF}^{}" | awk 'NR==1{print $1}')"
    [[ -n "${hermes_sha}" ]] || hermes_sha="$(git ls-remote "${hermes_url}" "refs/tags/${HC_HERMES_REF}" | awk 'NR==1{print $1}')"
    [[ -n "${hermes_sha}" ]] || die "HC_HERMES_REF=${HC_HERMES_REF} is not a tag in ${hermes_url} (branches are not accepted)"
  fi
  [[ "${hermes_sha}" == "${HC_HERMES_SHA}" ]] || die "HC_HERMES_REF resolves to ${hermes_sha}, but HC_HERMES_SHA=${HC_HERMES_SHA}"

  # uv: official release tarball, verified against the pinned checksum.
  uv_tar="uv-x86_64-unknown-linux-gnu.tar.gz"
  dl="${HC_ROOT}/.build/dl"; mkdir -p "${dl}"
  uv_dir="${dl}/uv-${uv_version}"
  printf '%s  %s\n' "${HC_UV_SHA256}" "${uv_tar}" > "${dl}/${uv_tar}.sha256"
  uv_ok() { (cd "${dl}" && { sha256sum -c "${uv_tar}.sha256" || shasum -a 256 -c "${uv_tar}.sha256"; }) >/dev/null 2>&1; }
  if [[ ! -f "${dl}/${uv_tar}" ]] || ! uv_ok; then
    log "downloading uv ${uv_version}"
    curl -fsSLo "${dl}/${uv_tar}" "https://github.com/astral-sh/uv/releases/download/${uv_version}/${uv_tar}"
  fi
  # Verified on every run (a cached tarball or extracted binary is never trusted unchecked).
  uv_ok || die "uv checksum mismatch (expected HC_UV_SHA256=${HC_UV_SHA256})"
  rm -rf "${uv_dir}"; mkdir -p "${uv_dir}"
  tar -xzf "${dl}/${uv_tar}" -C "${uv_dir}" --strip-components=1

  log "installing hermes-agent ${HC_HERMES_REF} (${hermes_sha:0:12}) with Python ${hermes_py} (linux/amd64, ${HC_HERMES_BUILD_IMAGE})"
  rm -rf "${B}/hermes" "${B}/hermes-python"
  mkdir -p "${B}/hermes" "${B}/hermes-python"
  docker run --rm --platform linux/amd64 \
    -v "${B}/hermes:/opt/harness/hermes" -v "${B}/hermes-python:/opt/harness/hermes-python" \
    -v "${uv_dir}:/uvbin:ro" \
    -e OWNER="$(id -u):$(id -g)" -e HERMES_SHA="${hermes_sha}" \
    -e HERMES_PY="${hermes_py}" -e HERMES_URL="${hermes_url}" \
    "${HC_HERMES_BUILD_IMAGE}" bash -c '
      set -euo pipefail
      trap '"'"'chown -R "${OWNER}" /opt/harness/hermes /opt/harness/hermes-python'"'"' EXIT
      export DEBIAN_FRONTEND=noninteractive HOME=/tmp
      apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates git >/dev/null
      export PATH=/uvbin:${PATH}
      export UV_PYTHON_INSTALL_DIR=/opt/harness/hermes-python UV_CACHE_DIR=/tmp/uv-cache UV_LINK_MODE=copy
      uv python install "${HERMES_PY}"
      # Hermes refuses wheel/sdist builds, so install the project from a checkout (editable) using
      # its hash-verified uv.lock; the checkout stays in the bundle at its final path.
      mkdir -p /opt/harness/hermes/src && cd /opt/harness/hermes/src
      git init -q . && git remote add origin "${HERMES_URL}"
      git fetch -q --depth 1 origin "${HERMES_SHA}" && git checkout -q FETCH_HEAD
      [ "$(git rev-parse HEAD)" = "${HERMES_SHA}" ] || { echo "checked-out HEAD != ${HERMES_SHA}" >&2; exit 1; }
      export UV_PROJECT_ENVIRONMENT=/opt/harness/hermes/venv
      uv sync --frozen --no-dev --managed-python --python "${HERMES_PY}" --compile-bytecode
      # The bundle is read-only at run time, so precompile the source tree now.
      /opt/harness/hermes/venv/bin/python -m compileall -q -x "/(tests|tests-js)/" /opt/harness/hermes/src
      mkdir -p /opt/harness/hermes/entry
      cp /opt/harness/hermes/venv/bin/hermes /opt/harness/hermes/entry/hermes
      /opt/harness/hermes/venv/bin/python -I -B -c "import sys; print(sys.version.split()[0], sys.prefix)"'
  py_patch="$(awk -F' = ' '$1=="version_info"{print $2}' "${B}/hermes/venv/pyvenv.cfg")"
  [[ -n "${py_patch}" ]] || die "cannot read the Python version from ${B}/hermes/venv/pyvenv.cfg"
  HC_HERMES_REF="${HC_HERMES_REF}" python3 - "${B}/hermes/build-info.json" "${hermes_sha}" "${py_patch}" "${uv_version}" << 'PY'
import json, os, sys
json.dump({"hermes_ref": os.environ["HC_HERMES_REF"], "hermes_sha": sys.argv[2],
           "hermes_python": sys.argv[3], "uv_version": sys.argv[4],
           "uv_sha256": os.environ.get("HC_UV_SHA256"), "build_image": os.environ.get("HC_HERMES_BUILD_IMAGE")},
          open(sys.argv[1], "w"), indent=2)
PY
  # No PYTHON* variables are exported here: they would leak into the agent's tool shells.
  # -I (isolated) and -B (no bytecode writes) are python flags, not environment.
  cat > "${B}/bin/hermes" << 'EOF'
#!/bin/sh
exec /opt/harness/hermes/venv/bin/python -I -B /opt/harness/hermes/entry/hermes "$@"
EOF
  chmod 0755 "${B}/bin/hermes"
  log "hermes wrapper ready"
fi

# ------------------------------------------------------------ manifest
# Hermes fields come from hermes/build-info.json (written by the build, kept with --skip-hermes);
# the SHA is cross-checked against the checkout in hermes/src.
hermes_head=""
[[ -d "${B}/hermes/src/.git" ]] && hermes_head="$(git -C "${B}/hermes/src" rev-parse HEAD 2>/dev/null || true)"
python3 - "${B}" "${hone_sha}" "${hermes_head}" << 'PY'
import hashlib, json, os, pathlib, sys
b = pathlib.Path(sys.argv[1])
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest() if p.exists() else None
try:
    hb = json.loads((b / "hermes" / "build-info.json").read_text())
except (OSError, ValueError):
    hb = {}
if hb and hb.get("hermes_sha") != sys.argv[3]:
    sys.exit(f"hermes/build-info.json sha {hb.get('hermes_sha')} != hermes/src HEAD {sys.argv[3]!r}")
m = {
    "hone_ref": os.environ.get("HC_HONE_REF"), "hone_sha": sys.argv[2] or None,
    "hone_bin_sha256": sha(b / "bin" / "hone"),
    "pi_version": os.environ.get("HC_PI_VERSION"), "node_version": os.environ.get("HC_NODE_VERSION"),
    "pi_ext_sha256": {p.name: sha(p) for p in sorted((b / "pi-ext").glob("*.ts"))},
    "helpers_sha256": {p.name: sha(p) for p in sorted((b / "bin").glob("*.py"))},
    "rust_image": os.environ.get("HC_RUST_IMAGE"),
    "hermes_ref": hb.get("hermes_ref"),
    "hermes_sha": hb.get("hermes_sha"),
    "hermes_python": hb.get("hermes_python"),
    "uv_version": hb.get("uv_version"),
    "uv_sha256": hb.get("uv_sha256"),
    "hermes_build_image": hb.get("build_image"),
}
(b / "manifest.json").write_text(json.dumps(m, indent=2) + "\n")
print(json.dumps(m, indent=2))
PY
log "bundle ready at ${B}"
