"""Spec section 11.17, round 15d, K138-K140 (the K138 prefix tests are in round15c).

K139: the recorded exclusions text says what "left out of pass@1" means for the contrasts.
K140: `grader_infra_final` runs are printed to stderr as WARN lines (one per run, every time
`hcbench.grade` runs); fork grading refuses a missing or wrong stage-a file with its own
message and no Python traceback.
"""
# ruff: noqa: F401

import json
import re

import pytest
from test_campaign1_round15 import grade_args, grade_rig
from test_campaign1_round15b import INFRA, attempt, run_attempts, run_fork, run_log, step

from hcbench import grade, prereg

# ---- K139 ----


def test_k139_exclusions_text_should_state_the_per_contrast_rule():
    text = " ".join(prereg.ANALYSIS_PLAN_A["exclusions"].lower().split())
    assert "no judged run" in text
    assert "every paired contrast" in text
    assert "per contrast" in text
    assert "count" in text


def test_k139_the_rule_should_name_pass_at_1_and_the_final_grader_status():
    text = " ".join(prereg.ANALYSIS_PLAN_A["exclusions"].lower().split())
    assert "grader_infra_final" in text
    assert "pass@1" in text


# ---- K140 stderr warnings ----


def warn_lines(err):
    return [ln for ln in err.splitlines() if "WARN" in ln]


def final_run(tmp_path, monkeypatch, ids=("a__a-1",)):
    both = {
        "report": {"error_ids": list(ids)},
        "logs": {i: {"run_instance.log": run_log(i, INFRA)} for i in ids},
    }
    _, camp, ev = run_fork(tmp_path, monkeypatch, [both], ids, "--regrade")
    grade.main([*grade_args(camp, tmp_path / "prereg"), "--regrade"])
    return camp, ev


def test_k140_a_final_grader_failure_should_be_printed_to_stderr_as_a_warn_line(
    tmp_path, monkeypatch, capsys
):
    final_run(tmp_path, monkeypatch)
    lines = [ln for ln in warn_lines(capsys.readouterr().err) if "grader_infra_final" in ln]
    assert len(lines) == 1
    assert "a__a-1" in lines[0]


def test_k140_every_final_run_should_get_its_own_line(tmp_path, monkeypatch, capsys):
    ids = ("a__a-1", "b__b-2", "c__c-3")
    final_run(tmp_path, monkeypatch, ids)
    lines = [ln for ln in warn_lines(capsys.readouterr().err) if "grader_infra_final" in ln]
    assert len(lines) == 3
    for i in ids:
        assert sum(i in ln for ln in lines) == 1


def test_k140_the_warning_should_come_back_on_every_later_run(tmp_path, monkeypatch, capsys):
    camp, _ = final_run(tmp_path, monkeypatch)
    capsys.readouterr()
    grade.main(grade_args(camp, tmp_path / "prereg"))
    err = capsys.readouterr().err
    assert any("a__a-1" in ln and "grader_infra_final" in ln for ln in warn_lines(err))


def test_k140_a_grader_failure_that_is_not_final_should_not_warn_as_final(
    tmp_path, monkeypatch, capsys
):
    run_attempts(tmp_path, monkeypatch, ["infra", "infra"])  # two attempts: still re-gradable
    assert "grader_infra_final" not in capsys.readouterr().err


def test_k140_runs_that_are_not_final_should_not_get_a_warn_line(tmp_path, monkeypatch, capsys):
    run_attempts(tmp_path, monkeypatch, ["amb", "amb"])
    err = capsys.readouterr().err
    assert not [ln for ln in warn_lines(err) if "ambiguous_repeated" in ln and "FINAL" in ln]
    assert "grader_infra_final" not in err


def test_k140_the_warning_should_go_to_stderr_not_stdout(tmp_path, monkeypatch, capsys):
    final_run(tmp_path, monkeypatch)
    out = capsys.readouterr()
    assert "grader_infra_final" in out.err
    assert not warn_lines(out.out)


@pytest.mark.parametrize("toml", [None, "cb"])
def test_k140_a_bad_stage_a_file_should_stop_with_its_own_message(tmp_path, toml):
    r, uv, _ = grade_rig(tmp_path, toml=toml)
    out = r.stdout + r.stderr
    assert r.returncode != 0
    assert "preregistration.toml" in out
    assert "Traceback" not in out
    assert uv == []


def test_k140_an_unreadable_stage_a_file_should_stop_with_its_own_message(tmp_path):
    _, _, d = grade_rig(tmp_path, toml="a")
    (d / "preregistration.toml").write_text("this is [not toml\n")
    from fakebin import calls, make_fake_bin, run_script

    # Run the script again on the same rig with the broken file.
    rig = d.parent.parent
    (tmp_path / "again").mkdir()
    bindir, log = make_fake_bin(tmp_path / "again")
    env = {"FAKE_UV_STUB_MODULES": "hcbench.grade"}
    r = run_script(rig, "20_grade.sh", ["camp"], bindir, env)
    out = r.stdout + r.stderr
    assert r.returncode != 0
    assert "preregistration.toml" in out
    assert "Traceback" not in out
    assert [c for c in calls(log) if c[0] == "uv"] == []
