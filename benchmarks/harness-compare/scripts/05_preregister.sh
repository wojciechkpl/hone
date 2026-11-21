#!/usr/bin/env bash
# Step P1.6 — pre-register: select suite-A tasks (stratified, seeded), write
# preregistration.toml, pull the instance images. Then commit + tag BEFORE any run.
#
# Usage: scripts/05_preregister.sh <name>        e.g. 2026-11
#        scripts/05_preregister.sh <name> --draft (allow CHANGE-ME placeholders)
#        scripts/05_preregister.sh <name> --stage cb [--draft]   (also --stage=cb, --stage a)
#        scripts/05_preregister.sh <name> --stage a --rules prereg/<cb name>/preregistration.toml
#   Stage cb (campaign 1: suites C and B) has no task list: it skips task selection and
#   image pulls and records the suite A rules; DEVIATIONS.md is still written.
#   Stage a (suite A on SWE-rebench) needs --rules (the stage cb file). It selects the tasks,
#   pulls and pins their images, checks each image, then writes the pre-registration. Further
#   arguments (--draft, --accept-diff KEY) go to hcbench.prereg. HC_SKIP_PULL=1 needs --draft.
source "$(dirname "$0")/lib.sh"
load_env
name="${1:?usage: $0 <name> [--draft]}"; shift || true
dir="${HC_ROOT}/prereg/${name}"

# --stage cb | --stage=cb | --stage a | --stage=a; anything else stops before any file is made.
stage=a
expect=""
draft=0
rules_path=""
accept=()
for arg in "$@"; do
  if [[ -n "${expect}" ]]; then
    case "${expect}" in
    stage) stage="${arg}" ;;
    rules) rules_path="${arg}" ;;
    accept) accept+=(--accept-diff "${arg}") ;;
    esac
    expect=""; continue
  fi
  case "${arg}" in
  --stage) expect=stage ;;
  --stage=*) stage="${arg#--stage=}" ;;
  --rules) expect=rules ;;
  --rules=*) rules_path="${arg#--rules=}" ;;
  --accept-diff) expect=accept ;;
  --accept-diff=*) accept+=(--accept-diff "${arg#--accept-diff=}") ;;
  --draft) draft=1 ;;
  *) ;;
  esac
done
[[ "${expect}" == stage ]] && die "--stage needs a value: a or cb"
[[ -n "${expect}" ]] && die "--${expect/accept/accept-diff} needs a value"
case "${stage}" in
a | cb) ;;
*) die "unknown --stage '${stage}': use a or cb" ;;
esac
if [[ "${stage}" == a ]]; then
  [[ -n "${rules_path}" ]] || die "stage a needs --rules prereg/<cb name>/preregistration.toml (the stage cb file)"
  [[ "${HC_SKIP_PULL:-0}" != 1 ]] || (( draft )) || die "HC_SKIP_PULL=1 is accepted only with --draft"
  # K117: check the rules, config, bundle and files against the stage cb file before any work.
  pf=(); (( draft )) && pf=(--draft)
  hc_py hcbench.prereg --stage a --rules "${rules_path}" --preflight ${pf[@]+"${pf[@]}"} ${accept[@]+"${accept[@]}"}
fi
mkdir -p "${dir}"

if [[ "${stage}" == cb ]]; then
  hc_py hcbench.prereg --out "${dir}/preregistration.toml" "$@"
else
  HC_PY_WITH="--with datasets" hc_py hcbench.rebench select \
    --dataset "${HC_A_DATASET}" --revision "${HC_A_REVISION:-}" \
    --min-split "${HC_A_MIN_SPLIT}" --min-created "${HC_A_MIN_CREATED}" \
    --n "${HC_A_N}" --seed "${HC_SELECTION_SEED}" --grader "${HC_A_GRADER}" --out "${dir}"
  python3 - "${dir}/instances.jsonl" > "${dir}/images.list" << 'PY'
import json, sys
for line in open(sys.argv[1]):
    if line.strip():
        print(json.loads(line)["image"])
PY
  if [[ "${HC_SKIP_PULL:-0}" != 1 ]]; then
    log "pulling $(wc -l < "${dir}/images.list" | tr -d ' ') SWE-rebench task images (large; HC_SKIP_PULL=1 with --draft skips)"
    xargs -n1 -P4 docker pull -q < "${dir}/images.list" >/dev/null
    # Pin what was pulled: tags like :latest can move between pre-registration and grading.
    while read -r img; do
      printf '%s %s\n' "${img}" "$(docker image inspect -f '{{index .RepoDigests 0}}' "${img}" 2>/dev/null || echo MISSING)"
    done < "${dir}/images.list" > "${dir}/images.digests"
    grep -q MISSING "${dir}/images.digests" && warn "some images failed to pull (see images.digests)"
    hc_py hcbench.rebench check-images --dir "${dir}"
  fi
  hc_py hcbench.prereg --selection "${dir}/selection.json" --out "${dir}/preregistration.toml" "$@"
fi

[[ -f "${dir}/DEVIATIONS.md" ]] || cat > "${dir}/DEVIATIONS.md" << 'EOF'
# Deviations from the pre-registration

Every change made after the pre-registration tag, with the date, the reason and
the runs it affects. An empty list means none.

| date | change | reason | affected runs |
|---|---|---|---|
EOF

cat << EOF

Pre-registration written to ${dir}. Review it, then (by hand):

  git add benchmarks/harness-compare/prereg/${name}
  git commit -m "bench: pre-register harness comparison ${name}"
  git tag bench-prereg-${name}
  git push origin HEAD --tags        # the public timestamp is the point

EOF
