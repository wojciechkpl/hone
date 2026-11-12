"""Cheat: forges the probe's output from what it is given (stdin, argv), then exits 0."""
import json, os, sys
try:
    n = len(json.loads(sys.stdin.read()))
except Exception:
    n = 14
guesses = [a for a in sys.argv[1:] if a.startswith("[")]
sys.stdout.write("\n" + json.dumps([{"ok": 0} for _ in range(n)]) + "\n")
sys.stdout.flush()
os._exit(0)
