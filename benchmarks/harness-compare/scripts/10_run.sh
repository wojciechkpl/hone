#!/usr/bin/env bash
# Steps P1.7–P1.9 — run a suite for a campaign (resumable; re-run to continue).
#
# Usage: scripts/10_run.sh <suite a|b|c> <campaign-name> [extra matrix args...]
#   a: SWE-bench Lite subset — needs prereg/<prereg-name>/ (HC_PREREG, default = campaign name)
#   b: mini-bench (10 tasks)        c: red-team (25 scenarios, serial)
source "$(dirname "$0")/lib.sh"
load_env
suite="${1:?suite a|b|c}"; name="${2:?campaign name}"; shift 2
out="$(campaign_dir "${name}")"
prereg="${HC_ROOT}/prereg/${HC_PREREG:-${name}}"

case "${suite}" in
a)
  [[ -f "${prereg}/tasks.txt" ]] || die "no pre-registration at ${prereg} (run 05_preregister.sh)"
  git -C "${REPO_ROOT}" tag --list 'bench-prereg-*' | grep -q . || warn "no bench-prereg-* tag found — commit and tag the pre-registration first"
  hc_py hcbench.matrix --suite a --arms "${HC_ARMS_MAIN}" --seeds "${HC_REPEATS}" \
    --instances "${prereg}/instances.jsonl" --tasks-file "${prereg}/tasks.txt" \
    --concurrency "${HC_CONCURRENCY}" --out "${out}" "$@"
  ;;
b)
  hc_py hcbench.matrix --suite b --arms "${HC_ARMS_MAIN}" --seeds "${HC_REPEATS}" \
    --concurrency "${HC_CONCURRENCY}" --out "${out}" "$@"
  ;;
c)
  hc_py hcbench.matrix --suite c --arms "${HC_ARMS_REDTEAM}" --seeds "${HC_REPEATS}" \
    --out "${out}" "$@"
  ;;
*) die "unknown suite ${suite}" ;;
esac
