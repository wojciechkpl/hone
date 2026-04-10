#!/usr/bin/env bash
# run_ablation.sh — Run A0-A8 ablation suite on Aider polyglot (225 tasks).
#
# USAGE
#   ./benchmarks/run_ablation.sh [OPTIONS]
#
# OPTIONS
#   --config <a0|a1|...|a8|all>   Which ablation(s) to run  (default: all)
#   --limit  <N>                   Cap tasks per config      (default: 225)
#   --resume                       Skip configs with a completed results dir
#   --dry-run                      Print commands, do not execute
#   --help
#
# ENVIRONMENT VARIABLES (all optional with documented defaults)
#   OPENAI_API_KEY          API key for the cheap-model endpoint
#   HONE_OPENAI_BASE_URL    Base URL  (default: https://api.deepseek.com)
#   HONE_OPENAI_MODEL       Model ID  (default: deepseek-chat)
#   HONE_BENCH_USD_CAP      Hard USD cap per config run  (default: 25)
#   POLYGLOT_BENCH_DIR      Path to cloned Aider polyglot repo
#                           (default: /data/bench/polyglot)
#   HONE_BIN                Path to hone binary
#                           (default: ./target/release/hone)
#   RESULTS_DIR             Root results directory
#                           (default: ./benchmarks/results)
#
# IDEMPOTENCY
#   Each config writes to results/<DATE>-ablation-<CONFIG>/.
#   Re-running with --resume skips any directory that already contains
#   a completed summary.csv (non-empty, with a "completed" marker).
#
# EXAMPLE — dry-run all configs on DeepSeek via OpenRouter
#   OPENAI_API_KEY=sk-or-... \
#   HONE_OPENAI_BASE_URL=https://openrouter.ai/api \
#   HONE_OPENAI_MODEL=deepseek/deepseek-v3 \
#   ./benchmarks/run_ablation.sh --dry-run
#
# EXAMPLE — run only A0 and A6 (validation pair) with 10-task limit
#   ./benchmarks/run_ablation.sh --config a0 --limit 10
#   ./benchmarks/run_ablation.sh --config a6 --limit 10

set -euo pipefail

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

OPENAI_API_KEY="${OPENAI_API_KEY:-}"
HONE_OPENAI_BASE_URL="${HONE_OPENAI_BASE_URL:-https://api.deepseek.com}"
HONE_OPENAI_MODEL="${HONE_OPENAI_MODEL:-deepseek-chat}"
HONE_BENCH_USD_CAP="${HONE_BENCH_USD_CAP:-25}"
POLYGLOT_BENCH_DIR="${POLYGLOT_BENCH_DIR:-/data/bench/polyglot}"
HONE_BIN="${HONE_BIN:-./target/release/hone}"
RESULTS_DIR="${RESULTS_DIR:-./benchmarks/results}"
CONFIGS_DIR="./benchmarks/configs"

SELECTED_CONFIG="all"
TASK_LIMIT=225
RESUME=0
DRY_RUN=0

ALL_CONFIGS=(a0 a1 a2 a3 a4 a5 a6 a7 a8)

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
        --config)  SELECTED_CONFIG="$2"; shift 2 ;;
        --limit)   TASK_LIMIT="$2";      shift 2 ;;
        --resume)  RESUME=1;             shift ;;
        --dry-run) DRY_RUN=1;            shift ;;
        --help|-h) usage ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done

# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------

abort() { echo "ERROR: $*" >&2; exit 1; }

[[ -z "$OPENAI_API_KEY" ]] && abort \
    "OPENAI_API_KEY is not set. Export it before running:\n  export OPENAI_API_KEY=sk-..."

if [[ "$DRY_RUN" -eq 0 ]]; then
    [[ -f "$HONE_BIN" ]] || abort \
        "Hone binary not found at '$HONE_BIN'. Build with: cargo build --release --locked"
    [[ -d "$POLYGLOT_BENCH_DIR" ]] || abort \
        "Aider polyglot benchmark not found at '$POLYGLOT_BENCH_DIR'.\n" \
        "Clone it with:\n  git clone https://github.com/Aider-AI/polyglot-benchmark $POLYGLOT_BENCH_DIR\n  cd $POLYGLOT_BENCH_DIR && git checkout v0.75.0"
fi

if [[ "$SELECTED_CONFIG" == "all" ]]; then
    CONFIGS_TO_RUN=("${ALL_CONFIGS[@]}")
else
    # Validate single config name
    valid=0
    for c in "${ALL_CONFIGS[@]}"; do
        [[ "$c" == "$SELECTED_CONFIG" ]] && valid=1 && break
    done
    [[ "$valid" -eq 0 ]] && abort "Unknown config '$SELECTED_CONFIG'. Valid: ${ALL_CONFIGS[*]}"
    CONFIGS_TO_RUN=("$SELECTED_CONFIG")
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

