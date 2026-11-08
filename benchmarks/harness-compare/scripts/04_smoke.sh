#!/usr/bin/env bash
# Step P1.5 — smoke every arm before pre-registration (capability probes, not results):
#   edit-basic       tool calls work end to end through the served model
#   shell-basic      the shell tool runs inside a container (Hone fails closed if
#                    seccomp blocks Landlock)
#   toolchain-probe  /opt/miniconda3 (SWE-bench env) is reachable from the shell
#   landlock-canary  Hone's sandbox denies ~/.ssh inside Docker (Pi expected to leak)
#
# Usage: scripts/04_smoke.sh [campaign-name] [arms]
source "$(dirname "$0")/lib.sh"
load_env

name="${1:-smoke-$(date +%Y%m%d-%H%M)}"
arms="${2:-${HC_ARMS_MAIN},pi-guarded,hone-guarded}"
out="$(campaign_dir "${name}")"

docker pull -q "$(python3 -c "
import sys; sys.path.insert(0, '${HC_ROOT}')
from hcbench.suites import swebench_image; print(swebench_image('django__django-11099', '${HC_SWE_NAMESPACE}'))
")" >/dev/null || warn "could not pull the toolchain-probe SWE-bench image"

hc_py hcbench.matrix --suite smoke --arms "${arms}" --seeds 0 --out "${out}" --force || true
status=0
hc_py hcbench.gate smoke --campaign "${out}" || status=$?

cat << EOF

Interpretation:
  * shell-basic FAIL for hone        → sandbox fails closed in Docker (seccomp). Run Hone's arm
                                       with --security-opt seccomp=<profile allowing landlock_*>
                                       or on the host in a per-task git worktree; log as deviation.
  * toolchain-probe FAIL for hone    → the bundle predates P1.3b (/opt + /usr/local allowlist), or
                                       the toolchain lives elsewhere: set HONE_SANDBOX_EXTRA_READ.
  * landlock-canary FAIL for hone-guarded → Landlock is not enforced in the container even with
                                       HONE_SANDBOX_ENFORCE=1 (check the kernel and seccomp).
  * pi-guarded rows                  → record whether the gates block or allow without a UI.
Results: ${out}
EOF
exit "${status}"
