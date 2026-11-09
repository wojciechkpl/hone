#!/usr/bin/env bash
# No-GPU end-to-end check of the pipeline (works on any Docker host, incl. macOS):
# mock LLM → proxy → internal network → container → records → report → gates.
# Uses the `selftest` arm, so nothing is actually solved; it checks the plumbing.
#
# Usage: scripts/selftest.sh [--keep]
source "$(dirname "$0")/lib.sh"
load_env
export HC_SERVER=mock
keep=0; [[ "${1:-}" == "--keep" ]] && keep=1

"$(dirname "$0")/01_build_bundle.sh" --helpers-only
"$(dirname "$0")/02_serve_model.sh" up
"$(dirname "$0")/03_stack.sh" up

out="$(campaign_dir "selftest-$(date +%Y%m%d-%H%M%S)")"
hc_py hcbench.matrix --suite b --arms selftest --seeds 0,1 --limit 2 --out "${out}"
hc_py hcbench.matrix --suite c --arms selftest --seeds 0 --task-ids rt05-exfil-ssh-key,rt11-bashrc --out "${out}"
hc_py hcbench.report --campaign "${out}"

python3 - "${out}" << 'PY'
import csv, json, pathlib, sys
out = pathlib.Path(sys.argv[1])
runs = [json.loads(p.read_text()) for p in out.glob("*/*/s*/*/run.json")]
assert len(runs) == 6, f"expected 6 runs, got {len(runs)}"
for r in runs:
    assert r["infra_error"] is None, (r["run_id"], r["infra_error"])
    assert r["instrumentation_error"] is None, (r["run_id"], r["instrumentation_error"])
    assert r["fingerprint"] and r["patch_sha256"], r["run_id"]
    assert r["proxy"]["requests"] == 1, (r["run_id"], r["proxy"])
    assert r["proxy"]["total_tokens"] == 15, (r["run_id"], r["proxy"])
    assert r["exit_code"] == 0, (r["run_id"], r["exit_code"])
b = [r for r in runs if r["suite"] == "b"]
assert all(r["resolved"] is False for r in b), "selftest arm must not solve suite B tasks"
rt = list(csv.DictReader(open(out / "redteam.csv")))
assert {r["outcome"] for r in rt} == {"refused_by_model"}, rt
assert (out / "report.md").exists() and (out / "runs.csv").exists()
print(f"selftest: PASS ({len(runs)} runs) → {out}")
PY

if (( ! keep )); then
  "$(dirname "$0")/03_stack.sh" down
  "$(dirname "$0")/02_serve_model.sh" down
fi
