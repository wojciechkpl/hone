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

if [[ "${grader}" == "${fork_repo}" ]]; then
  [[ "${HC_A_GRADER_REF:-}" =~ ^[0-9a-f]{40}$ ]] || die "HC_A_GRADER_REF must be 40 lowercase hex characters, got '${HC_A_GRADER_REF:-}'"
  need uv
  (cd "${HC_ROOT}" && uv run --no-project -q --python 3.12 --with "aiohttp>=3.9" \
    --with "swebench @ git+https://github.com/${fork_repo}.git@${HC_A_GRADER_REF}" \
    python -m hcbench.grade --campaign "$(campaign_dir "${name}")" \
    --dataset "${prereg}/dataset.json" --namespace "${HC_A_NAMESPACE:-}" --cache-level instance \
    --image-digests "${prereg}/images.digests" \
    --grader-repo "${fork_repo}" --grader-commit "${HC_A_GRADER_REF}" \
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