write_metadata() {
    local run_dir="$1"
    local config_name="$2"
    local config_file="$3"

    local hone_sha
    hone_sha="$(git -C "$(dirname "$HONE_BIN")/../.." rev-parse HEAD 2>/dev/null || echo unknown)"

    cat > "$run_dir/metadata.yaml" <<YAML
hone_sha: "${hone_sha}"
model: "${HONE_OPENAI_MODEL}"
base_url: "${HONE_OPENAI_BASE_URL}"
benchmark: "aider_polyglot"
config: "${config_name}"
config_file: "${config_file}"
task_limit: ${TASK_LIMIT}
usd_cap: ${HONE_BENCH_USD_CAP}
run_date: "${RUN_DATE}"
completed: false
YAML
}

mark_completed() {
    local run_dir="$1"
    # Update the completed flag in metadata.yaml without a full rewrite
    if [[ -f "$run_dir/metadata.yaml" ]]; then
        sed -i.bak 's/^completed: false/completed: true/' "$run_dir/metadata.yaml"
        rm -f "$run_dir/metadata.yaml.bak"
    fi
    # Touch a sentinel so --resume can find it quickly
    touch "$run_dir/.completed"
}

is_completed() {
    local run_dir="$1"
    [[ -f "$run_dir/.completed" ]]
}

# ---------------------------------------------------------------------------
# Scaffold env-var mapping
#
# Each a*.toml documents which features are enabled/disabled.  We translate
# those into HONE_SCAFFOLD_* env vars that main.rs can read at startup.
# This is intentionally explicit — no TOML parsing in bash.
# ---------------------------------------------------------------------------

declare -A SCAFFOLD_FLAGS
# Format: "G1 G2 G3 G4 G5 G6 G7 G8 G9 G10 G11 G12"
#          (1=enabled, 0=disabled, space-separated)
SCAFFOLD_FLAGS[a0]="0 0 0 0 0 0 0 0 0 0 0 0"
SCAFFOLD_FLAGS[a1]="1 0 0 0 0 0 0 0 0 0 0 0"
SCAFFOLD_FLAGS[a2]="1 1 0 0 0 0 0 0 0 0 0 0"
SCAFFOLD_FLAGS[a3]="1 1 1 0 0 0 0 0 0 0 0 0"
SCAFFOLD_FLAGS[a4]="1 1 1 1 0 0 0 0 0 0 0 0"
SCAFFOLD_FLAGS[a5]="1 1 1 1 1 0 0 0 0 0 0 0"
SCAFFOLD_FLAGS[a6]="1 1 1 1 1 1 0 0 0 0 0 0"
SCAFFOLD_FLAGS[a7]="1 1 1 1 1 1 1 1 0 0 0 0"
SCAFFOLD_FLAGS[a8]="1 1 1 1 1 1 1 1 1 1 1 1"

scaffold_env_for() {
    local config="$1"
    local flags="${SCAFFOLD_FLAGS[$config]}"
    read -r g1 g2 g3 g4 g5 g6 g7 g8 g9 g10 g11 g12 <<< "$flags"

    echo "HONE_SCAFFOLD_EDIT_UNIQUENESS=$g1"
    echo "HONE_SCAFFOLD_LINE_AWARE_READ=$g2"
    echo "HONE_SCAFFOLD_MULTI_EDIT=$g3"
    echo "HONE_SCAFFOLD_GREP_TOOL=$g4"
    echo "HONE_SCAFFOLD_SYMBOL_SEARCH=$g5"
    echo "HONE_SCAFFOLD_REAL_SYSTEM_PROMPT=$g6"
    echo "HONE_SCAFFOLD_PLAN_FIRST=$g7"
    echo "HONE_SCAFFOLD_PARALLEL_TOOLS=$g8"
    echo "HONE_SCAFFOLD_TEST_VERIFY=$g9"
    echo "HONE_SCAFFOLD_FILE_VERIFY=$g10"
    echo "HONE_SCAFFOLD_TOOL_RETRY=$g11"
    echo "HONE_SCAFFOLD_CONTEXT_COMPACTION=$g12"
}

# ---------------------------------------------------------------------------
# Per-config run
# ---------------------------------------------------------------------------

