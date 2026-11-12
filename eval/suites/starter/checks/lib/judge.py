"""The parent of every coding check (T13/T14 of docs/plans/eval-starter-suite-spec.md).

usage: python3 -I -B judge.py <task-id>     (the check runs it in the agent's copy; the runner
sets EVAL_SUITE_DIR). This process runs NO agent code. It

  1. copies what it needs (harness scripts, pristine tests and project files) into a private
     temp directory, then moves its own cwd out of the agent's copy;
  2. builds a work directory = pristine files + only the allowlisted solution files of the copy;
  3. runs agent code only in child processes (own session, minimal environment, no suite path in
     argv/env/cwd) that read the INPUTS of the cases on stdin and print one JSON result per case;
  4. requires a result for every case and compares them with the expectations in the spec, which
     this process alone holds;
  5. runs the project's own tests as advisory checks: a failing test fails the check, a passing
     one proves nothing.

The verdict is this process's exit code.
"""
import fnmatch
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile

SUITE = os.environ.pop("EVAL_SUITE_DIR")
COPY = os.getcwd()
TASK = sys.argv[1]
SPEC = json.load(open(os.path.join(SUITE, "checks", "specs", TASK + ".json"), encoding="utf-8"))
REAL_HOME = os.path.expanduser("~")

PRIV = tempfile.mkdtemp(prefix="evalchk-")
ASSETS = os.path.join(PRIV, "assets")
WORK = os.path.join(PRIV, "work")


def make_readonly(path):
    for root, dirs, files in os.walk(path):
        for f in files:
            os.chmod(os.path.join(root, f), 0o444)
        for d in dirs:
            os.chmod(os.path.join(root, d), 0o555)
    os.chmod(path, 0o555)


def cleanup():
    def onerror(func, p, _):
        try:
            os.chmod(os.path.dirname(p), 0o755)
            os.chmod(p, 0o755)
            func(p)
        except OSError:
            pass
    for root, dirs, files in os.walk(PRIV):
        try:
            os.chmod(root, 0o755)
        except OSError:
            pass
    shutil.rmtree(PRIV, onerror=onerror)


def fail(message):
    print(message)
    cleanup()
    sys.exit(1)


# 1. private assets, then leave the agent's copy
os.makedirs(ASSETS)
for name in SPEC.get("assets", []):
    shutil.copy(os.path.join(SUITE, "checks", "lib", name), os.path.join(ASSETS, name))
make_readonly(ASSETS)
os.chdir(PRIV)

# 2. the work directory
os.makedirs(WORK)
base = SPEC.get("base")
if base:
    shutil.copytree(os.path.join(SUITE, base), WORK, dirs_exist_ok=True)
for item in SPEC.get("take", []):
    if "file" in item:
        sources = [item["file"]]
    else:
        sources = []
        root = os.path.join(COPY, item["tree"])
        if os.path.isdir(root) and not os.path.islink(root):
            for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
                dirnames[:] = [d for d in dirnames if not os.path.islink(os.path.join(dirpath, d))]
                for f in filenames:
                    if fnmatch.fnmatch(f, item["glob"]):
                        rel = os.path.relpath(os.path.join(dirpath, f), COPY)
                        if not any(rel.startswith(x) for x in item.get("skip", [])):
                            sources.append(rel)
    for rel in sources:
        src = os.path.join(COPY, rel)
        if os.path.isfile(src) and not os.path.islink(src):
            dst = os.path.join(WORK, rel)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copy(src, dst)
for rel, asset in SPEC.get("add", {}).items():
    dst = os.path.join(WORK, rel)
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    shutil.copy(os.path.join(ASSETS, asset), dst)


def child_env():
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "HOME": os.path.join(PRIV, "home"),
        "TMPDIR": os.path.join(PRIV, "tmp"),
        "LANG": "C.UTF-8",
        "PYTHONDONTWRITEBYTECODE": "1",
        "CARGO_TARGET_DIR": os.path.join(PRIV, "target"),
        "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", os.path.join(REAL_HOME, ".rustup")),
        "CARGO_HOME": os.environ.get("CARGO_HOME", os.path.join(REAL_HOME, ".cargo")),
    }
    for d in ("home", "tmp"):
        os.makedirs(os.path.join(PRIV, d), exist_ok=True)
    return env


def run_child(argv, stdin_text=None, timeout=25):
    """Runs agent code in its own session; the whole group is killed afterwards."""
    proc = subprocess.Popen(
        argv, cwd=WORK, env=child_env(), stdin=subprocess.PIPE if stdin_text is not None else subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, start_new_session=True, text=True, errors="replace",
    )
    try:
        out, err = proc.communicate(stdin_text, timeout=timeout)
    except subprocess.TimeoutExpired:
        out, err = "", "timed out"
        proc.kill()
        proc.communicate()
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except OSError:
        pass
    return proc.returncode, out, err


