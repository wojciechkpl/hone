"""Helpers for running scripts/*.sh offline: fake docker/curl/uv on PATH that record argv.

Not a test module (no ``test_`` prefix). The scripts run from a private copy of the rig, so
they cannot write into the real working tree.
"""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

_RECORDER = """#!{python}
import json, sys
with open({log!r}, "a") as fh:
    fh.write(json.dumps([{name!r}] + sys.argv[1:]) + "\\n")
"""

_UV = """#!{python}
import json, os, subprocess, sys
args = sys.argv[1:]
with open({log!r}, "a") as fh:
    fh.write(json.dumps(["uv"] + args) + "\\n")
i = args.index("-m")
env = dict(os.environ, PYTHONPATH=os.getcwd())
sys.exit(subprocess.run([{python!r}, "-m", *args[i + 1:]], env=env).returncode)
"""


def _write_exec(path: Path, text: str) -> None:
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


def make_rig(tmp_path: Path) -> Path:
    """Copy the rig to a scratch dir (two levels deep, so REPO_ROOT resolves)."""
    dest = tmp_path / "a" / "b" / "hc"
    shutil.copytree(
        ROOT,
        dest,
        ignore=shutil.ignore_patterns(
            ".bundle", "results", "__pycache__", ".claude", "prereg", ".pytest_cache", ".venv"
        ),
    )
    return dest


def make_fake_bin(tmp_path: Path) -> tuple[Path, Path]:
    """Fake docker, curl and uv. Returns (bin dir, call log: one JSON argv per line)."""
    bindir = tmp_path / "fakebin"
    bindir.mkdir()
    log = tmp_path / "calls.jsonl"
    for name in ("docker", "curl"):
        _write_exec(bindir / name, _RECORDER.format(python=sys.executable, log=str(log), name=name))
    _write_exec(bindir / "uv", _UV.format(python=sys.executable, log=str(log)))
    return bindir, log


def run_script(rig: Path, script: str, args: list[str], bindir: Path, env: dict[str, str]):
    full_env = {
        "PATH": f"{bindir}:/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin",
        "HOME": str(rig.parent),
        "HC_ENV_FILE": str(rig / "config" / "bench.env.example"),
        **env,
    }
    return subprocess.run(
        ["bash", str(rig / "scripts" / script), *args],
        capture_output=True,
        text=True,
        env=full_env,
        cwd=rig,
        check=False,
        timeout=120,
    )


def calls(log: Path) -> list[list[str]]:
    if not log.exists():
        return []
    return [json.loads(ln) for ln in log.read_text().splitlines() if ln.strip()]


def docker_calls(log: Path) -> list[list[str]]:
    return [c[1:] for c in calls(log) if c[0] == "docker"]


def real_env(**extra: str) -> dict[str, str]:
    return {k: v for k, v in os.environ.items() if k.startswith("HC_")} | extra