run_config() {
    local config_name="$1"
    local config_file="$CONFIGS_DIR/${config_name}.toml"
    local run_id="${RUN_DATE}-ablation-${config_name}"
    local run_dir="${RESULTS_DIR}/${run_id}"

    logn "Config ${config_name^^} — run_id: ${run_id}"

    if [[ "$RESUME" -eq 1 ]] && is_completed "$run_dir"; then
        log "  SKIP: already completed (--resume). Delete '$run_dir/.completed' to re-run."
        return 0
    fi

    # Create result directory structure
    if [[ "$DRY_RUN" -eq 0 ]]; then
        mkdir -p "$run_dir/events"
    fi

    log "  Results dir: $run_dir"
    log "  Config file: $config_file"
    log "  Model: $HONE_OPENAI_MODEL @ $HONE_OPENAI_BASE_URL"
    log "  Task limit: $TASK_LIMIT  |  USD cap: \$$HONE_BENCH_USD_CAP"

    # Write metadata before the run starts
    if [[ "$DRY_RUN" -eq 0 ]]; then
        write_metadata "$run_dir" "$config_name" "$config_file"
    fi

    # Build scaffold env vars for this config
    mapfile -t scaffold_env < <(scaffold_env_for "$config_name")

    # Construct the harness invocation.
    #
    # The Aider polyglot harness expects a command that, given a task directory,
    # produces a unified diff on stdout and exits 0 on success.  We wrap hone
    # in a thin adapter that:
    #   1. Sets the working directory to the task sandbox
    #   2. Injects all HONE_SCAFFOLD_* flags
    #   3. Runs hone in one-shot mode with the task description
    #   4. Captures per-task metrics from hone's structured event log
    #
    # The adapter script is generated inline so the harness sees a single
    # executable command.

    local adapter_script="<dry-run-no-adapter>"
    if [[ "$DRY_RUN" -eq 0 ]]; then
        adapter_script="$(mktemp /tmp/hone-bench-adapter-XXXXXX.sh)"
        chmod +x "$adapter_script"

        cat > "$adapter_script" <<ADAPTER
#!/usr/bin/env bash
# Auto-generated adapter for config ${config_name} — $(date -u)
set -euo pipefail

TASK_DIR="\$1"
TASK_ID="\$2"
EVENT_LOG="${run_dir}/events/\${TASK_ID}.jsonl"

export OPENAI_API_KEY="${OPENAI_API_KEY}"
export HONE_OPENAI_BASE_URL="${HONE_OPENAI_BASE_URL}"
export HONE_OPENAI_MODEL="${HONE_OPENAI_MODEL}"
export HONE_BENCH_CONFIG="${config_name}"
export HONE_BENCH_RUN_ID="${run_id}"
export HONE_BENCH_USD_CAP="${HONE_BENCH_USD_CAP}"
export HONE_EVENT_LOG="\${EVENT_LOG}"

$(printf 'export %s\n' "${scaffold_env[@]}")

cd "\$TASK_DIR"
exec "${HONE_BIN}" --model "openai:${HONE_OPENAI_MODEL}" "\$3"
ADAPTER
    fi

    log "  Adapter script: $adapter_script"

    # Run the polyglot harness
    local harness_log="$run_dir/harness.log"
    local summary_csv="$run_dir/summary.csv"

    if [[ "$DRY_RUN" -eq 1 ]]; then
        echo "  DRY-RUN: python3 ${POLYGLOT_BENCH_DIR}/run_benchmark.py \\"
        echo "             --agent-command $adapter_script \\"
        echo "             --results-dir $run_dir \\"
        echo "             --tasks ${TASK_LIMIT} \\"
        echo "             --timeout 300 \\"
        echo "             2>&1 | tee $harness_log"
    else
        python3 \
            "${POLYGLOT_BENCH_DIR}/run_benchmark.py" \
            --agent-command "$adapter_script" \
            --results-dir "$run_dir" \
            --tasks "${TASK_LIMIT}" \
            --timeout 300 \
            2>&1 | tee "$harness_log" || {
                log "  WARNING: harness exited non-zero — check $harness_log"
            }
    fi

    # Clean up adapter script
    if [[ "$DRY_RUN" -eq 0 ]]; then
        rm -f "$adapter_script"
    fi

    # Mark completed only if summary.csv was produced
    if [[ "$DRY_RUN" -eq 0 ]]; then
        if [[ -f "$summary_csv" && -s "$summary_csv" ]]; then
            mark_completed "$run_dir"
            log "  DONE: $config_name — see $summary_csv"
        else
            log "  WARNING: $config_name run ended but summary.csv is missing or empty"
            log "           Check $harness_log for errors"
        fi
    else
        log "  DRY-RUN complete for $config_name"
    fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

logn "Hone Ablation Harness"
log "  Configs to run : ${CONFIGS_TO_RUN[*]}"
log "  Task limit     : $TASK_LIMIT"
log "  Resume mode    : $RESUME"
log "  Dry-run mode   : $DRY_RUN"
log "  Model          : $HONE_OPENAI_MODEL"
log "  Base URL       : $HONE_OPENAI_BASE_URL"
log "  Results root   : $RESULTS_DIR"
echo ""

if [[ "$DRY_RUN" -eq 0 ]]; then
    mkdir -p "$RESULTS_DIR"
fi

FAILED_CONFIGS=()

for cfg in "${CONFIGS_TO_RUN[@]}"; do
    run_config "$cfg" || {
        log "  ERROR: config $cfg failed — continuing with remaining configs"
        FAILED_CONFIGS+=("$cfg")
    }
done

echo ""
log "All requested configs processed."

if [[ "${#FAILED_CONFIGS[@]}" -gt 0 ]]; then
    log "FAILED configs: ${FAILED_CONFIGS[*]}"
    log "Re-run with: ./benchmarks/run_ablation.sh --config <name> --resume"
    exit 1
fi

log "Run 'make analyze' or 'python3 benchmarks/analyze.py' to generate the comparison table."
