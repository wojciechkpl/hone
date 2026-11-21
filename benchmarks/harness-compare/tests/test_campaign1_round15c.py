"""Spec section 11.17, round 15c, K134-K137: classify only the harness's own error block.

K134 replaces parts of K129 (and K135 parts of K130); the round 15b tests were updated to the new
reading. The cases here pin what the review of round 15b found: the classification must not read
text the agent controls (the patch in "Git diff before/after:", the project's test output) and
must read only ``error_ids``. Fake logs follow the fork's ``eval.py``.
"""
# ruff: noqa: F401

import json
import re

import pytest
from fakebin import calls
from test_campaign1_round15 import grade_rig
from test_campaign1_round15b import (
    AMB,
    INFRA,
    ForkEvaluate,
    attempt,
    fork_log,
    outcomes,
    run_attempts,
    run_fork,
    run_log,
    step,
)

from hcbench import grade

PATCH_WITH_SIGNATURE = (
    "diff --git a/f.py b/f.py\n+try:\n+    import x\n+except ModuleNotFoundError:\n+    pass\n"
    "+# Error creating container\n+# Cannot allocate memory\n"
)

# ---- K134 which instances ----


@pytest.mark.parametrize("text", [INFRA, AMB, "Cannot allocate memory", "ModuleNotFoundError: x"])
def test_k134_an_unresolved_instance_should_stay_unresolved(tmp_path, monkeypatch, text):
    s = {
        "report": {"unresolved_ids": ["a__a-1"]},
        "logs": {
            "a__a-1": {
                "run_instance.log": run_log("a__a-1", text, patch=PATCH_WITH_SIGNATURE),
                "test_output.txt": text + "\nTimeout error: 9 seconds exceeded.",
            }
        },
    }
    rc, camp, _ = run_fork(tmp_path, monkeypatch, [s])
    assert outcomes(camp)["a__a-1"] == ("0", "unresolved")
    assert rc == 0


# ---- K134 which text ----


@pytest.mark.parametrize("signature", [INFRA, AMB, "Cannot allocate memory", "ModuleNotFoundError"])
def test_k134_a_signature_in_the_patch_sections_should_not_count(tmp_path, monkeypatch, signature):
    patch = f"diff --git a/f.py b/f.py\n+# {signature}\n+except ModuleNotFoundError:\n"
    log = run_log("a__a-1", "AssertionError: tests failed", patch=patch)
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


@pytest.mark.parametrize(
    "output",
    ["Timeout error: 1800 seconds exceeded.", INFRA, "ModuleNotFoundError", "collected 0 items"],
)
def test_k134_test_output_should_never_be_read(tmp_path, monkeypatch, output):
    s = step("a__a-1", "AssertionError: tests failed", test_output="\n\n" + output)
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


def test_k134_the_fork_timeout_should_use_the_error_block_not_test_output(tmp_path, monkeypatch):
    # eval.py writes the test_output.txt line AND raises "Test timed out after N seconds.".
    s = step("a__a-1", AMB, test_output="\n\nTimeout error: 1800 seconds exceeded.")
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s])
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")


def test_k134_an_error_with_no_error_block_should_stay_error(tmp_path, monkeypatch):
    log = run_log("a__a-1", error=None) + fork_log(("info", "later: Error creating container"))
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "error")  # a signature outside the block


def test_k134_only_the_last_error_line_should_start_the_block(tmp_path, monkeypatch):
    first_bad = (
        run_log("a__a-1", INFRA)  # an earlier error block with a signature
        + fork_log(
            ("info", "retrying"),
            ("error", "Error in evaluating model for a__a-1: AssertionError: tests failed"),
        )
    )
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=first_bad)])
    assert outcomes(camp)["a__a-1"] == ("0", "error")  # not the first line to the end


def test_k134_a_signature_in_the_last_block_should_count_even_after_a_clean_first_one(
    tmp_path, monkeypatch
):
    log = run_log("a__a-1", "AssertionError: first") + run_log("a__a-1", INFRA)
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")


