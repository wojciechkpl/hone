"""Spec section 13 (Hermes smoke probes P-H1..P-H7) in suites/smoke.toml, via the suite loader."""

import json
import os
import re
import subprocess
from pathlib import Path

import pytest

from hcbench.config import BenchConfig
from hcbench.gate import smoke_gate
from hcbench.suites import load_task_suite

ROOT = Path(__file__).resolve().parent.parent
SMOKE = ROOT / "suites" / "smoke.toml"


@pytest.fixture(scope="module")
def tasks():
    return {t.task_id: t for t in load_task_suite(SMOKE, "smoke", BenchConfig.from_env({}))}


def probe(tasks, n):
    """Probe ids start with `p-h<n>` (e.g. `p-h1-oneshot-exit`); exactly one per number."""
    found = [t for tid, t in tasks.items() if re.match(rf"^p-h{n}(-|$)", tid)]
    assert len(found) == 1, f"expected exactly one task for probe P-H{n}, got {len(found)}"
    return found[0]


@pytest.mark.parametrize("n", range(1, 8))
def test_smoke_suite_should_declare_probe_p_h(tasks, n):
    t = probe(tasks, n)
    assert t.prompt.strip()
    assert t.suite == "smoke"


@pytest.mark.parametrize("n", [1, 2, 3, 5, 7])
def test_deterministic_probes_should_pin_an_expected_outcome_for_hermes(tasks, n):
    # P-H4 (approvals without --yolo) and P-H6 (effective window) are observations whose
    # outcome selects a pre-registered fallback, so they carry no pinned expectation.
    t = probe(tasks, n)
    assert t.expect.get("hermes") in {"pass", "fail"}
    assert t.check.strip(), "a deterministic probe needs a check command"


@pytest.mark.parametrize("n", [4, 6])
def test_observation_probes_should_not_pin_any_hermes_outcome(tasks, n):
    # `hermes = ...` in an expectation also pins `hermes-guarded` through the arm-kind fallback
    # in gate.smoke_gate, so observation probes (P-H4 approvals headless, P-H6 effective window)
    # carry no key starting with "hermes": their outcome selects a pre-registered fallback.
    assert not [k for k in probe(tasks, n).expect if k.startswith("hermes")]


def test_p_h7_offline_probe_should_exist_alongside_existing_probes(tasks):
    assert {"edit-basic", "shell-basic", "toolchain-probe", "landlock-canary"} <= set(tasks)


@pytest.mark.parametrize("tid", ["edit-basic", "shell-basic", "toolchain-probe"])
def test_existing_capability_probes_should_expect_hermes_to_pass(tasks, tid):
    assert tasks[tid].expect.get("hermes") == "pass"


def test_landlock_canary_should_expect_default_hermes_to_leak(tasks):
    # Hermes's local backend has no sandbox: like Pi, the permissive arm leaks.
    assert tasks["landlock-canary"].expect.get("hermes") == "fail"


# ---- gate: P-H2 (proxy must see Hermes requests) -----------------------


def _write_run(campaign: Path, arm, kind, requests):
    d = campaign / "smoke" / arm / "s0" / "edit-basic"
    d.mkdir(parents=True)
    run = {
        "run_id": f"smoke.{arm}.s0.edit-basic",
        "suite": "smoke",
        "task_id": "edit-basic",
        "arm": arm,
        "arm_kind": kind,
        "resolved": True,
        "infra_error": None,
        "proxy": {"requests": requests},
    }
    (d / "run.json").write_text(json.dumps(run))


def test_smoke_gate_when_hermes_run_never_reached_proxy_should_fail(tmp_path):
    _write_run(tmp_path, "hermes", "hermes", 0)
    assert smoke_gate(tmp_path, SMOKE) == 1


# ---- round 2 ----------------------------------------------------------------


def _load(window: int):
    cfg = BenchConfig.from_env({"HC_MAX_MODEL_LEN": str(window)})
    return {t.task_id: t for t in load_task_suite(SMOKE, "smoke", cfg)}


def _raw():
    import tomllib

    return {t["id"]: t for t in tomllib.loads(SMOKE.read_text())["task"]}


@pytest.mark.parametrize("tid", ["p-h1-oneshot-exit", "p-h7-offline-startup"])
def test_p_h1_and_p_h7_should_require_exit_and_result_event_for_hermes_kind_only(tid):
    req = _raw()[tid]["require"]
    assert isinstance(req, dict), "require is a table keyed by arm name or kind, like expect"
    assert set(req["hermes"]) == {"exited", "result_event"}
    assert set(req) <= {"hermes", "hermes-guarded"}  # never constrains pi / hone / selftest


def test_p_h7_should_bound_wall_time_for_hermes_only():
    bound = _raw()["p-h7-offline-startup"]["max_wall_s"]
    assert isinstance(bound, dict)
    assert set(bound) <= {"hermes", "hermes-guarded"}
    assert 0 < bound["hermes"] <= 600


def test_p_h1_should_not_declare_a_wall_bound_table_for_other_kinds():
    assert set(_raw()["p-h1-oneshot-exit"].get("max_wall_s", {"hermes": 1})) <= {
        "hermes",
        "hermes-guarded",
    }


def test_p_h6_prompt_should_require_reading_the_whole_file_and_not_allow_skipping_it(tasks):
    prompt = probe(tasks, 6).prompt.lower()
    assert re.search(r"\bread\b", prompt)
    assert re.search(r"\b(entire|whole|all of|every)\b", prompt)
    skips = ("without reading", "without loading", "do not read", "don't read", "count its")
    for skip in skips:
        assert skip not in prompt, f"prompt lets the agent skip the read: {skip!r}"
    assert not re.search(r"\bwc\b", prompt)


@pytest.mark.parametrize("window", [32768, 65536, 131072])
def test_p_h6_input_should_exceed_half_the_served_context_window(window, tmp_path):
    # The loader substitutes `{{context_window}}` (from HC_MAX_MODEL_LEN) in task text; the
    # task's setup must then create more than 0.5 * window tokens of input (bytes / 4 is a
    # conservative token estimate; real tokens per byte are higher for numeric text).
    t = probe(_load(window), 6)
    assert "{{" not in t.setup + t.prompt + t.check
    workdir = tmp_path / "wd"
    script = t.setup.replace(t.workdir, str(workdir))
    env = {
        **os.environ,
        "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
        "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t",
    }  # fmt: skip
    subprocess.run(["bash", "-c", script], check=True, capture_output=True, env=env)
    files = [f for f in workdir.rglob("*") if f.is_file() and ".git" not in f.parts]
    size = sum(f.stat().st_size for f in files)
    assert size / 4 > 0.5 * window


def test_p_h6_should_scale_with_the_configured_window():
    small, big = probe(_load(32768), 6), probe(_load(131072), 6)
    assert small.setup != big.setup or small.prompt != big.prompt


def test_toolchain_probe_should_expect_the_task_image_env_not_the_harness_bundle(tasks):
    t = tasks["toolchain-probe"]
    assert "sys.prefix" in t.prompt
    assert "/opt/miniconda3" in t.check
    assert re.search(r"!\s*grep[^;&|\n]*/opt/harness", t.check), "must assert no /opt/harness"
