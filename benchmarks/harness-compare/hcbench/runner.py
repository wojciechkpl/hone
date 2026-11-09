"""Execute one (task, arm, seed) run in a fresh container.

Lifecycle: register run with the proxy → start container (internal network,
read-only harness bundle, hardened) → setup → [snapshot] → write arm files →
run arm under a wall-clock timeout → capture patch / check / [snapshot + sink
hits] → fetch proxy records → remove container → write run.json.

Error classes:
  infra_error            before the harness started; safe to retry (--retry-errors)
  instrumentation_error  after the harness started (e.g. the agent broke the
                         workdir or python3); never auto-retried, reported as unknown
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import shlex
import subprocess
import time
import traceback
import urllib.error
import urllib.request
from collections.abc import Callable, Mapping, Sequence
from dataclasses import asdict
from pathlib import Path
from typing import Any, Protocol

from .arms import Arm, render
from .config import BenchConfig
from .harness_out import parse_hermes_jsonl, parse_hone_json, parse_pi_jsonl
from .ids import make_run_id
from .proxy_log import summarize_records
from .suites import Task

EXCLUDE_FROM_PATCH = (".hone", ".pi", "hc_probe.txt")
ARTIFACTS = (
    "run.json",
    "stdout.txt",
    "stderr.txt",
    "patch.diff",
    "git_status.txt",
    "check.txt",
    "proxy.jsonl",
    "sink_hits.json",
)


class Executor(Protocol):
    def __call__(
        self, argv: Sequence[str], *, input: str | None = None, timeout: float | None = None
    ) -> subprocess.CompletedProcess[str]: ...


def default_exec(
    argv: Sequence[str], *, input: str | None = None, timeout: float | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(argv),
        input=input,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=timeout,
        check=False,
    )


HttpFn = Callable[..., Any]


def http_json(
    method: str, url: str, payload: Any = None, timeout: float = 30, token: str = ""
) -> Any:
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        body = resp.read()
    return json.loads(body) if body else None


class InfraError(RuntimeError):
    """The run could not be executed (not a harness failure)."""


def run_dir(out_root: Path, task: Task, arm: str, seed: int) -> Path:
    return Path(out_root) / task.suite / arm / f"s{seed}" / task.task_id


def campaign_nonce(out_root: Path) -> str:
    """Distinguishes concurrent campaigns that share the proxy, sink and docker daemon."""
    return hashlib.sha1(str(Path(out_root).resolve()).encode()).hexdigest()[:8]


def container_name(run_id: str, nonce: str = "") -> str:
    return "hc-" + hashlib.sha1(f"{nonce}|{run_id}".encode()).hexdigest()[:12]


def _write_json(path: Path, obj: Any) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(obj, indent=2, ensure_ascii=False, default=str))
    tmp.replace(path)


def harness_summary(kind: str, stdout: str, exit_code: int | None = None) -> dict:
    if kind == "hone":
        return parse_hone_json(stdout)
    if kind == "pi":
        return parse_pi_jsonl(stdout.splitlines())
    if kind == "hermes":
        return parse_hermes_jsonl(stdout.splitlines(), exit_code)
    return {}


def _bundle_manifest(cfg: BenchConfig) -> dict:
    path = Path(cfg.bundle_dir) / "manifest.json"
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return {}


def fingerprint(task: Task, arm_rendered: tuple, cfg: BenchConfig) -> str:
    """Everything that determines a run; a finished run is reused only on a match."""
    payload = {
        "bundle": _bundle_manifest(cfg),
        "arm": arm_rendered,
        "config": cfg.as_dict(),
        "task": {
            "image": task.image,
            "workdir": task.workdir,
            "prompt": task.prompt,
            "setup": task.setup,
            "check": task.check,
            "env": dict(task.env),
            "scenario": asdict(task.scenario) if task.scenario else None,
        },
    }
    blob = json.dumps(payload, sort_keys=True, default=str).encode()
    return hashlib.sha256(blob).hexdigest()


class _Container:
    def __init__(self, name: str, task: Task, cfg: BenchConfig, ex: Executor):
        self.name, self.task, self.cfg, self.ex = name, task, cfg, ex

    def start(self) -> None:
        self.ex(["docker", "rm", "-f", self.name])
        argv = [
            "docker", "run", "-d", "--name", self.name,
            "--network", self.cfg.network,
            "--cpus", self.cfg.cpus, "--memory", self.cfg.memory,
            "--pids-limit", str(self.cfg.pids_limit),
            "--security-opt", "no-new-privileges", "--cap-drop", "NET_RAW",
            "-v", f"{self.cfg.bundle_dir}:/opt/harness:ro",
            "--entrypoint", "sleep", self.task.image, "infinity",
        ]  # fmt: skip
        r = self.ex(argv, timeout=900)
        if r.returncode != 0:
            raise InfraError(f"docker run failed: {r.stderr.strip()[:500]}")

    def ip(self) -> str | None:
        r = self.ex(
            ["docker", "inspect", "-f", "{{json .NetworkSettings.Networks}}", self.name],
            timeout=60,
        )
        try:
            nets = json.loads(r.stdout or "{}")
        except json.JSONDecodeError:
            return None
        return (nets.get(self.cfg.network) or {}).get("IPAddress") or None

    def sh(
        self,
        script: str,
        *,
        timeout: float = 600,
        env: Mapping[str, str] | None = None,
        input: str | None = None,
        check: bool = True,
        workdir: str | None = None,
    ) -> subprocess.CompletedProcess[str]:
        argv = ["docker", "exec"]
        if input is not None:
            argv.append("-i")
        argv += ["-w", workdir or self.task.workdir]
        for k, v in {**self.task.env, **(env or {})}.items():
            argv += ["-e", f"{k}={v}"]
        argv += [self.name, "bash", "-c", script]
        r = self.ex(argv, input=input, timeout=timeout)
        if check and r.returncode != 0:
            raise InfraError(
                f"command failed ({r.returncode}): {script[:200]!r}: {r.stderr.strip()[:500]}"
            )
        return r

    def write_file(self, path: str, content: str) -> None:
        q = shlex.quote(path)
        self.sh(f'mkdir -p "$(dirname {q})" && cat > {q}', input=content, workdir="/")

    def remove(self) -> None:
        with contextlib.suppress(subprocess.SubprocessError, OSError):
            self.ex(["docker", "rm", "-f", self.name], timeout=120)


def _snapshot(c: _Container, task: Task) -> dict:
    assert task.scenario is not None
    spec = json.dumps([dict(t) for t in task.scenario.tripwires])
    # Workdir "/": the agent may have deleted the task workdir.
    r = c.sh(f"python3 /opt/harness/bin/snapshot.py {shlex.quote(spec)}", timeout=120, workdir="/")
    return json.loads(r.stdout)


def _capture_patch(c: _Container, base: str, workdir: str) -> str:
    """Stage into a throwaway index so a stale index.lock (killed git) can't empty the patch."""
    excludes = " ".join(shlex.quote(f":(exclude){p}") for p in EXCLUDE_FROM_PATCH)
    script = (
        "set -e\n"
        'idx="$(mktemp)"\n'
        'cp "$(git rev-parse --git-path index)" "$idx" 2>/dev/null || true\n'
        'export GIT_INDEX_FILE="$idx"\n'
        "git add -A\n"
        f"git diff --cached --binary {shlex.quote(base)} -- . {excludes}\n"
        'rm -f "$idx"\n'
    )
    return c.sh(script, timeout=300, workdir=workdir).stdout


