#!/usr/bin/env bash
# mini_bench.sh — Self-contained benchmark for Hone
#
# Runs Hone against a set of coding tasks in temporary repos and
# checks if the output meets acceptance criteria. No external harness needed.
#
# Usage:
#   OPENAI_API_KEY=... HONE_OPENAI_BASE_URL=... HONE_OPENAI_MODEL=... \
#     bash benchmarks/mini_bench.sh [--limit N]
#
# Results written to benchmarks/results/mini-<date>/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HONE="${SCRIPT_DIR}/../target/release/hone"

# Parse args
LIMIT=10
while [[ $# -gt 0 ]]; do
    case "$1" in
        --limit) LIMIT="$2"; shift 2 ;;
        *) LIMIT="$1"; shift ;;
    esac
done
DATE=$(date +%Y-%m-%d-%H%M)
RESULTS_DIR="${SCRIPT_DIR}/results/mini-${DATE}"
mkdir -p "${RESULTS_DIR}"

# Verify hone binary exists
if [[ ! -x "${HONE}" ]]; then
    echo "ERROR: hone binary not found. Run 'cargo build --release' first." >&2
    exit 1
fi

# Verify API key
if [[ -z "${OPENAI_API_KEY:-}" ]]; then
    echo "ERROR: OPENAI_API_KEY not set." >&2
    exit 1
fi

PASSED=0
FAILED=0
TOTAL=0
TOTAL_COST=0

log() { echo "[$(date +%H:%M:%S)] $*"; }

run_task() {
    local name="$1"
    local setup="$2"
    local prompt="$3"
    local check="$4"

    TOTAL=$((TOTAL + 1))
    if [[ ${TOTAL} -gt ${LIMIT:=10} ]]; then
        TOTAL=$((TOTAL - 1))
        return
    fi

    local task_dir
    task_dir=$(mktemp -d)
    local task_results="${RESULTS_DIR}/${name}"
    mkdir -p "${task_results}"

    log "Task ${TOTAL}/${LIMIT}: ${name}"

    # Setup the task repo
    cd "${task_dir}"
    git init -q
    eval "${setup}"
    git add -A && git commit -q -m "initial" 2>/dev/null || true

    # Run hone
    local start_time
    start_time=$(date +%s)

    OPENAI_API_KEY="${OPENAI_API_KEY}" \
    HONE_OPENAI_BASE_URL="${HONE_OPENAI_BASE_URL:-}" \
    HONE_OPENAI_MODEL="${HONE_OPENAI_MODEL:-gpt-4o}" \
    "${HONE}" --model "openai:${HONE_OPENAI_MODEL:-gpt-4o}" \
        "${prompt}" > "${task_results}/output.txt" 2>&1 || true

    local end_time
    end_time=$(date +%s)
    local duration=$((end_time - start_time))

    # Check acceptance criteria
    cd "${task_dir}"
    if eval "${check}"; then
        PASSED=$((PASSED + 1))
        echo "PASS" > "${task_results}/status.txt"
        log "  PASS (${duration}s)"
    else
        FAILED=$((FAILED + 1))
        echo "FAIL" > "${task_results}/status.txt"
        log "  FAIL (${duration}s)"
    fi

    echo "${duration}" > "${task_results}/duration.txt"

    # Capture final state
    git diff > "${task_results}/diff.txt" 2>/dev/null || true
    cp -r . "${task_results}/workspace" 2>/dev/null || true

    # Cleanup
    rm -rf "${task_dir}"
    cd "${SCRIPT_DIR}"
}

log "Hone Mini-Benchmark"
log "  Model: ${HONE_OPENAI_MODEL:-gpt-4o}"
log "  Base URL: ${HONE_OPENAI_BASE_URL:-default}"
log "  Limit: ${LIMIT} tasks"
log "  Results: ${RESULTS_DIR}"
log ""

# ---------------------------------------------------------------------------
# Task definitions
# ---------------------------------------------------------------------------

run_task "fix-typo" \
    'cat > main.py << "PYEOF"
def calcualte_average(numbers):
    """Calcualte the average of a list of numbers."""
    if len(numbers) == 0:
        return 0
    return sum(numbers) / len(numbers)

def main():
    result = calcualte_average([1, 2, 3, 4, 5])
    print(f"Average: {result}")

if __name__ == "__main__":
    main()
PYEOF' \
    "Fix the typo in the function name in main.py — it should be calculate_average not calcualte_average" \
    'grep -q "def calculate_average" main.py'

run_task "add-function" \
    'cat > math_utils.py << "PYEOF"
def add(a, b):
    return a + b

def subtract(a, b):
    return a - b
PYEOF' \
    "Add a multiply function to math_utils.py that takes two arguments and returns their product" \
    'grep -q "def multiply" math_utils.py && python3 -c "from math_utils import multiply; assert multiply(3,4)==12"'

run_task "fix-bug-off-by-one" \
    'cat > range_check.py << "PYEOF"
def is_in_range(value, low, high):
    """Check if value is in range [low, high] inclusive."""
    return low < value < high

