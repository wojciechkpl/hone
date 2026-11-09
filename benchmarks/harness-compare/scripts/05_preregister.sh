#!/usr/bin/env bash
# Step P1.6 — pre-register: select suite-A tasks (stratified, seeded), write
# preregistration.toml, pull the instance images. Then commit + tag BEFORE any run.
#
# Usage: scripts/05_preregister.sh <name>        e.g. 2026-11
#        scripts/05_preregister.sh <name> --draft (allow CHANGE-ME placeholders)
source "$(dirname "$0")/lib.sh"
load_env
name="${1:?usage: $0 <name> [--draft]}"; shift || true
dir="${HC_ROOT}/prereg/${name}"
mkdir -p "${dir}"

rev=(); [[ -n "${HC_SWE_DATASET_REVISION:-}" ]] && rev=(--revision "${HC_SWE_DATASET_REVISION}")
HC_PY_WITH="--with datasets" hc_py hcbench.select_tasks \
  --dataset "${HC_SWE_DATASET}" --split "${HC_SWE_SPLIT}" "${rev[@]}" \
  --n "${HC_SWE_N}" --seed "${HC_SELECTION_SEED}" --out "${dir}"
hc_py hcbench.prereg --selection "${dir}/selection.json" --out "${dir}/preregistration.toml" "$@"

[[ -f "${dir}/DEVIATIONS.md" ]] || cat > "${dir}/DEVIATIONS.md" << 'EOF'
# Deviations from the pre-registration

Every change made after the pre-registration tag, with the date, the reason and
the runs it affects. An empty list means none.

| date | change | reason | affected runs |
|---|---|---|---|
EOF

python3 - "${dir}/instances.jsonl" "${HC_SWE_NAMESPACE}" > "${dir}/images.list" << 'PY'
import json, os, sys
sys.path.insert(0, os.environ["HC_ROOT"])
from hcbench.suites import swebench_image
for line in open(sys.argv[1]):
    if line.strip():
        inst = json.loads(line)
        print(inst.get("image") or swebench_image(inst["instance_id"], sys.argv[2]))
PY
if [[ "${HC_SKIP_PULL:-0}" != 1 ]]; then
  log "pulling ${HC_SWE_N} SWE-bench instance images (large; set HC_SKIP_PULL=1 to skip)"
  xargs -n1 -P4 docker pull -q < "${dir}/images.list" >/dev/null
  # Pin what was pulled: tags like :latest can move between pre-registration and grading.
  while read -r img; do
    printf '%s %s\n' "${img}" "$(docker image inspect -f '{{index .RepoDigests 0}}' "${img}" 2>/dev/null || echo MISSING)"
  done < "${dir}/images.list" > "${dir}/images.digests"
  grep -q MISSING "${dir}/images.digests" && warn "some images failed to pull (see images.digests)"
fi

cat << EOF

Pre-registration written to ${dir}. Review it, then (by hand):

  git add benchmarks/harness-compare/prereg/${name}
  git commit -m "bench: pre-register harness comparison ${name}"
  git tag bench-prereg-${name}
  git push origin HEAD --tags        # the public timestamp is the point

EOF