def test_k134_the_block_should_run_to_the_end_of_the_file(tmp_path, monkeypatch):
    log = run_log("a__a-1", "RuntimeError: x") + fork_log(
        (
            "info",
            '  File "eval.py", line 9, in run_instance\nOSError: [Errno 12] Cannot allocate memory',
        )
    )
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")


def test_k134_an_error_line_of_another_instance_should_not_start_the_block(tmp_path, monkeypatch):
    log = run_log("a__a-1") + fork_log(
        ("error", f"Error in evaluating model for other__x-9: {INFRA}")
    )
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


TS = "2026-11-20 23:27:23,136"
ERR = f"Error in evaluating model for a__a-1: {INFRA}"


STAMP = r"(?m)^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d,\d{3} - "


def test_k138_the_fixture_should_write_the_fork_format():
    text = run_log("a__a-1", INFRA)
    assert re.search(STAMP + r"ERROR - Error in evaluating model for a__a-1:", text)
    assert re.search(STAMP + r"INFO - Git diff before:", text)


def test_k138_a_timestamped_error_line_should_start_the_block(tmp_path, monkeypatch):
    log = run_log("a__a-1") + f"{TS} - ERROR - {ERR}\n"
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")


@pytest.mark.parametrize(
    "line",
    [
        ERR,  # the message alone
        f"{TS} - INFO - {ERR}",  # another level
        f"{TS} - WARNING - {ERR}",
        f" {TS} - ERROR - {ERR}",  # space-prefixed
        f"+{TS} - ERROR - {ERR}",  # a diff line imitating it
        f"-{TS} - ERROR - {ERR}",
        f"INFO {TS} - ERROR - {ERR}",
        f"2026-11-20 23:27:23 - ERROR - {ERR}",  # no milliseconds
        f"{TS} ERROR {ERR}",  # not the fork's separators
    ],
)
def test_k138_only_the_fork_prefixed_error_line_should_start_the_block(tmp_path, monkeypatch, line):
    log = run_log("a__a-1") + line + "\n"
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


def test_k138_a_diff_line_that_imitates_the_error_line_should_not_start_the_block(
    tmp_path, monkeypatch
):
    patch = f"diff --git a/f.py b/f.py\n+{TS} - ERROR - {ERR}\n {TS} - ERROR - {ERR}\n"
    log = run_log("a__a-1", "AssertionError: tests failed", patch=patch)
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


def test_k138_the_instance_id_should_be_escaped(tmp_path, monkeypatch):
    iid = "o__r.s-1"
    other = "o__rXs-1"  # matches an unescaped "."
    log = run_log(iid) + fork_log(("error", f"Error in evaluating model for {other}: {INFRA}"))
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step(iid, log=log)], (iid,))
    assert outcomes(camp)[iid] == ("0", "error")
    log = run_log(iid, INFRA)
    (tmp_path / "ok").mkdir()
    _, camp, _ = run_fork(tmp_path / "ok", monkeypatch, [step(iid, log=log)], (iid,))
    assert outcomes(camp)[iid] == ("", "grader_infra")


def test_k138_a_pull_failure_should_be_grader_infra(tmp_path, monkeypatch):
    error = (
        "Error building image a__a-1: 404 Client Error: pull access denied for "
        "swerebench/sweb.eval.x86_64.a_1776_a-1"
    )
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", error)])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")


def test_k138_a_test_timeout_should_be_ambiguous(tmp_path, monkeypatch):
    # eval.py: test_output.txt gets "Timeout error: N seconds exceeded." and the
    # EvaluationError text is "<iid>: Test timed out after N seconds."
    error = "a__a-1: Test timed out after 1800 seconds."
    s = step("a__a-1", error, test_output="\n\nTimeout error: 1800 seconds exceeded.")
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s])
    assert outcomes(camp)["a__a-1"] == ("", "ambiguous")


