#!/usr/bin/env bash
# Record an asciinema demo of Hone solving a coding task.
# Produces assets/demo.cast which can be embedded in README via asciinema-player.
#
# Usage: bash benchmarks/record_demo.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
CAST_FILE="${REPO_ROOT}/assets/demo.cast"
HONE="${REPO_ROOT}/target/release/hone"

if [[ ! -x "${HONE}" ]]; then
    echo "Building hone..."
    cargo build --release --bin hone
fi

# Create a temporary project for the demo
DEMO_DIR=$(mktemp -d)
cd "${DEMO_DIR}"
git init -q

cat > calculator.py << 'PYEOF'
def add(a, b):
    return a + b

def subtract(a, b):
    return a - b

def divide(a, b):
    return a / b

def multiply(a, b):
    return a * b
PYEOF

cat > test_calculator.py << 'PYEOF'
from calculator import add, subtract, divide, multiply

def test_add():
    assert add(2, 3) == 5

def test_subtract():
    assert subtract(5, 3) == 2

def test_divide():
    assert divide(10, 2) == 5.0
    assert divide(10, 0) is None  # BUG: should handle division by zero

def test_multiply():
    assert multiply(3, 4) == 12
PYEOF

git add -A && git commit -q -m "initial"

echo "Recording demo to ${CAST_FILE}..."

# Record the asciinema session
asciinema rec "${CAST_FILE}" \
  --title "Hone — AI Coding Agent" \
  --idle-time-limit 2 \
  --command "${HONE} --model openai:${HONE_OPENAI_MODEL:-deepseek/deepseek-chat} 'The test_divide test fails because divide(10, 0) raises ZeroDivisionError instead of returning None. Fix the divide function in calculator.py to handle division by zero gracefully.'"

# Cleanup
rm -rf "${DEMO_DIR}"

echo ""
echo "Demo recorded: ${CAST_FILE}"
echo "Upload to asciinema.org: asciinema upload ${CAST_FILE}"
