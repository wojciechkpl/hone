#!/usr/bin/env bash
# Step P1.10 — grade suite-A patches with Docker.
# Grades against the pre-registered dataset rows (prereg/<name>/dataset.json) when present.
# When prereg/<name>/selection.json names the SWE-rebench grading fork as grader, installs the
# fork at HC_A_GRADER_REF and grades on the pinned images; otherwise swebench==5.0.2 (SWE-bench Lite).
# Usage: scripts/20_grade.sh <campaign-name> [--regrade]
source "$(dirname "$0")/lib.sh"
load_env
name="${1:?campaign name}"; shift
prereg="${HC_ROOT}/prereg/${HC_PREREG:-${name}}"
fork_repo="SWE-rebench/SWE-bench-fork"
grader=""
if [[ -f "${prereg}/selection.json" ]]; then
  grader="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1])).get("grader") or "")' "${prereg}/selection.json")"
fi

# The stage-a file's [grader] table is the pre-registered fork commit and namespace (K131).
py_toml() { # python3 needs tomllib (3.11+); otherwise use the rig's uv python
  if python3 -c 'import tomllib' 2>/dev/null; then python3 "$@"
  else (cd "${HC_ROOT}" && uv run --no-project -q --python 3.12 python "$@"); fi
}
toml_grader() { # key
  py_toml - "${prereg}/preregistration.toml" "$1" << 'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as fh:
    print(tomllib.load(fh).get("grader", {}).get(sys.argv[2], ""))
PY
}

if [[ "${grader}" == "${fork_repo}" ]]; then
  ref="${HC_A_GRADER_REF:-}"; ns="${HC_A_NAMESPACE:-}"
  if [[ -f "${prereg}/preregistration.toml" ]]; then
    reg_ref="$(toml_grader commit)"; reg_ns="$(toml_grader namespace)"
    [[ "${reg_ref}" =~ ^[0-9a-f]{40}$ ]] || die "the [grader] commit in ${prereg}/preregistration.toml is not 40 lowercase hex characters: '${reg_ref}'"
    [[ -z "${ref}" || "${ref}" == "${reg_ref}" ]] || die "HC_A_GRADER_REF '${ref}' differs from the pre-registered fork commit '${reg_ref}'"
    [[ -z "${ns}" || "${ns}" == "${reg_ns}" ]] || die "HC_A_NAMESPACE '${ns}' differs from the pre-registered namespace '${reg_ns}'"
    ref="${reg_ref}"; ns="${reg_ns}"
  else
    warn "no ${prereg}/preregistration.toml; using HC_A_GRADER_REF and HC_A_NAMESPACE"
  fi
  [[ "${ref}" =~ ^[0-9a-f]{40}$ ]] || die "HC_A_GRADER_REF must be 40 lowercase hex characters, got '${ref}'"
  need uv
  (cd "${HC_ROOT}" && uv run --no-project -q --python 3.12 --with "aiohttp>=3.9" \
    --with "swebench @ git+https://github.com/${fork_repo}.git@${ref}" \
    python -m hcbench.grade --campaign "$(campaign_dir "${name}")" \
    --dataset "${prereg}/dataset.json" --namespace "${ns}" --cache-level instance \
    --image-digests "${prereg}/images.digests" \
    --grader-repo "${fork_repo}" --grader-commit "${ref}" \
    --max-workers "${HC_GRADE_WORKERS}" "$@")
else
  dataset="${HC_SWE_DATASET}"
  if [[ -f "${prereg}/dataset.json" ]]; then
    dataset="${prereg}/dataset.json"
  else
    warn "no ${prereg}/dataset.json; grading against the live hub dataset ${HC_SWE_DATASET}"
  fi
  HC_PY_WITH="--with swebench==5.0.2" hc_py hcbench.grade --campaign "$(campaign_dir "${name}")" \
    --dataset "${dataset}" --split "${HC_SWE_SPLIT}" --max-workers "${HC_GRADE_WORKERS}" "$@"
fi
