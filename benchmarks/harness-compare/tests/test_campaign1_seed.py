"""Spec section 11.2, K8: the seed registered with the proxy is HC_SEED_BASE + repeat index.

The repeat index stays the key for run ids and directories (s<r>); run.json carries both
`repeat` and `seed`. Interface assumption: the runner takes HC_SEED_BASE from the environment
through BenchConfig.from_env (the tests set both the env var and the mapping).
"""

import subprocess
from pathlib import Path

import pytest

from hcbench.arms import load_arms
from hcbench.config import BenchConfig
from hcbench.runner import execute_run
from hcbench.suites import Task

ROOT = Path(__file__).resolve().parent.parent


def ok(stdout=""):
    return subprocess.CompletedProcess([], 0, stdout, "")


def fake_exec(argv, *, input=None, timeout=None):
    return ok()


class FakeHttp:
    def __init__(self):
        self.calls = []

    def __call__(self, method, url, payload=None, timeout=30, token=""):
        self.calls.append((method, url, payload))
        return [] if method == "GET" else None

    def registration(self):
        (payload,) = [p for m, u, p in self.calls if m == "POST" and u.endswith("/admin/runs")]
        return payload


def do_run(tmp_path, monkeypatch, repeat, base=None):
    env = {"HC_BUNDLE_DIR": str(tmp_path / "bundle"), "HC_ADMIN_TOKEN": "t"}
    monkeypatch.delenv("HC_SEED_BASE", raising=False)
    if base is not None:
        env["HC_SEED_BASE"] = str(base)
        monkeypatch.setenv("HC_SEED_BASE", str(base))
    cfg = BenchConfig.from_env(env)
    arm = load_arms(ROOT / "config" / "arms.toml")["pi"]
    task = Task(suite="b", task_id="t1", image="img", workdir="/work", prompt="p")
    http = FakeHttp()
    out = tmp_path / "out"
    res = execute_run(task, arm, repeat, cfg, out, ex=fake_exec, http=http)
    return res, http.registration(), out


@pytest.mark.parametrize("repeat", [0, 1, 2])
def test_k8_registered_seed_should_be_base_plus_repeat(tmp_path, monkeypatch, repeat):
    _, reg, _ = do_run(tmp_path, monkeypatch, repeat, base=20261117)
    assert reg["seed"] == 20261117 + repeat


def test_k8_run_json_should_carry_both_repeat_and_seed(tmp_path, monkeypatch):
    res, _, _ = do_run(tmp_path, monkeypatch, 2, base=20261117)
    assert res["repeat"] == 2
    assert res["seed"] == 20261119


def test_k8_run_id_and_directory_should_still_be_keyed_by_the_repeat_index(tmp_path, monkeypatch):
    res, reg, out = do_run(tmp_path, monkeypatch, 1, base=20261117)
    assert res["run_id"] == "b.pi.s1.t1"
    assert reg["run_id"].startswith("b.pi.s1.t1")
    assert (out / "b" / "pi" / "s1" / "t1" / "run.json").exists()
    assert not (out / "b" / "pi" / "s20261118").exists()


@pytest.mark.parametrize("repeat", [0, 2])
def test_k8_seed_base_unset_should_mean_base_zero(tmp_path, monkeypatch, repeat):
    res, reg, _ = do_run(tmp_path, monkeypatch, repeat, base=None)
    assert reg["seed"] == repeat
    assert res["seed"] == repeat
    assert res["repeat"] == repeat
