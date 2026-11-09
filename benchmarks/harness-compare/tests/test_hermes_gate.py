"""Spec section 12 (round 2): smoke tasks may `require` run-level conditions.

Both keys are tables keyed like `expect` (arm name first, then arm kind):
`require = { hermes = ["exited", "result_event"] }`: the run did not time out, and the harness
summary saw a completion event (`run["harness"]["completed"]`). `max_wall_s = { hermes = 300 }`:
`run["wall_s"]` <= N. Runs of arms with no entry (pi, hone, selftest) are not constrained.
A hermes run whose check file was written but which hit the timeout must fail such probes.
"""

import json
from pathlib import Path

import pytest

from hcbench.gate import smoke_gate

ROOT = Path(__file__).resolve().parent.parent
SUITE = """
[[task]]
id = "probe"
image = "mini"
prompt = "x"
check = "true"
expect = { hermes = "pass", pi = "pass", hone = "pass" }
require = { hermes = ["exited", "result_event"], hermes-guarded = [] }
max_wall_s = { hermes = 120 }
"""


def _write_run(campaign: Path, arm="hermes", kind="hermes", **over):
    d = campaign / "smoke" / arm / "s0" / "probe"
    d.mkdir(parents=True)
    run = {
        "run_id": f"smoke.{arm}.s0.probe",
        "suite": "smoke",
        "task_id": "probe",
        "arm": arm,
        "arm_kind": kind,
        "resolved": True,
        "infra_error": None,
        "proxy": {"requests": 5},
        "timed_out": False,
        "wall_s": 30.0,
        "harness": {"completed": True, "success": True},
    }
    run.update(over)
    (d / "run.json").write_text(json.dumps(run))


def _gate(tmp_path, arm="hermes", kind="hermes", **over):
    suite = tmp_path / "suite.toml"
    suite.write_text(SUITE)
    camp = tmp_path / "camp"
    camp.mkdir()
    _write_run(camp, arm, kind, **over)
    return smoke_gate(camp, suite)


def test_smoke_gate_when_run_exited_with_result_event_within_bound_should_pass(tmp_path):
    assert _gate(tmp_path) == 0


def test_smoke_gate_when_probe_check_passed_but_run_timed_out_should_fail(tmp_path):
    assert _gate(tmp_path, timed_out=True) == 1


def test_smoke_gate_when_no_result_event_should_fail(tmp_path):
    assert _gate(tmp_path, harness={"completed": False, "success": False}) == 1


def test_smoke_gate_when_harness_summary_missing_should_fail_result_event(tmp_path):
    assert _gate(tmp_path, harness=None) == 1


@pytest.mark.parametrize("wall", [120.5, 900.0])
def test_smoke_gate_when_wall_time_exceeds_bound_should_fail(tmp_path, wall):
    assert _gate(tmp_path, wall_s=wall) == 1


def test_smoke_gate_when_wall_time_missing_and_bound_set_should_fail(tmp_path):
    assert _gate(tmp_path, wall_s=None) == 1


NO_RESULT = {"timed_out": True, "wall_s": 900.0, "harness": {"success": True}}


@pytest.mark.parametrize(("arm", "kind"), [("pi", "pi"), ("hone-inject", "hone")])
def test_smoke_gate_when_non_hermes_run_has_no_result_event_should_still_pass(tmp_path, arm, kind):
    assert _gate(tmp_path, arm, kind, **NO_RESULT) == 0


def test_smoke_gate_when_hermes_guarded_arm_name_entry_overrides_kind_entry_should_pass(tmp_path):
    # arm name wins over arm kind, even when the arm-name entry is an empty list
    assert _gate(tmp_path, "hermes-guarded", "hermes", **NO_RESULT) == 0


def test_smoke_gate_when_hermes_run_has_no_result_event_should_still_fail_per_kind(tmp_path):
    assert _gate(tmp_path, "hermes", "hermes", **NO_RESULT) == 1