def last_json_line(out):
    for line in reversed(out.splitlines()):
        line = line.strip()
        if line.startswith("["):
            try:
                return json.loads(line)
            except ValueError:
                return None
    return None


runtime = SPEC["runtime"]
cases = SPEC["cases"]
problems = []

# 5 (first, cheap): advisory project tests
advisory = SPEC.get("advisory")
if advisory:
    if advisory == "python":
        argv = [sys.executable, "-I", "-B", "-X", "pycache_prefix=" + os.path.join(PRIV, "pyc"),
                "-m", "unittest", "discover", "-s", ".", "-t", ".", "-p", "test_*.py", "-q"]
    elif advisory == "node":
        argv = ["node", "--test"]
    else:
        argv = ["cargo", "test", "--offline", "-q"]
    code, out, err = run_child(argv, timeout=100 if advisory == "rust" else 25)
    if code != 0:
        fail("the project's own tests fail:\n" + (out + err)[-1500:])

# 3. the probe
inputs = []
for c in cases:
    inputs.append({k: v for k, v in c.items() if k not in ("expect", "raises", "contains", "expect_args_after", "expect_same")})
results = None
if runtime == "python":
    code, out, err = run_child(
        [sys.executable, "-I", "-B", "-X", "pycache_prefix=" + os.path.join(PRIV, "pyc"),
         os.path.join(ASSETS, "probe_child.py"), WORK, SPEC.get("module", "")],
        json.dumps(inputs))
    results = last_json_line(out)
elif runtime == "node":
    code, out, err = run_child(["node", os.path.join(ASSETS, "probe_child.js"), WORK, SPEC.get("module", "")], json.dumps(inputs))
    results = last_json_line(out)
elif runtime in ("rust-kv", "rust-roman"):
    code, out, err = run_child(["cargo", "build", "--offline", "-q", "--bin", "probe"], timeout=100)
    if code != 0:
        fail("the solution does not build as a library:\n" + (out + err)[-1500:])
    if runtime == "rust-kv":
        stdin_text = "".join(c["args"][0].encode().hex() + "\n" for c in cases)
    else:
        stdin_text = "".join(str(c["args"][0]) + "\n" for c in cases)
    code, out, err = run_child(["cargo", "run", "--offline", "-q", "--bin", "probe"], stdin_text)
    lines = [l for l in out.splitlines() if l == "N" or l.startswith("S\t")]
    if len(lines) >= len(cases):
        results = []
        for l in lines[-len(cases):]:
            if l == "N":
                results.append({"ok": None})
            elif runtime == "rust-kv":
                _, k, v = l.split("\t")
                results.append({"ok": [bytes.fromhex(k).decode(), bytes.fromhex(v).decode()]})
            else:
                results.append({"ok": bytes.fromhex(l.split("\t")[1]).decode()})
else:
    fail(f"unknown runtime {runtime}")

if not isinstance(results, list) or len(results) != len(cases):
    fail("the solution did not produce a result for every case "
         f"(exit {code}); it must be loaded and called normally\n{(out + err)[-600:]}")

for case, got in zip(cases, results):
    label = case.get("scenario") or f"{case.get('fn')}({json.dumps(case.get('args', []))[1:-1]})"
    if case.get("patch"):
        label += f"  [with {case['patch']['attr']} replaced]"
    if "raises" in case:
        ok = isinstance(got, dict) and got.get("raises") == case["raises"]
        want = f"raises {case['raises']}"
    elif "contains" in case:
        ok = isinstance(got, dict) and isinstance(got.get("ok"), str) and case["contains"] in got["ok"]
        want = f"a string containing {case['contains']!r}"
    else:
        ok = isinstance(got, dict) and "ok" in got and got["ok"] == case["expect"]
        want = json.dumps(case.get("expect"))
    if ok and "expect_args_after" in case:
        ok = isinstance(got, dict) and got.get("args_after") == case["expect_args_after"]
        want = f"arguments afterwards {json.dumps(case['expect_args_after'])}"
    if ok and "expect_same" in case:
        ok = isinstance(got, dict) and got.get("same0") == case["expect_same"]
        want = f"result identical to the argument: {case['expect_same']}"
    if not ok:
        problems.append(f"{label}: expected {want}, got {json.dumps(got)[:200]}")

# the context tasks: only the target file may differ from the original project
manifest = SPEC.get("manifest")
if manifest:
    m = json.load(open(os.path.join(SUITE, "checks", "data", manifest), encoding="utf-8"))
    targets = set(m["target"])

    def sha(path):
        import hashlib
        with open(path, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()

    for rel, digest in m["files"].items():
        path = os.path.join(COPY, rel)
        if not os.path.isfile(path):
            problems.append(f"deleted: {rel}")
        elif rel in targets:
            if sha(path) == digest:
                problems.append(f"the file that needs the change was not modified: {rel}")
        elif sha(path) != digest:
            problems.append(f"changed a file that should stay as it was: {rel}")

if problems:
    fail("the check failed:\n  " + "\n  ".join(problems))
cleanup()
print("ok")
