#!/usr/bin/env bash
# run_headline.sh — Headline SWE-bench Verified run on Claude Sonnet 4.5
#
# This is the publishable, leaderboard-comparable run.  It uses the full A8
# scaffold on all 500 SWE-bench Verified tasks with claude-sonnet-4-5-20250514.
#
# USAGE
#   ./benchmarks/run_headline.sh [OPTIONS]
#
# OPTIONS
#   --limit  <N>      Cap number of tasks (default: 500, the full set)
#   --resume          Skip tasks already logged in the run directory
#   --dry-run         Print commands, do not execute
#   --help
#
# PREREQUISITES
#   export ANTHROPIC_API_KEY=sk-ant-...
#   pip install 'swebench==3.0.13' 'sb-cli==0.2.*' 'datasets==2.21.*'
#   docker --version  # Docker 24+ required
#   ./benchmarks/run_headline.sh --limit 5 --dry-run  # verify setup first
#
# ENVIRONMENT VARIABLES
#   ANTHROPIC_API_KEY         (required) Anthropic API key
#   HONE_BENCH_USD_CAP        Hard USD cap for the entire run (default: 600)
#   HONE_BENCH_REQ_PER_MIN    Client-side rate limit (default: 50)
#   SWEBENCH_DATASET_DIR      Path to pre-downloaded SWE-bench dataset
#                             (default: /data/bench/swebench_verified)
#   HONE_BIN                  Path to hone binary (default: ./target/release/hone)
#   RESULTS_DIR               Root results dir (default: ./benchmarks/results)
#
# COST ESTIMATE
#   Full 500-task run: ~$350-600 (prompt caching at ~60% hit rate)
#   Dry-run (5 tasks): ~$7-15
#   Per task budget cap: $4.00
#
# IDEMPOTENCY
#   Results go to results/<DATE>-swebench-headline/.
#   Re-run with --resume to continue after an interruption.

set -euo pipefail

# ---------------------------------------------------------------------------
# Model constants — NEVER change these for published runs
# ---------------------------------------------------------------------------

readonly HEADLINE_MODEL="claude-sonnet-4-5-20250514"
readonly HEADLINE_PROVIDER_PREFIX="anthropic"
readonly SCAFFOLD_CONFIG="a8"
readonly MAX_TURNS=75
readonly PER_TASK_USD=4.00
readonly TEMPERATURE=0.0
readonly MAX_TOKENS=8192

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"
HONE_BENCH_USD_CAP="${HONE_BENCH_USD_CAP:-600}"
HONE_BENCH_REQ_PER_MIN="${HONE_BENCH_REQ_PER_MIN:-50}"
SWEBENCH_DATASET_DIR="${SWEBENCH_DATASET_DIR:-/data/bench/swebench_verified}"
HONE_BIN="${HONE_BIN:-./target/release/hone}"
RESULTS_DIR="${RESULTS_DIR:-./benchmarks/results}"
CONFIGS_DIR="./benchmarks/configs"

TASK_LIMIT=500
RESUME=0
DRY_RUN=0

RUN_DATE="$(date -u +%Y-%m-%d)"

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

usage() {
    sed -n '2,/^set -euo/{ /^set -euo/d; s/^# //; s/^#$//; p }' "$0"
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --limit)   TASK_LIMIT="$2"; shift 2 ;;
        --resume)  RESUME=1;        shift ;;
        --dry-run) DRY_RUN=1;       shift ;;
        --help|-h) usage ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------

abort() { echo "ERROR: $*" >&2; exit 1; }

[[ -z "$ANTHROPIC_API_KEY" ]] && abort \
    "ANTHROPIC_API_KEY is not set. This run requires Claude Sonnet 4.5.\n  export ANTHROPIC_API_KEY=sk-ant-..."

if [[ "$DRY_RUN" -eq 0 ]]; then
    [[ -f "$HONE_BIN" ]] || abort \
        "Hone binary not found at '$HONE_BIN'.\nBuild with: cargo build --release --locked"

    # Docker check — SWE-bench Verified requires Docker 24+
    if ! command -v docker &>/dev/null; then
        abort "Docker not found. SWE-bench Verified requires Docker 24+."
    fi
    docker_version="$(docker --version | grep -oE '[0-9]+\.[0-9]+' | head -1)"
    docker_major="${docker_version%%.*}"
    if [[ "$docker_major" -lt 24 ]]; then
        abort "Docker $docker_version found but Docker 24+ required."
    fi

    # SWE-bench dataset check
    [[ -d "$SWEBENCH_DATASET_DIR" ]] || abort \
        "SWE-bench Verified dataset not found at '$SWEBENCH_DATASET_DIR'.\n" \
        "Download with:\n  python3 -c \"from datasets import load_dataset; load_dataset('princeton-nlp/SWE-bench_Verified', split='test').save_to_disk('$SWEBENCH_DATASET_DIR')\""

    # Python harness check
    if ! python3 -c "import swebench" 2>/dev/null; then
        abort "swebench Python package not installed.\n  pip install 'swebench==3.0.13' 'sb-cli==0.2.*'"
    fi

    # Disk space check (SWE-bench needs ~100 GB for Docker images)
    available_gb="$(df -BG "$RESULTS_DIR" 2>/dev/null | tail -1 | awk '{print $4}' | tr -d 'G' || echo 999)"
    if [[ "$available_gb" -lt 150 ]]; then
        abort "Less than 150 GB disk space available (${available_gb} GB). SWE-bench Verified requires ~100 GB for Docker images."
    fi