assert is_in_range(5, 1, 10) == True
assert is_in_range(1, 1, 10) == True   # This fails! Boundary not inclusive
assert is_in_range(10, 1, 10) == True  # This fails! Boundary not inclusive
PYEOF' \
    "Fix the bug in range_check.py — is_in_range should be inclusive on both ends (use <= instead of <)" \
    'python3 range_check.py'

run_task "add-error-handling" \
    'cat > divide.py << "PYEOF"
def safe_divide(a, b):
    return a / b
PYEOF' \
    "Add error handling to safe_divide in divide.py — return None when dividing by zero instead of raising an exception" \
    'python3 -c "from divide import safe_divide; assert safe_divide(10,2)==5.0; assert safe_divide(10,0) is None"'

run_task "create-file" \
    'mkdir -p src' \
    "Create a file src/greeting.py with a function greet(name) that returns the string Hello, {name}!" \
    'python3 -c "from src.greeting import greet; assert greet(\"World\")==\"Hello, World!\""'

run_task "rename-variable" \
    'cat > config.py << "PYEOF"
x = "localhost"
y = 5432
z = "mydb"

def get_connection_string():
    return f"postgresql://{x}:{y}/{z}"
PYEOF' \
    "Rename the variables in config.py to be descriptive: x->host, y->port, z->database" \
    'grep -q "host" config.py && grep -q "port" config.py && grep -q "database" config.py && ! grep -q "^x " config.py'

run_task "add-docstring" \
    'cat > utils.py << "PYEOF"
def flatten(nested_list):
    result = []
    for item in nested_list:
        if isinstance(item, list):
            result.extend(flatten(item))
        else:
            result.append(item)
    return result
PYEOF' \
    "Add a docstring to the flatten function in utils.py explaining what it does" \
    'python3 -c "from utils import flatten; assert flatten.__doc__ is not None and len(flatten.__doc__) > 10"'

run_task "fix-import" \
    'cat > app.py << "PYEOF"
from mathutils import calculate_sum

def main():
    result = calculate_sum([1, 2, 3])
    print(result)
PYEOF
cat > math_utils.py << "PYEOF"
def calculate_sum(numbers):
    return sum(numbers)
PYEOF' \
    "Fix the import in app.py — the module is called math_utils not mathutils" \
    'python3 app.py'

run_task "add-test" \
    'cat > calculator.py << "PYEOF"
def add(a, b):
    return a + b

def subtract(a, b):
    return a - b

def multiply(a, b):
    return a * b
PYEOF' \
    "Create a test file test_calculator.py with pytest tests for all three functions in calculator.py" \
    'python3 -m pytest test_calculator.py -q 2>/dev/null || python3 test_calculator.py'

run_task "refactor-duplication" \
    'cat > handlers.py << "PYEOF"
def handle_get(request):
    if not request.get("auth_token"):
        return {"error": "unauthorized", "code": 401}
    if not request.get("user_id"):
        return {"error": "missing user_id", "code": 400}
    return {"data": "get_result", "code": 200}

def handle_post(request):
    if not request.get("auth_token"):
        return {"error": "unauthorized", "code": 401}
    if not request.get("user_id"):
        return {"error": "missing user_id", "code": 400}
    return {"data": "post_result", "code": 200}

def handle_delete(request):
    if not request.get("auth_token"):
        return {"error": "unauthorized", "code": 401}
    if not request.get("user_id"):
        return {"error": "missing user_id", "code": 400}
    return {"data": "delete_result", "code": 200}
PYEOF' \
    "Refactor handlers.py to extract the duplicated auth_token and user_id validation into a shared helper function" \
    'grep -c "auth_token" handlers.py | grep -q "^[1-4]$" && python3 -c "
from handlers import handle_get, handle_post, handle_delete
assert handle_get({})[ \"code\"] == 401
assert handle_get({\"auth_token\": \"x\"})[\"code\"] == 400
assert handle_get({\"auth_token\": \"x\", \"user_id\": \"1\"})[\"code\"] == 200
"'

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

log ""
log "========================================="
log "  Results: ${PASSED} passed, ${FAILED} failed, ${TOTAL} total"
log "  Pass rate: $(( PASSED * 100 / (PASSED + FAILED) ))%"
log "  Results dir: ${RESULTS_DIR}"
log "========================================="

# Write summary CSV
echo "task,status,duration_s" > "${RESULTS_DIR}/summary.csv"
for task_dir in "${RESULTS_DIR}"/*/; do
    task_name=$(basename "${task_dir}")
    [[ "${task_name}" == "summary.csv" ]] && continue
    status=$(cat "${task_dir}/status.txt" 2>/dev/null || echo "UNKNOWN")
    duration=$(cat "${task_dir}/duration.txt" 2>/dev/null || echo "0")
    echo "${task_name},${status},${duration}" >> "${RESULTS_DIR}/summary.csv"
done

# Write metadata
cat > "${RESULTS_DIR}/metadata.yaml" << EOF
model: ${HONE_OPENAI_MODEL:-gpt-4o}
base_url: ${HONE_OPENAI_BASE_URL:-default}
date: $(date -u +%Y-%m-%dT%H:%M:%SZ)
passed: ${PASSED}
failed: ${FAILED}
total: ${TOTAL}
pass_rate: $(( PASSED * 100 / (PASSED + FAILED) ))
EOF

exit $(( FAILED > 0 ? 1 : 0 ))
