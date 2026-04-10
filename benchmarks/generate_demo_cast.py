#!/usr/bin/env python3
"""Generate an asciinema .cast file from a real Hone run.

Runs Hone against a task, captures output with timing, and produces
a .cast v2 file that can be played with asciinema-player.
"""

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

def main():
    repo_root = Path(__file__).parent.parent
    hone = str(repo_root / "target" / "release" / "hone")
    cast_file = repo_root / "assets" / "demo.cast"

    if not Path(hone).exists():
        print("Building hone...", file=sys.stderr)
        subprocess.run(["cargo", "build", "--release", "--bin", "hone"],
                       cwd=repo_root, check=True)

    # Create demo project
    demo_dir = Path(tempfile.mkdtemp(prefix="hone-demo-"))

    (demo_dir / "calculator.py").write_text('''\
def add(a, b):
    return a + b

def subtract(a, b):
    return a - b

def divide(a, b):
    return a / b

def multiply(a, b):
    return a * b
''')

    (demo_dir / "test_calculator.py").write_text('''\
from calculator import add, subtract, divide, multiply

def test_add():
    assert add(2, 3) == 5

def test_subtract():
    assert subtract(5, 3) == 2

def test_divide():
    assert divide(10, 2) == 5.0
    assert divide(10, 0) is None  # Should handle zero division

def test_multiply():
    assert multiply(3, 4) == 12
''')

    subprocess.run(["git", "init", "-q"], cwd=demo_dir, check=True)
    subprocess.run(["git", "add", "-A"], cwd=demo_dir, check=True)
    subprocess.run(["git", "commit", "-q", "-m", "initial"], cwd=demo_dir, check=True)

    prompt = (
        "The test_divide test fails because divide(10, 0) raises "
        "ZeroDivisionError instead of returning None. Fix the divide "
        "function in calculator.py to handle division by zero."
    )

    model = os.environ.get("HONE_OPENAI_MODEL", "deepseek/deepseek-chat")

    # Capture output with timing
    print(f"Running Hone demo (model: {model})...", file=sys.stderr)

    env = os.environ.copy()
    start = time.time()

    proc = subprocess.Popen(
        [hone, "--model", f"openai:{model}", prompt],
        cwd=demo_dir,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
    )

    events = []
    # Add initial prompt display
    events.append(make_event(0.0, f"\x1b[1;36m$ hone\x1b[0m \"{prompt[:80]}...\"\r\n"))

    buf = b""
    while True:
        chunk = proc.stdout.read(1)
        if not chunk:
            break
        buf += chunk
        # Flush on newline or after accumulating some bytes
        if chunk == b"\n" or len(buf) > 80:
            elapsed = time.time() - start
            try:
                text = buf.decode("utf-8", errors="replace")
                events.append(make_event(elapsed + 0.5, text))
            except Exception:
                pass
            buf = b""

    if buf:
        elapsed = time.time() - start
        events.append(make_event(elapsed + 0.5, buf.decode("utf-8", errors="replace")))

    proc.wait()
    total = time.time() - start

    # Add final status
    events.append(make_event(total + 1.0, f"\r\n\x1b[1;32mDone in {total:.0f}s\x1b[0m\r\n"))

    # Check if fix worked
    result = subprocess.run(
        ["python3", "-c",
         "from calculator import divide; assert divide(10,0) is None; print('Fix verified!')"],
        cwd=demo_dir, capture_output=True, text=True
    )
    if result.returncode == 0:
        events.append(make_event(total + 1.5,
            f"\x1b[1;36m$ python3 -c \"from calculator import divide; assert divide(10,0) is None\"\x1b[0m\r\n"))
        events.append(make_event(total + 2.0, "\x1b[1;32mFix verified!\x1b[0m\r\n"))

    # Show the diff
    diff = subprocess.run(["git", "diff"], cwd=demo_dir, capture_output=True, text=True)
    if diff.stdout:
        events.append(make_event(total + 2.5, f"\r\n\x1b[1;36m$ git diff\x1b[0m\r\n"))
        for line in diff.stdout.split("\n"):
            t = total + 2.8
            if line.startswith("+") and not line.startswith("+++"):
                events.append(make_event(t, f"\x1b[32m{line}\x1b[0m\r\n"))
            elif line.startswith("-") and not line.startswith("---"):
                events.append(make_event(t, f"\x1b[31m{line}\x1b[0m\r\n"))
            else:
                events.append(make_event(t, f"{line}\r\n"))

    # Write .cast file (asciinema v2 format)
    cast_file.parent.mkdir(parents=True, exist_ok=True)
    with open(cast_file, "w") as f:
        header = {
            "version": 2,
            "width": 120,
            "height": 35,
            "timestamp": int(time.time()),
            "title": "Hone — AI Coding Agent Demo",
            "env": {"TERM": "xterm-256color", "SHELL": "/bin/zsh"},
        }
        f.write(json.dumps(header) + "\n")
        for evt in events:
            f.write(json.dumps(evt) + "\n")

    # Cleanup
    import shutil
    shutil.rmtree(demo_dir, ignore_errors=True)

    print(f"\nDemo recorded: {cast_file}", file=sys.stderr)
    print(f"  {len(events)} events, {total:.0f}s runtime", file=sys.stderr)
    print(f"  Play locally: asciinema play {cast_file}", file=sys.stderr)
    print(f"  Upload: asciinema upload {cast_file}", file=sys.stderr)


def make_event(timestamp: float, data: str) -> list:
    """Create an asciinema v2 event: [timestamp, "o", data]"""
    return [round(timestamp, 6), "o", data]


if __name__ == "__main__":
    main()
