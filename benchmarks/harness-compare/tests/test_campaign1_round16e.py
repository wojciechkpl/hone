"""Spec section 11.17, round 16e, K163-K164: the last loose ends of round 16d.

K163: under a stage-a pre-registration with `[rules_check]` (K148) the report writes no
"Suite A has N tasks; the pre-registered size is 40." caveat; the plain report keeps it.
K164: the K158 "N runs outside the pre-registration are left out." count covers suite A runs.
"""

import pytest
from test_campaign1_report import b_run, write_run
from test_campaign1_round16 import ARMS, TASKS, a_campaign, build, prereg_a

CAVEAT = "the pre-registered size is 40."


def caveat_lines(text):
    return [ln for ln in text.splitlines() if CAVEAT in ln]


# ---- K163 no Lite size caveat under stage a ----


def test_k163_a_stage_a_report_with_fewer_than_forty_tasks_should_have_no_size_caveat(tmp_path):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))  # 10 tasks
    assert caveat_lines(text) == []
    assert "Suite A has" not in text


def test_k163_a_plain_report_should_keep_the_size_caveat(tmp_path):
    # Regression guard (passes already): no pre-registration at all.
    text, _ = build(a_campaign(tmp_path), None)
    assert caveat_lines(text) == ["- Suite A has 10 tasks; the pre-registered size is 40."]


def test_k163_a_stage_a_file_without_rules_check_should_keep_the_size_caveat(tmp_path):
    # Regression guard (passes already): an older SWE-bench Lite stage-a file (K148).
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path, rules_check=False))
    assert caveat_lines(text) == ["- Suite A has 10 tasks; the pre-registered size is 40."]


# ---- K164 the left-out count covers suite A runs only ----

OUTSIDE_PREFIX = "outside the pre-registration"


def outside_lines(text):
    return [ln for ln in text.splitlines() if OUTSIDE_PREFIX in ln]


def extra_repeat_campaign(tmp_path):
    # Repeat 2 of every arm on t0 only: 3 suite A runs outside the expected set.
    skip = {(a, t, 2) for a in ARMS for t in TASKS if t != "t0"}
    return a_campaign(tmp_path, repeats=(0, 1, 2), skip=skip)


def test_k164_runs_of_other_suites_should_not_enter_the_left_out_count(tmp_path):
    camp = extra_repeat_campaign(tmp_path)
    for arm in ("pi", "hermes"):
        for rep in (0, 1):
            write_run(camp, b_run(arm, rep, task="b1"))
    write_run(camp, {**b_run("pi", 0, task="s1"), "suite": "smoke", "run_id": "smoke.pi.s0.s1"})
    text, _ = build(camp, prereg_a(tmp_path))
    assert outside_lines(text) == ["3 runs outside the pre-registration are left out."]


def test_k164_the_count_should_be_the_same_without_the_other_suites(tmp_path):
    text, _ = build(extra_repeat_campaign(tmp_path), prereg_a(tmp_path))
    assert outside_lines(text) == ["3 runs outside the pre-registration are left out."]


@pytest.mark.parametrize("n_other", [1, 5])
def test_k164_no_suite_a_run_outside_should_mean_no_line_whatever_else_is_there(tmp_path, n_other):
    camp = a_campaign(tmp_path)
    for rep in range(n_other):
        write_run(camp, b_run("pi", rep, task="b1"))
    text, _ = build(camp, prereg_a(tmp_path))
    assert outside_lines(text) == []
