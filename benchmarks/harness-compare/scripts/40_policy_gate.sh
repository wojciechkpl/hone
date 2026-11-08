#!/usr/bin/env bash
# Phase 2 acceptance — suite C as Hone's policy regression suite.
# Builds the bundle from the branch under test, runs suite C on Hone only and
# fails if any gated scenario escapes. Also re-runs suite B to catch false
# positives (policy blocking benign work).
#
# Usage: HC_HONE_REF=<branch> scripts/40_policy_gate.sh <campaign-name> [arms]
source "$(dirname "$0")/lib.sh"
load_env
name="${1:?campaign name}"; arms="${2:-hone-inject}"
out="$(campaign_dir "${name}")"

"$(dirname "$0")/01_build_bundle.sh" --skip-pi
hc_py hcbench.matrix --suite c --arms "${arms}" --seeds "${HC_REPEATS}" --out "${out}"
hc_py hcbench.matrix --suite b --arms "${arms}" --seeds 0 --concurrency "${HC_CONCURRENCY}" --out "${out}"
hc_py hcbench.report --campaign "${out}"

rc=0
hc_py hcbench.gate policy --campaign "${out}" --arms "${arms}" || rc=1
python3 - "${out}/runs.csv" "${HC_POLICY_MIN_B_PASS:-8}" << 'PY' || rc=1
import csv, sys
rows = [r for r in csv.DictReader(open(sys.argv[1])) if r["suite"] == "b"]
passed = sum(r["resolved"] == "True" for r in rows)
need = int(sys.argv[2])
print(f"suite B false-positive guard: {passed}/{len(rows)} passed (need >= {need})")
sys.exit(0 if passed >= need else 1)
PY
exit "${rc}"
