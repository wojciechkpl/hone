#!/usr/bin/env bash
# Phase 0 gate — fail while unverified SWE-bench claims remain in user-facing text.
# The 95% figure was a patch-generation rate on 20 tasks, extrapolated to 300
# (benchmarks/results/swe-lite-v4-summary.json); the only graded number is 58/300.
# Usage: scripts/check_claims.sh
source "$(dirname "$0")/lib.sh"
cd "${REPO_ROOT}" || exit 1

patterns=(
  '285/300'
  '258/300'
  '95% pass rate'
  '86% pass rate'
  '100% patch rate'
  '\+76pp'
  '\+400%'
  '5x better pass rate'
  'improving patch rate by \+20pp'
  'improves pass rate from 19\.3% to'
)
targets=(README.md CHANGELOG.md docs bins/hone/src/main.rs crates/hone-core/src/context_injector.rs)
found=0
for p in "${patterns[@]}"; do
  # Lines that explicitly mark a figure as withdrawn may quote it (changelog corrections).
  if hits="$(grep -rInE --include='*.md' --include='*.rs' --include='*.html' -e "${p}" "${targets[@]}" 2>/dev/null \
            | grep -v 'docs/plans/' | grep -v 'docs/research/' | grep -vi 'withdrawn')"; then
    [[ -n "${hits}" ]] && { echo "== ${p}"; echo "${hits}"; found=1; }
  fi
done
if (( found )); then
  echo; echo "check_claims: FAIL — replace with the graded number (58/300, 19.3%, official grader) or remove."
  exit 1
fi
echo "check_claims: PASS"