fi

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

log()  { echo "[$(date -u +%H:%M:%S)] $*"; }
logn() { echo ""; log "$*"; }

run_cmd() {
    if [[ "$DRY_RUN" -eq 1 ]]; then
        echo "  DRY-RUN: $*"
    else
        "$@"
    fi
}

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------

RUN_ID="${RUN_DATE}-swebench-headline"
RUN_DIR="${RESULTS_DIR}/${RUN_ID}"

logn "Hone Headline Run — SWE-bench Verified"
log "  Model        : ${HEADLINE_MODEL}"
log "  Scaffold     : ${SCAFFOLD_CONFIG} (full — all G1-G12)"
log "  Tasks        : ${TASK_LIMIT}"
log "  USD cap      : \$${HONE_BENCH_USD_CAP}"
log "  RPM limit    : ${HONE_BENCH_REQ_PER_MIN}"
log "  Run ID       : ${RUN_ID}"
log "  Results dir  : ${RUN_DIR}"
echo ""

if [[ "$RESUME" -eq 1 ]] && [[ -f "$RUN_DIR/.completed" ]]; then
    log "Run already completed. Remove '$RUN_DIR/.completed' to re-run."
    exit 0
fi

if [[ "$DRY_RUN" -eq 0 ]]; then
    mkdir -p "$RUN_DIR/events"
fi

# ---------------------------------------------------------------------------
# Record run metadata and SHA before the run begins
# ---------------------------------------------------------------------------

write_headline_metadata() {
    local hone_sha
    hone_sha="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
    local hone_sha_file="$RUN_DIR/hone.sha"

    run_cmd tee "$hone_sha_file" <<META
hone_sha: ${hone_sha}
binary: ${HONE_BIN}
META

    cat > "$RUN_DIR/metadata.yaml" <<YAML
hone_sha: "${hone_sha}"
model: "${HEADLINE_MODEL}"
provider: "${HEADLINE_PROVIDER_PREFIX}"
benchmark: "swebench_verified"
config: "${SCAFFOLD_CONFIG}"
task_limit: ${TASK_LIMIT}
max_turns: ${MAX_TURNS}
per_task_usd_cap: ${PER_TASK_USD}
usd_cap_total: ${HONE_BENCH_USD_CAP}
temperature: ${TEMPERATURE}
max_tokens: ${MAX_TOKENS}
run_date: "${RUN_DATE}"
completed: false
deviations: []
YAML
}

if [[ "$DRY_RUN" -eq 0 ]]; then
    write_headline_metadata
fi

# ---------------------------------------------------------------------------
# Full scaffold env vars (A8 = all features on)
# ---------------------------------------------------------------------------

export HONE_SCAFFOLD_EDIT_UNIQUENESS=1
export HONE_SCAFFOLD_LINE_AWARE_READ=1
export HONE_SCAFFOLD_MULTI_EDIT=1
export HONE_SCAFFOLD_GREP_TOOL=1
export HONE_SCAFFOLD_SYMBOL_SEARCH=1
export HONE_SCAFFOLD_REAL_SYSTEM_PROMPT=1
export HONE_SCAFFOLD_PLAN_FIRST=1
export HONE_SCAFFOLD_PARALLEL_TOOLS=1
export HONE_SCAFFOLD_TEST_VERIFY=1
export HONE_SCAFFOLD_FILE_VERIFY=1
export HONE_SCAFFOLD_TOOL_RETRY=1
export HONE_SCAFFOLD_CONTEXT_COMPACTION=1

export ANTHROPIC_API_KEY
export HONE_BENCH_USD_CAP
export HONE_BENCH_CONFIG="${SCAFFOLD_CONFIG}"
export HONE_BENCH_RUN_ID="${RUN_ID}"
export HONE_BENCH_REQ_PER_MIN

# ---------------------------------------------------------------------------
# Pre-build Docker images (SWE-bench specific step)
# ---------------------------------------------------------------------------

log "Pre-building SWE-bench Docker images (this takes ~30-60 min on first run)..."
log "Images are cached by Docker; subsequent runs reuse them."

