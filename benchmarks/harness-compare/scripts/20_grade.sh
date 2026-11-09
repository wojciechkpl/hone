#!/usr/bin/env bash
# Step P1.10 — grade suite-A patches with the official SWE-bench harness (Docker).
# Grades against the pre-registered dataset rows (prereg/<name>/dataset.json) when present.
# Usage: scripts/20_grade.sh <campaign-name> [--regrade]
source "$(dirname "$0")/lib.sh"
load_env
name="${1:?campaign name}"; shift
prereg="${HC_ROOT}/prereg/${HC_PREREG:-${name}}"
dataset="${HC_SWE_DATASET}"
if [[ -f "${prereg}/dataset.json" ]]; then
  dataset="${prereg}/dataset.json"
else
  warn "no ${prereg}/dataset.json; grading against the live hub dataset ${HC_SWE_DATASET}"
fi
HC_PY_WITH="--with swebench==5.0.2" hc_py hcbench.grade --campaign "$(campaign_dir "${name}")" \
  --dataset "${dataset}" --split "${HC_SWE_SPLIT}" --max-workers "${HC_GRADE_WORKERS}" "$@"