def test_k134_apply_failure_should_still_take_precedence(tmp_path, monkeypatch):
    # K143: the harness's own INFO line, written through the fork's Formatter.
    log = run_log("a__a-1", INFRA) + fork_log(("info", ">>>>> Patch Apply Failed"))
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "apply_failed")


def test_k134_the_lite_path_should_still_read_the_report_lists_only(tmp_path, monkeypatch):
    s = {
        "report": {"error_ids": ["a__a-1"]},
        "logs": {"a__a-1": {"run_instance.log": run_log("a__a-1", INFRA)}},
    }
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s], fork=False)
    assert outcomes(camp)["a__a-1"] == ("0", "error")


# ---- K135 and K137 ----


@pytest.mark.parametrize(
    ("kinds", "want"),
    [
        (["infra", "amb", "amb"], "ambiguous_repeated"),
        (["infra", "infra", "infra"], "grader_infra_final"),
    ],
)
def test_k137_the_review_cases_should_hold(tmp_path, monkeypatch, kinds, want):
    _, camp, ev = run_attempts(tmp_path, monkeypatch, kinds)
    assert outcomes(camp)["a__a-1"][1] == want
    assert len(ev.cmds) == 3  # and no fourth attempt


def test_k135_ambiguous_repeated_should_count_attempts_after_the_last_infra_only(
    tmp_path, monkeypatch
):
    # An earlier error between two ambiguous attempts is a plain failure, so it ends there.
    _, camp, _ = run_attempts(tmp_path, monkeypatch, ["amb", "error"])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


def test_k135_the_final_statuses_should_be_per_instance(tmp_path, monkeypatch):
    ids = ("a__a-1", "b__b-2")
    infra_both = {
        "report": {"error_ids": list(ids)},
        "logs": {i: {"run_instance.log": run_log(i, INFRA)} for i in ids},
    }
    cleared = {
        "report": {"error_ids": ["a__a-1"], "resolved_ids": ["b__b-2"]},
        "logs": {"a__a-1": {"run_instance.log": run_log("a__a-1", INFRA)}},
    }
    _, camp, _ = run_fork(tmp_path, monkeypatch, [infra_both, cleared, cleared], ids, "--regrade")
    grade.main([*grade_args_for(camp, tmp_path), "--regrade"])
    assert outcomes(camp) == {"a__a-1": ("", "grader_infra_final"), "b__b-2": ("1", "resolved")}


def grade_args_for(camp, tmp_path):
    from test_campaign1_round15 import grade_args

    return grade_args(camp, tmp_path / "prereg")


# ---- K136 fork grading needs the stage-a file ----


def test_k136_the_rig_should_write_a_stage_a_file_that_the_script_accepts(tmp_path):
    r, uv, _ = grade_rig(tmp_path)
    assert r.returncode == 0, r.stderr
    assert len(uv) == 1


def test_k136_a_missing_stage_a_file_should_stop_before_uv(tmp_path):
    # The environment holds a valid commit and namespace: there is no fallback to it.
    r, uv, d = grade_rig(tmp_path, toml=None)
    assert not (d / "preregistration.toml").exists()
    assert r.returncode != 0
    assert "preregistration.toml" in r.stdout + r.stderr
    assert "Traceback" not in r.stdout + r.stderr  # K140: its own message
    assert uv == []


@pytest.mark.parametrize("stage", ["cb", "x", ""])
def test_k136_a_file_that_is_not_stage_a_should_stop_before_uv(tmp_path, stage):
    r, uv, _ = grade_rig(tmp_path, toml=stage)
    assert r.returncode != 0
    assert "stage" in (r.stdout + r.stderr).lower()
    assert "Traceback" not in r.stdout + r.stderr
    assert uv == []


def test_k136_the_lite_path_should_not_need_the_file(tmp_path):
    r, (argv,), _ = grade_rig(tmp_path, grader="SWE-bench/other", toml=None)
    assert r.returncode == 0, r.stderr
    assert "swebench==5.0.2" in " ".join(argv)