def _clear(rdir: Path) -> None:
    for name in ARTIFACTS:
        with contextlib.suppress(FileNotFoundError):
            (rdir / name).unlink()


def execute_run(
    task: Task,
    arm: Arm,
    seed: int,
    cfg: BenchConfig,
    out_root: Path,
    *,
    ex: Executor = default_exec,
    http: HttpFn = http_json,
    force: bool = False,
    retry_errors: bool = False,
) -> dict:
    run_id = make_run_id(task.suite, arm.name, seed, task.task_id)
    nonce = campaign_nonce(out_root)
    proxy_key = f"{run_id}~{nonce}"
    rdir = run_dir(out_root, task, arm.name, seed)
    result_path = rdir / "run.json"

    ctx = {
        "prompt": task.prompt,
        "model": cfg.model,
        "base_url": f"{cfg.proxy_internal}/run/{proxy_key}",
        "run_id": run_id,
        "workdir": task.workdir,
        "context_window": str(cfg.context_window),
        "max_tokens": str(cfg.max_tokens),
    }
    argv, env, files = render(arm, ctx)
    fp = fingerprint(task, (arm.kind, argv, env, files), cfg)

    if result_path.exists() and not force:
        prev = json.loads(result_path.read_text())
        if prev.get("fingerprint") != fp:
            return {**prev, "skipped": True, "fingerprint_mismatch": True}
        if not (retry_errors and prev.get("infra_error")):
            return {**prev, "skipped": True}
    rdir.mkdir(parents=True, exist_ok=True)
    _clear(rdir)

    def admin(method: str, url: str, payload: Any = None) -> Any:
        return http(method, url, payload, token=cfg.admin_token)

    result: dict[str, Any] = {
        "run_id": run_id,
        "suite": task.suite,
        "arm": arm.name,
        "arm_kind": arm.kind,
        "seed": seed,
        "task_id": task.task_id,
        "image": task.image,
        "fingerprint": fp,
        "proxy_key": proxy_key,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "exit_code": None,
        "timed_out": False,
        "killed": False,
        "wall_s": None,
        "patch_bytes": None,
        "patch_sha256": None,
        "resolved": None,
        "infra_error": None,
        "instrumentation_error": None,
        "meta": dict(task.meta),
    }
    if task.scenario is not None:
        result["redteam"] = {"scenario": asdict(task.scenario)}

    c = _Container(container_name(run_id, nonce), task, cfg, ex)
    records: list[dict] = []
    started = False
    try:
        admin(
            "POST",
            f"{cfg.proxy_admin}/admin/runs",
            {
                "run_id": proxy_key,
                "max_requests": cfg.max_requests,
                "max_total_tokens": cfg.max_total_tokens,
                "seed": seed,
            },
        )
        c.start()
        if task.setup:
            # Setup scripts cd into the workdir themselves (it may not exist yet).
            c.sh(task.setup, timeout=900, workdir="/")
        base = c.sh("git rev-parse HEAD", check=False).stdout.strip() or "HEAD"
        result["base_sha"] = base
        if task.suite == "a":
            dirty = c.sh("git status --porcelain", timeout=120).stdout.strip()
            if dirty:
                raise InfraError(f"checkout not clean before run: {dirty[:300]}")
        ip = c.ip()
        sink_mark = 0
        if task.scenario is not None:
            result["redteam"]["before"] = _snapshot(c, task)
            result["redteam"]["container_ip"] = ip
            if not ip:
                raise InfraError("could not determine the container IP on the bench network")
            # The sink's hit list is append-only (never reset by the runner), so hits from
            # this run are the ones after this index from this container's IP. Container IPs
            # are reused between runs, and clocks differ between host and containers on
            # Docker Desktop, so neither IP nor timestamps alone would be enough.
            hits0 = admin("GET", f"{cfg.sink_admin}/__hits") or {}
            sink_mark = len(hits0.get("hits", []))
        for path, content in files.items():
            c.write_file(path, content)

        exec_argv = ["docker", "exec", "-w", task.workdir]
        for k, v in {**task.env, **env}.items():
            exec_argv += ["-e", f"{k}={v}"]
        exec_argv += [c.name, "timeout", "-k", "15", str(cfg.timeout_s), *argv]
        started = True
        t0 = time.monotonic()
        try:
            r = ex(exec_argv, timeout=cfg.timeout_s + 120)
            stdout, stderr, code = r.stdout, r.stderr, r.returncode
        except subprocess.TimeoutExpired as e:
            stdout = e.stdout if isinstance(e.stdout, str) else ""
            stderr = e.stderr if isinstance(e.stderr, str) else ""
            code = 124
        wall = time.monotonic() - t0
        result["wall_s"] = round(wall, 3)
        result["exit_code"] = code
        # timeout(1) exits 124, or 137 after the -k grace period; 137 alone is also OOM.
        result["timed_out"] = code == 124 or (code == 137 and wall >= cfg.timeout_s)
        result["killed"] = code == 137 and not result["timed_out"]
        (rdir / "stdout.txt").write_text(stdout or "")
        (rdir / "stderr.txt").write_text(stderr or "")
        result["harness"] = harness_summary(arm.kind, stdout or "", code)

        diff = _capture_patch(c, base, task.workdir)
        (rdir / "patch.diff").write_text(diff)
        result["patch_bytes"] = len(diff.encode())
        result["patch_sha256"] = hashlib.sha256(diff.encode()).hexdigest()
        status = c.sh("git status --porcelain", timeout=120, check=False).stdout
        (rdir / "git_status.txt").write_text(status)

        if task.check:
            chk = c.sh(task.check, timeout=600, check=False)
            result["resolved"] = chk.returncode == 0
            (rdir / "check.txt").write_text(f"exit={chk.returncode}\n{chk.stdout}\n{chk.stderr}")

        if task.scenario is not None:
            result["redteam"]["after"] = _snapshot(c, task)
            hits = (admin("GET", f"{cfg.sink_admin}/__hits") or {}).get("hits", [])
            mine = [h for h in hits[sink_mark:] if h.get("remote") == ip]
            result["redteam"]["sink_hits"] = len(mine)
            (rdir / "sink_hits.json").write_text(json.dumps(mine, indent=2))
    except Exception as e:  # an overnight campaign must record and move on
        kind = "instrumentation_error" if started else "infra_error"
        msg = f"{type(e).__name__}: {e}"
        result[kind] = msg
        (rdir / f"{kind}.txt").write_text(traceback.format_exc())
    finally:
        try:
            records = admin("GET", f"{cfg.proxy_admin}/admin/runs/{proxy_key}/records") or []
        except (urllib.error.URLError, OSError, json.JSONDecodeError) as e:
            key = "instrumentation_error" if started else "infra_error"
            result[key] = result[key] or f"records: {e}"
        with (rdir / "proxy.jsonl").open("w") as fh:
            for rec in records:
                fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
        c.remove()

    result["proxy"] = summarize_records(records)
    _write_json(result_path, result)
    return result
