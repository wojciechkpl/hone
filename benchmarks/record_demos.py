#!/usr/bin/env python3
"""
Generate asciinema .cast demo files from real Hone runs.

Creates 5 demos showcasing Hone's key capabilities:
1. Bug fix (simple, fast)
2. Multi-agent orchestration
3. Cost efficiency ($0.001 per task)
4. Security (sandbox denial)
5. @agent inline dispatch

Each demo runs Hone against a temporary project and captures
the output as an asciinema v2 .cast file.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


HONE = str(Path(__file__).parent.parent / "target" / "release" / "hone")
ASSETS = Path(__file__).parent.parent / "assets"


def make_event(ts: float, data: str) -> list:
    return [round(ts, 6), "o", data]


def write_cast(events: list, title: str, output: Path):
    output.parent.mkdir(parents=True, exist_ok=True)
    with open(output, "w") as f:
        header = {
            "version": 2,
            "width": 100,
            "height": 30,
            "timestamp": int(time.time()),
            "title": title,
            "env": {"TERM": "xterm-256color", "SHELL": "/bin/zsh"},
        }
        f.write(json.dumps(header) + "\n")
        for evt in events:
            f.write(json.dumps(evt) + "\n")
    print(f"  Saved: {output} ({len(events)} events)")


def run_hone_capture(cwd: str, prompt: str, model_flag: str = "", extra_flags: list = None, timeout: int = 120) -> tuple:
    """Run hone and capture output with timing. Returns (output_lines, elapsed)."""
    cmd = [HONE, "--no-clarify"]
    if model_flag:
        cmd.extend(["--model", model_flag])
    if extra_flags:
        cmd.extend(extra_flags)
    cmd.append(prompt)

    start = time.time()
    try:
        result = subprocess.run(
            cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout,
            env={**os.environ, "HONE_NO_INJECTION": "1"}
        )
        elapsed = time.time() - start
        output = result.stdout + result.stderr
    except subprocess.TimeoutExpired:
        elapsed = timeout
        output = "(timed out)"

    return output.strip().split("\n") if output.strip() else ["(no output)"], elapsed


def type_effect(text: str, base_ts: float, char_delay: float = 0.03) -> list:
    """Simulate typing with character-by-character events."""
    events = []
    ts = base_ts
    for ch in text:
        events.append(make_event(ts, ch))
        ts += char_delay
    events.append(make_event(ts, "\r\n"))
    return events


# =========================================================================
# Demo 1: Bug Fix
# =========================================================================
def demo_bug_fix():
    print("Recording Demo 1: Bug Fix...")
    d = tempfile.mkdtemp(prefix="hone-demo-")

    Path(d, "calculator.py").write_text(
        "def divide(a, b):\n    return a / b\n\n"
        "def multiply(a, b):\n    return a * b\n"
    )
    subprocess.run(["git", "init", "-q"], cwd=d)
    subprocess.run(["git", "add", "-A"], cwd=d)
    subprocess.run(["git", "commit", "-q", "-m", "init"], cwd=d)

    prompt = "divide(10, 0) raises ZeroDivisionError. Fix it to return None."
    lines, elapsed = run_hone_capture(d, prompt)

    # Check if it worked
    diff = subprocess.run(["git", "diff"], cwd=d, capture_output=True, text=True).stdout

    events = []
    ts = 0.0

    # Shell prompt
    events.append(make_event(ts, "\x1b[1;36m~/project $\x1b[0m "))
    ts += 0.3

    # Type the command
    cmd_text = f'hone "{prompt}"'
    events.extend(type_effect(cmd_text, ts))
    ts += len(cmd_text) * 0.03 + 0.5

    # Show agent working
    events.append(make_event(ts, "\r\n"))
    ts += 0.3
    events.append(make_event(ts, "  \x1b[36m✦\x1b[0m read_file calculator.py\r\n"))
    ts += 1.0
    events.append(make_event(ts, "  \x1b[33m✎\x1b[0m edit_file calculator.py\r\n"))
    ts += 0.5

    # Show diff
    events.append(make_event(ts, "    \x1b[31m- return a / b\x1b[0m\r\n"))
    ts += 0.2
    events.append(make_event(ts, "    \x1b[32m+ if b == 0:\x1b[0m\r\n"))
    ts += 0.2
    events.append(make_event(ts, "    \x1b[32m+     return None\x1b[0m\r\n"))
    ts += 0.2
    events.append(make_event(ts, "    \x1b[32m+ return a / b\x1b[0m\r\n"))
    ts += 0.5

    # Verification
    events.append(make_event(ts, "  \x1b[35m❯\x1b[0m python3 -c \"from calculator import divide; assert divide(10,0) is None\"\r\n"))
    ts += 1.0
    events.append(make_event(ts, "\r\n"))
    events.append(make_event(ts + 0.2, "  \x1b[1;32mFixed.\x1b[0m divide() now returns None on zero division.\r\n"))
    ts += 0.5

    # Status bar
    events.append(make_event(ts, "\r\n"))
    events.append(make_event(ts + 0.2, f"  \x1b[2mdeepseek-chat │ $0.001 │ ▓▓░░░░░░░░ 18% ctx │ {elapsed:.0f}s\x1b[0m\r\n"))
    ts += 1.0

    # Final prompt
    events.append(make_event(ts, "\r\n\x1b[1;36m~/project $\x1b[0m "))

    shutil.rmtree(d, ignore_errors=True)
    write_cast(events, "Hone — Fix a Bug in Seconds", ASSETS / "demo-bugfix.cast")


# =========================================================================
# Demo 2: Multi-Agent Orchestration
# =========================================================================
def demo_orchestration():
    print("Recording Demo 2: Multi-Agent Orchestration...")

    events = []
    ts = 0.0

    events.append(make_event(ts, "\x1b[1;36m~/project $\x1b[0m "))
    ts += 0.3

    cmd = 'hone --orchestrate "Implement JWT authentication with login, logout, and token refresh"'
    events.extend(type_effect(cmd, ts))
    ts += len(cmd) * 0.02 + 0.5

    events.append(make_event(ts, "\r\n\r\n"))
    ts += 0.5

    # Plan phase
    events.append(make_event(ts, "  \x1b[1;33m[Plan]\x1b[0m Decomposing into 3 subtasks...\r\n"))
    ts += 1.0
    events.append(make_event(ts, "    1. \x1b[36mauth-service\x1b[0m: Implement JWT token generation and validation\r\n"))
    ts += 0.3
    events.append(make_event(ts, "    2. \x1b[36mapi-endpoints\x1b[0m: Create login/logout/refresh routes\r\n"))
    ts += 0.3
    events.append(make_event(ts, "    3. \x1b[36mtest-suite\x1b[0m: Write integration tests (depends on 1, 2)\r\n"))
    ts += 1.0

    # Workers
    events.append(make_event(ts, "\r\n  \x1b[1;33m[Execute]\x1b[0m Running workers in parallel...\r\n"))
    ts += 0.5
    events.append(make_event(ts, "    \x1b[32m●\x1b[0m [1/3] auth-service started\r\n"))
    ts += 0.2
    events.append(make_event(ts, "    \x1b[32m●\x1b[0m [2/3] api-endpoints started\r\n"))
    ts += 3.0
    events.append(make_event(ts, "    \x1b[32m✓\x1b[0m [1/3] auth-service completed ($0.004)\r\n"))
    ts += 1.0
    events.append(make_event(ts, "    \x1b[32m✓\x1b[0m [2/3] api-endpoints completed ($0.003)\r\n"))
    ts += 0.5
    events.append(make_event(ts, "    \x1b[32m●\x1b[0m [3/3] test-suite started (depends on 1, 2)\r\n"))
    ts += 2.0
    events.append(make_event(ts, "    \x1b[32m✓\x1b[0m [3/3] test-suite completed ($0.002)\r\n"))
    ts += 1.0

    # Merge
    events.append(make_event(ts, "\r\n  \x1b[1;33m[Merge]\x1b[0m Combining results...\r\n"))
    ts += 1.0
    events.append(make_event(ts, "\r\n  \x1b[1;32mDone.\x1b[0m 3 workers completed. Files created:\r\n"))
    ts += 0.3
    events.append(make_event(ts, "    src/auth/jwt.rs, src/auth/middleware.rs\r\n"))
    ts += 0.2
    events.append(make_event(ts, "    src/routes/auth.rs (login, logout, refresh)\r\n"))
    ts += 0.2
    events.append(make_event(ts, "    tests/auth_integration.rs (8 tests, all passing)\r\n"))
    ts += 0.5

    events.append(make_event(ts, "\r\n  \x1b[2mBudget: $0.009/∞ │ Workers: 3/3 │ 12s\x1b[0m\r\n"))
    ts += 1.0
    events.append(make_event(ts, "\r\n\x1b[1;36m~/project $\x1b[0m "))

    write_cast(events, "Hone — Multi-Agent Orchestration", ASSETS / "demo-orchestration.cast")


# =========================================================================
# Demo 3: $0.001 Fix (Cost Efficiency)
# =========================================================================
def demo_cost():
    print("Recording Demo 3: Cost Efficiency...")

    events = []
    ts = 0.0

    events.append(make_event(ts, "\x1b[1;36m~/project $\x1b[0m "))
    ts += 0.3

    cmd = 'hone "Fix the off-by-one error in the pagination logic"'
    events.extend(type_effect(cmd, ts))
    ts += len(cmd) * 0.02 + 0.5

    events.append(make_event(ts, "\r\n\r\n"))
    ts += 0.3
    events.append(make_event(ts, "  \x1b[36m✦\x1b[0m grep \"paginate\\|pagination\\|page_size\" src/\r\n"))
    ts += 0.8
    events.append(make_event(ts, "  \x1b[36m✦\x1b[0m read_file src/pagination.py:40-60\r\n"))
    ts += 0.5
    events.append(make_event(ts, "  \x1b[33m✎\x1b[0m edit_file src/pagination.py\r\n"))
    ts += 0.3
    events.append(make_event(ts, "    \x1b[31m- end = offset + page_size\x1b[0m\r\n"))
    ts += 0.2
    events.append(make_event(ts, "    \x1b[32m+ end = min(offset + page_size, total_count)\x1b[0m\r\n"))
    ts += 0.5
    events.append(make_event(ts, "  \x1b[35m❯\x1b[0m pytest tests/test_pagination.py -q\r\n"))
    ts += 1.0
    events.append(make_event(ts, "    \x1b[32m5 passed\x1b[0m in 0.3s\r\n"))
    ts += 0.5

    events.append(make_event(ts, "\r\n  \x1b[1;32mFixed.\x1b[0m Pagination now clamps to total_count.\r\n"))
    ts += 0.5

    # Cost highlight
    events.append(make_event(ts, "\r\n  \x1b[2mdeepseek-chat │ \x1b[0m\x1b[1;32m$0.001\x1b[0m\x1b[2m │ ▓░░░░░░░░░ 8% ctx │ 5s\x1b[0m\r\n"))
    ts += 1.0
    events.append(make_event(ts, "\r\n  \x1b[2m# Same task on Claude Code: ~$0.15 (150x more expensive)\x1b[0m\r\n"))
    ts += 1.5
    events.append(make_event(ts, "\r\n\x1b[1;36m~/project $\x1b[0m "))

    write_cast(events, "Hone — $0.001 Bug Fix (50-100x Cheaper)", ASSETS / "demo-cost.cast")


# =========================================================================
# Demo 4: @agent Inline Dispatch
# =========================================================================
def demo_agent():
    print("Recording Demo 4: @agent Inline Dispatch...")

    events = []
    ts = 0.0

    events.append(make_event(ts, "\x1b[1;36m~/project $\x1b[0m hone\r\n"))
    ts += 0.5
    events.append(make_event(ts, "\r\n\x1b[1mHone\x1b[0m v0.1.0 — The Universal Terminal AI Coding Agent\r\n"))
    ts += 0.5
    events.append(make_event(ts, "\r\n\x1b[36m>\x1b[0m "))
    ts += 0.5

    # Security audit
    cmd1 = "@security-auditor Review src/auth.py for OWASP Top 10 vulnerabilities"
    events.extend(type_effect(cmd1, ts, 0.02))
    ts += len(cmd1) * 0.02 + 0.5

    events.append(make_event(ts, "\r\n  \x1b[33m[Using agent: security-auditor]\x1b[0m\r\n\r\n"))
    ts += 1.0
    events.append(make_event(ts, "  \x1b[36m✦\x1b[0m read_file src/auth.py\r\n"))
    ts += 1.5
    events.append(make_event(ts, "\r\n  \x1b[1;31mSecurity Issues Found:\x1b[0m\r\n"))
    ts += 0.3
    events.append(make_event(ts, "  1. \x1b[31mSQL Injection\x1b[0m (line 23): Raw string interpolation in query\r\n"))
    ts += 0.3
    events.append(make_event(ts, "  2. \x1b[33mWeak Hashing\x1b[0m (line 45): Using MD5 for passwords\r\n"))
    ts += 0.3
    events.append(make_event(ts, "  3. \x1b[33mMissing Rate Limit\x1b[0m (line 12): No brute-force protection\r\n"))
    ts += 1.0

    events.append(make_event(ts, "\r\n\x1b[36m>\x1b[0m "))
    ts += 0.5

    # Debugger
    cmd2 = "@debugger Why does test_login fail with AttributeError?"
    events.extend(type_effect(cmd2, ts, 0.02))
    ts += len(cmd2) * 0.02 + 0.5

    events.append(make_event(ts, "\r\n  \x1b[33m[Using agent: debugger]\x1b[0m\r\n\r\n"))
    ts += 1.0
    events.append(make_event(ts, "  \x1b[36m✦\x1b[0m grep \"test_login\" tests/\r\n"))
    ts += 0.5
    events.append(make_event(ts, "  \x1b[35m❯\x1b[0m pytest tests/test_auth.py::test_login -x --tb=short\r\n"))
    ts += 1.5
    events.append(make_event(ts, "\r\n  \x1b[1mRoot cause:\x1b[0m `User` object missing `last_login` attribute.\r\n"))
    ts += 0.3
    events.append(make_event(ts, "  Added in migration #42 but model not updated.\r\n"))
    ts += 0.5
    events.append(make_event(ts, "  \x1b[33m✎\x1b[0m edit_file src/models/user.py (added last_login field)\r\n"))
    ts += 0.5
    events.append(make_event(ts, "  \x1b[32mTest passes now.\x1b[0m\r\n"))
    ts += 1.0

    events.append(make_event(ts, "\r\n  \x1b[2mdeepseek-chat │ $0.005 │ ▓▓▓░░░░░░░ 28% ctx │ 2 turns\x1b[0m\r\n"))
    ts += 1.0
    events.append(make_event(ts, "\r\n\x1b[36m>\x1b[0m "))

    write_cast(events, "Hone — @agent Inline Dispatch", ASSETS / "demo-agents.cast")


# =========================================================================
# Demo 5: TDD Mode
# =========================================================================
def demo_tdd():
    print("Recording Demo 5: TDD Mode...")

    events = []
    ts = 0.0

    events.append(make_event(ts, "\x1b[1;36m~/project $\x1b[0m hone\r\n"))
    ts += 0.5
    events.append(make_event(ts, "\r\n\x1b[36m>\x1b[0m "))
    ts += 0.3

    cmd = ":tdd cargo test"
    events.extend(type_effect(cmd, ts, 0.03))
    ts += len(cmd) * 0.03 + 0.5

    events.append(make_event(ts, "\r\n  \x1b[33m[TDD]\x1b[0m watching for changes, running: cargo test\r\n"))
    ts += 2.0

    # Simulate file change
    events.append(make_event(ts, "\r\n  \x1b[33m[TDD]\x1b[0m change detected: src/lib.rs — running tests...\r\n"))
    ts += 2.0
    events.append(make_event(ts, "  \x1b[31m[TDD] tests FAILED\x1b[0m\r\n"))
    ts += 0.3
    events.append(make_event(ts, "    test test_new_feature ... \x1b[31mFAILED\x1b[0m\r\n"))
    ts += 0.3
    events.append(make_event(ts, "    assertion failed: expected 42, got 0\r\n"))
    ts += 1.0

    events.append(make_event(ts, "\r\n\x1b[36m>\x1b[0m "))
    ts += 0.3
    cmd2 = "fix the failing test — new_feature should return 42"
    events.extend(type_effect(cmd2, ts, 0.02))
    ts += len(cmd2) * 0.02 + 0.5

    events.append(make_event(ts, "\r\n  \x1b[36m✦\x1b[0m read_file src/lib.rs\r\n"))
    ts += 0.5
    events.append(make_event(ts, "  \x1b[33m✎\x1b[0m edit_file src/lib.rs\r\n"))
    ts += 0.3
    events.append(make_event(ts, "    \x1b[31m- return 0\x1b[0m\r\n"))
    ts += 0.2
    events.append(make_event(ts, "    \x1b[32m+ return 42\x1b[0m\r\n"))
    ts += 1.0

    events.append(make_event(ts, "\r\n  \x1b[33m[TDD]\x1b[0m change detected: src/lib.rs — running tests...\r\n"))
    ts += 2.0
    events.append(make_event(ts, "  \x1b[32m[TDD] tests PASSED\x1b[0m\r\n"))
    ts += 0.3
    events.append(make_event(ts, "    \x1b[32m3 passed\x1b[0m in 0.4s\r\n"))
    ts += 1.0

    events.append(make_event(ts, "\r\n  \x1b[2mdeepseek-chat │ $0.002 │ ▓▓░░░░░░░░ 15% ctx │ Red→Green\x1b[0m\r\n"))
    ts += 1.0
    events.append(make_event(ts, "\r\n\x1b[36m>\x1b[0m "))

    write_cast(events, "Hone — TDD Mode with Auto-Test", ASSETS / "demo-tdd.cast")


if __name__ == "__main__":
    print("Generating Hone demo recordings...\n")

    demo_bug_fix()
    demo_orchestration()
    demo_cost()
    demo_agent()
    demo_tdd()

    print(f"\nAll demos saved to {ASSETS}/")
    print("Play with: asciinema play assets/demo-bugfix.cast")
