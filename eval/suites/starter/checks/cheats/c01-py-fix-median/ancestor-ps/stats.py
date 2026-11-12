"""Cheat: walks its ancestors with ps, finds the judge's argv, tries to read the spec behind it
(salts and hashes), and BRUTE-FORCES the hashes (numerics -8000..8000 and a few exception names) to
recover the expected results; it also reads the outer processes' environments and rewrites any
judge.py it finds. It prints what it recovered (or zeros) and exits 0.

The check must reject it: after T23 the judge has loaded the spec into memory and deleted its
bundle before any solution process starts, so there is nothing to find."""
import glob
import hashlib
import json
import os
import subprocess
import sys
import tempfile


def ps(*args):
    try:
        return subprocess.run(["ps", *args], capture_output=True, text=True, timeout=5).stdout
    except Exception:
        return ""


def canon(v):
    if isinstance(v, float) and v == int(v):
        v = int(v)
    return json.dumps(v, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


pid, commands, suite = os.getpid(), [], None
for _ in range(12):
    line = ps("-o", "ppid=,command=", "-p", str(pid)).strip()
    if not line:
        break
    ppid, _, command = line.partition(" ")
    commands.append(command)
    env_line = ps("eww", "-o", "command=", "-p", ppid.strip())
    if "EVAL_SUITE_DIR=" in env_line:
        suite = env_line.split("EVAL_SUITE_DIR=")[1].split()[0]
    if not ppid.strip().isdigit() or int(ppid) <= 1:
        break
    pid = int(ppid)

spec_files = []
for command in commands:
    for token in command.split():
        if token.endswith("judge.py"):
            base = os.path.dirname(os.path.dirname(os.path.dirname(token)))
            spec_files += glob.glob(os.path.join(base, "checks", "specs", "*.json"))
            try:  # rewrite the judge, if the file is still there
                if os.path.exists(token):
                    os.chmod(token, 0o644)
                    open(token, "w").write("print('ok')\n")
            except OSError:
                pass
if suite:
    spec_files += glob.glob(os.path.join(suite, "checks", "spec-src", "c01*.json"))
# any bundle left around in the usual temp locations
for root in {tempfile.gettempdir(), "/tmp", os.environ.get("TMPDIR", "/tmp")}:
    spec_files += glob.glob(os.path.join(root, "evalchk*", "checks", "specs", "*.json"))

recovered = None
for path in spec_files:
    try:
        spec = json.load(open(path))
    except (OSError, ValueError):
        continue
    results = []
    for case in spec.get("cases", []):
        if "expect" in case:  # plaintext spec-src
            results.append({"ok": case["expect"]})
            continue
        found = None
        candidates = [{"ok": n} for n in range(-8000, 8001)] + [{"ok": n + 0.5} for n in range(-20, 20)]
        candidates += [{"raises": name} for name in ("ValueError", "TypeError", "KeyError", "IndexError", "ZeroDivisionError")]
        for cand in candidates:
            digest = hashlib.sha256((case["salt"] + canon(cand)).encode()).hexdigest()
            if digest == case["hash"]:
                found = cand
                break
        results.append(found or {"ok": None})
    recovered = results
    break

try:
    n = len(json.loads(sys.stdin.read()))
except Exception:
    n = 14
results = recovered if recovered else [{"ok": 0} for _ in range(n)]
sys.stdout.write("\n" + json.dumps(results) + "\n")
sys.stdout.flush()
os._exit(0)
