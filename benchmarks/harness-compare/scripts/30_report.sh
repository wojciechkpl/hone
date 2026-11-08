#!/usr/bin/env bash
# Step P1.11 — aggregate a campaign: runs.csv, redteam.csv, report.md, pass_at_1.png,
# plus a publishable trajectory bundle (proxy JSONL + harness logs + patches).
# Usage: scripts/30_report.sh <campaign-name> [--package]
source "$(dirname "$0")/lib.sh"
load_env
name="${1:?campaign name}"; shift || true
out="$(campaign_dir "${name}")"
prereg="${HC_ROOT}/prereg/${HC_PREREG:-${name}}"
args=(--campaign "${out}"); [[ -f "${prereg}/selection.json" ]] && args+=(--prereg "${prereg}")
HC_PY_WITH="--with matplotlib" hc_py hcbench.report "${args[@]}"

if [[ "${1:-}" == "--package" ]]; then
  pkg="${out}/publish"
  rm -rf "${pkg}"; mkdir -p "${pkg}"   # rebuilt from scratch: no stale SHA256SUMS, no nesting
  suites=()
  for s in a b c smoke; do [[ -d "${out}/${s}" ]] && suites+=("${s}"); done
  (( ${#suites[@]} )) || die "nothing to package"
  tar -C "${out}" -czf "${pkg}/trajectories.tar.gz" --exclude='*.tmp' "${suites[@]}"
  for f in report.md runs.csv redteam.csv pass_at_1.png invocations.jsonl; do
    [[ -f "${out}/${f}" ]] && cp "${out}/${f}" "${pkg}/"
  done
  [[ -d "${prereg}" ]] && cp -r "${prereg}" "${pkg}/prereg"
  if [[ -d "${out}/grading" ]]; then
    mkdir -p "${pkg}/grading"
    cp "${out}/grading/outcomes.csv" "${pkg}/grading/" 2>/dev/null || true
    find "${out}/grading" -name predictions.jsonl -o -name '*.json' -maxdepth 3 \
      | while read -r f; do rel="${f#"${out}/grading/"}"; mkdir -p "${pkg}/grading/$(dirname "${rel}")"; cp "${f}" "${pkg}/grading/${rel}"; done
  fi
  if command -v sha256sum >/dev/null; then hasher=(sha256sum); else hasher=(shasum -a 256); fi
  (cd "${pkg}" && find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 "${hasher[@]}" > SHA256SUMS)
  log "publication bundle: ${pkg}"
fi