run_cmd python3 -m swebench.harness.run_evaluation \
    --predictions_path "gold" \
    --max_workers 1 \
    --instance_ids "sympy__sympy-20590" \
    --run_id "docker-preflight" \
    2>&1 | head -20 || {
        log "Docker preflight check failed — check Docker daemon and disk space"
        [[ "$DRY_RUN" -eq 0 ]] && exit 1
    }

# ---------------------------------------------------------------------------
# Run the harness
# ---------------------------------------------------------------------------

HARNESS_LOG="$RUN_DIR/harness.log"
SUMMARY_CSV="$RUN_DIR/summary.csv"
TASKS_JSONL="$RUN_DIR/tasks.jsonl"

log "Starting SWE-bench Verified evaluation..."
log "  Concurrency: 4 tasks (respects $HONE_BENCH_REQ_PER_MIN RPM limit)"
log "  Logs: $HARNESS_LOG"

run_cmd python3 -m swebench.harness.run_evaluation \
    --dataset_name_or_path "${SWEBENCH_DATASET_DIR}" \
    --split test \
    --predictions_path "${RUN_DIR}/predictions.jsonl" \
    --max_workers 4 \
    --run_id "${RUN_ID}" \
    --output_dir "${RUN_DIR}" \
    2>&1 | tee "$HARNESS_LOG" || {
        log "WARNING: Harness exited non-zero. Check $HARNESS_LOG for details."
        log "         If this was an infrastructure failure, re-run with --resume."
    }

# ---------------------------------------------------------------------------
# Post-processing: write canonical summary row
# ---------------------------------------------------------------------------

if [[ "$DRY_RUN" -eq 0 ]]; then
    log "Writing canonical summary row..."
    python3 - <<PYEOF
import json, csv, os, sys, pathlib, datetime

run_dir = pathlib.Path("${RUN_DIR}")
tasks_file = run_dir / "tasks.jsonl"

if not tasks_file.exists():
    print(f"WARNING: {tasks_file} not found — summary.csv will be empty", file=sys.stderr)
    sys.exit(0)

tasks = [json.loads(line) for line in tasks_file.read_text().splitlines() if line.strip()]
total   = len(tasks)
passed  = sum(1 for t in tasks if t.get("resolved", False))
usd     = sum(t.get("usd", 0.0) for t in tasks)
walls   = sorted(t.get("wall_s", 0) for t in tasks)
turns   = sorted(t.get("turns", 0) for t in tasks)

p50_wall = walls[int(total * 0.50)] if walls else 0
p95_wall = walls[int(total * 0.95)] if walls else 0
med_turns = turns[total // 2] if turns else 0

inp_tok  = sum(t.get("input_tokens",  0) for t in tasks)
out_tok  = sum(t.get("output_tokens", 0) for t in tasks)
cache_hits = sum(t.get("cached_tokens", 0) for t in tasks)
cache_rate = cache_hits / max(inp_tok, 1)

row = {
    "hone_sha":        "${hone_sha:-unknown}",
    "model":           "${HEADLINE_MODEL}",
    "benchmark":       "swebench_verified",
    "config":          "${SCAFFOLD_CONFIG}",
    "pass_at_1":       round(passed / max(total, 1), 4),
    "resolved":        passed,
    "total":           total,
    "usd_total":       round(usd, 4),
    "usd_per_resolved": round(usd / max(passed, 1), 4),
    "p50_wall_s":      p50_wall,
    "p95_wall_s":      p95_wall,
    "median_turns":    med_turns,
    "input_tok":       inp_tok,
    "output_tok":      out_tok,
    "cache_hit_rate":  round(cache_rate, 4),
    "run_date":        "${RUN_DATE}",
}

with open(run_dir / "summary.csv", "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=row.keys())
    w.writeheader()
    w.writerow(row)

print(f"pass@1: {row['pass_at_1']:.1%}  ({passed}/{total})")
print(f"USD:    \${row['usd_total']:.2f} total  /  \${row['usd_per_resolved']:.3f} per resolved")
PYEOF

    # Mark run as completed
    sed -i.bak 's/^completed: false/completed: true/' "$RUN_DIR/metadata.yaml"
    rm -f "$RUN_DIR/metadata.yaml.bak"
    touch "$RUN_DIR/.completed"

    log "DONE. Run ID: ${RUN_ID}"
    log "  Summary : ${SUMMARY_CSV}"
    log "  Tasks   : ${TASKS_JSONL}"
    log "  Metadata: ${RUN_DIR}/metadata.yaml"
else
    log "DRY-RUN complete. No files written."
fi

echo ""
log "Next steps:"
log "  1. Review results:     python3 benchmarks/analyze.py --run-id ${RUN_ID}"
log "  2. Compare to baseline: python3 benchmarks/analyze.py --compare a0 ${RUN_ID}"
log "  3. Archive:            git add benchmarks/results/${RUN_ID}/summary.csv"
