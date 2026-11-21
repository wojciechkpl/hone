"""Spec section 11.17, round 16b, K144-K149: what the review of round 16 found.

Contract choices made here (the spec leaves them open):
* K144/K146: stage-a H1 columns keep `Holm` (the decision) and add `p (paired bootstrap)`; the
  old `tasks` column is gone. H2 keeps `p` (the bootstrap p) and `p (Holm-adjusted)` (the
  step-down adjusted p), with `claimed`. A row of an outside contrast has the text
  "outside the family" in one of its cells, `-` for the Holm decision / adjusted p / claimed.
* K145: `grade.collect` / `outcomes.csv` gives an `instrumentation_error` run status
  `instrumentation_error` and resolved `0`, and it is not sent to the grader. The report lists
  it on a line with its run id and the status.
* K146: the suite A table note is "all judged runs' tokens ÷ resolved runs".
* K147 (heading text replaced by K153): the H1 and H2 headings say "not final: N runs pending
  grading" and no `- ` claim line ("difference claimed" / "no detectable difference") is
  written in those sections.
"""
# ruff: noqa: F401

import json
import re
from pathlib import Path

import pytest
from test_campaign1_round15 import make_campaign
from test_campaign1_round15b import outcomes, run_fork
from test_campaign1_round16 import (
    ARMS,
    MINUS,
    TASKS,
    a_campaign,
    build,
    graded,
    h2_rows,
    num,
    nums,
    prereg_a,
    subsection,
    table,
    with_unjudged_pi_t9,
)

from hcbench import grade, prereg, sampling
from hcbench.stats import bootstrap_pvalue

ALL_ARMS = ("pi", "hone-inject", "hermes", "hone-noinject", "hone-callgraph")
FAMILY = (f"hone-inject {MINUS} pi", f"hone-inject {MINUS} hermes")
OUTSIDE = (f"hone-inject {MINUS} hone-noinject", f"pi {MINUS} hermes")


def holm_adjusted(ps):
    """Step-down adjusted p-values: max over ranks j <= i of min(1, (m - j) * p(j))."""
    m = len(ps)
    order = sorted(range(m), key=lambda i: ps[i])
    out, running = [0.0] * m, 0.0
    for rank, i in enumerate(order):
        running = max(running, min(1.0, (m - rank) * ps[i]))
        out[i] = running
    return out


def h1_rows(text):
    return table(subsection(text, "H1"))


def is_outside(row):
    return "outside the family" in " ".join(row.values())


# ---- K144 the contrasts ----


def five_arm_campaign(tmp_path, **kw):
    return a_campaign(tmp_path, arms=ALL_ARMS, **kw)


def test_k144_h1_should_hold_exactly_the_four_contrasts_of_k101(tmp_path):
    text, _ = build(five_arm_campaign(tmp_path), prereg_a(tmp_path, arms_main=ALL_ARMS))
    rows = h1_rows(text)
    assert [r["comparison"] for r in rows] == [*FAMILY, *OUTSIDE]
    assert [is_outside(r) for r in rows] == [False, False, True, True]


def test_k144_no_other_pair_should_appear_in_h1_or_h2(tmp_path):
    text, _ = build(five_arm_campaign(tmp_path), prereg_a(tmp_path, arms_main=ALL_ARMS))
    for marker in ("H1", "H2"):
        body = "\n".join(subsection(text, marker))
        assert "hone-callgraph" not in body
        assert f"hone-noinject {MINUS} pi" not in body


def test_k144_outside_rows_should_carry_no_holm_decision_in_h1(tmp_path):
    text, _ = build(five_arm_campaign(tmp_path), prereg_a(tmp_path, arms_main=ALL_ARMS))
    for row in h1_rows(text):
        if is_outside(row):
            assert row["Holm"] == "-"
        else:
            assert row["Holm"] in ("yes", "no")


def test_k144_h2_should_have_the_outside_contrasts_as_descriptive_rows(tmp_path):
    text, _ = build(five_arm_campaign(tmp_path), prereg_a(tmp_path, arms_main=ALL_ARMS))
    rows = table(subsection(text, "H2"))
    keys = {(r["comparison"], r["metric"]) for r in rows}
    assert keys == {(c, m) for c in (*FAMILY, *OUTSIDE) for m in ("total tokens", "wall seconds")}
    for row in rows:
        if row["comparison"] in OUTSIDE:
            assert is_outside(row)
            assert row["claimed"] == "-" and row["p (Holm-adjusted)"] == "-"
        else:
            assert not is_outside(row)
            assert row["claimed"] in ("yes", "no")


def test_k144_h1_holm_should_run_over_the_two_family_contrasts_only(tmp_path):
    # hone beats pi and hermes with p = 0.0178 each (7 tasks +1, 1 task -1, 2 ties). Holm over
    # the two family tests claims both (0.0178 <= 0.025); with the two outside contrasts (p = 1)
    # in the family of 4 it would need 0.0125 and claim neither.
    def resolved(arm, task, rep):
        k = int(task[1:])
        if arm in ("hone-inject", "hone-noinject"):
            return k <= 6 or k >= 8
        return k >= 7  # pi, hermes, hone-callgraph

    camp = five_arm_campaign(tmp_path, repeats=(0,), resolved=resolved)
    text, _ = build(camp, prereg_a(tmp_path, repeats="0", arms_main=ALL_ARMS))
    rows = {r["comparison"]: r for r in h1_rows(text)}
    for label in FAMILY:
        assert num(rows[label]["p (paired bootstrap)"]) == pytest.approx(0.0178, abs=0.0006)
        assert rows[label]["Holm"] == "yes"


def test_k146_h1_should_drop_the_tasks_column_and_add_the_bootstrap_p(tmp_path):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))
    row = h1_rows(text)[0]
    assert "p (paired bootstrap)" in row
    assert "tasks" not in row
    assert "tasks used" in row and "tasks left out" in row


def test_k146_h2_should_replace_holm_by_the_adjusted_p(tmp_path):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))
    row = next(iter(h2_rows(text).values()))
    assert "p (Holm-adjusted)" in row and "Holm" not in row


def test_k144_h2_adjusted_p_should_follow_a_hand_computation(tmp_path):
    # Hand computation: four family p-values 0.019 each, Holm step-down:
    # 4 * 0.019 = 0.076, 3 * 0.019 = 0.057, ... the running maximum is 0.076 for all four.
    d = [3, -3, 5, 0, 4, 4, 5, 0, 2, 0]
    hone_t = {t: 5000 for t in TASKS}
    other_t = {t: 5000 - 100 * d[k] for k, t in enumerate(TASKS)}
    hone_w = {t: 200.0 for t in TASKS}
    other_w = {t: 200.0 - d[k] for k, t in enumerate(TASKS)}
    camp = a_campaign(
        tmp_path,
        tokens=lambda a, t, r: hone_t[t] if a == "hone-inject" else other_t[t],
        wall=lambda a, t, r: hone_w[t] if a == "hone-inject" else other_w[t],
    )
    text, _ = build(camp, prereg_a(tmp_path))
    family = [r for r in table(subsection(text, "H2")) if r["comparison"] in FAMILY]
    assert len(family) == 4
    assert [num(r["p (Holm-adjusted)"]) for r in family] == pytest.approx([0.076] * 4, abs=0.0025)
    assert {r["claimed"] for r in family} == {"no"}


def test_k144_h2_holm_should_run_over_the_family_only_and_match_an_independent_holm(tmp_path):
    d1 = [3, -3, 5, 0, 4, 4, 5, 0, 2, 0]  # p = 0.019
    d2 = [5, 4, 3, 3, 2, 2, 1, 1, -1, -2]  # p = 0.0114
    hone_t = {t: 5000 for t in TASKS}
    pi_t = {t: 5000 - 100 * d1[k] for k, t in enumerate(TASKS)}
    hermes_t = {t: 5000 - 100 * d2[k] for k, t in enumerate(TASKS)}
    hone_w = {t: 200.0 for t in TASKS}
    pi_w = {t: 200.0 - d1[k] for k, t in enumerate(TASKS)}
    hermes_w = {t: 200.0 - d2[k] for k, t in enumerate(TASKS)}
    tok = {"hone-inject": hone_t, "hone-noinject": hone_t, "pi": pi_t, "hermes": hermes_t}
    wal = {"hone-inject": hone_w, "hone-noinject": hone_w, "pi": pi_w, "hermes": hermes_w}
    camp = a_campaign(
        tmp_path,
        arms=("pi", "hone-inject", "hermes", "hone-noinject"),
        tokens=lambda a, t, r: tok[a][t],
        wall=lambda a, t, r: wal[a][t],
    )
    text, _ = build(camp, prereg_a(tmp_path, arms_main=ALL_ARMS[:4]))
    rows = {(r["comparison"], r["metric"]): r for r in table(subsection(text, "H2"))}
    order = [
        (FAMILY[0], "total tokens", hone_t, pi_t),
        (FAMILY[0], "wall seconds", hone_w, pi_w),
        (FAMILY[1], "total tokens", hone_t, hermes_t),
        (FAMILY[1], "wall seconds", hone_w, hermes_w),
    ]
    ps = [bootstrap_pvalue(a, b) for _, _, a, b in order]
    expected = holm_adjusted(ps)  # over the four family tests only
    for (c, m, _, _), adj in zip(order, expected, strict=True):
        row = rows[(c, m)]
        assert num(row["p (Holm-adjusted)"]) == pytest.approx(adj, abs=0.0006)
        assert row["claimed"] == ("yes" if adj <= 0.05 else "no")
    assert all(rows[(c, m)]["claimed"] == "yes" for c, m, _, _ in order)  # 0.0456 <= 0.05
    for (c, _), row in rows.items():
        if c in OUTSIDE:
            assert row["claimed"] == "-"


# ---- K145 a run that breaks its own measurement ----

IDS3 = ["a__a-1", "b__b-2", "c__c-3"]


def instrument(camp, iid, key="instrumentation_error"):
    """Rewrite one run as runner.py writes a failed-after-start run: the error set, no patch."""
    rdir = camp / "a" / iid / "s0" / "pi"
    run = json.loads((rdir / "run.json").read_text())
    run[key] = "RuntimeError: the agent broke its checkout"
    run["patch_sha256"] = None
    run["patch_bytes"] = None
    (rdir / "run.json").write_text(json.dumps(run))
    (rdir / "patch.diff").unlink()


@pytest.mark.parametrize("fork", [True, False])
def test_k145_grade_should_record_status_and_not_send_the_run_to_the_grader(
    tmp_path, monkeypatch, fork
):
    camp = make_campaign(tmp_path, IDS3)
    instrument(camp, "b__b-2")
    step = {"report": {"resolved_ids": ["a__a-1", "c__c-3"]}}
    rc, camp, ev = run_fork(tmp_path, monkeypatch, [step], IDS3, fork=fork, camp=camp)
    assert outcomes(camp) == {
        "a__a-1": ("1", "resolved"),
        "b__b-2": ("0", "instrumentation_error"),
        "c__c-3": ("1", "resolved"),
    }
    (cmd,) = ev.cmds
    assert "b__b-2" not in cmd
    predictions = cmd[cmd.index("--predictions_path") + 1]
    assert "b__b-2" not in Path(predictions).read_text()
    assert rc == 0  # nothing to re-grade


def test_k145_an_infra_error_run_should_still_be_skipped_and_rerun(tmp_path, monkeypatch):
    camp = make_campaign(tmp_path, IDS3)
    instrument(camp, "b__b-2", key="infra_error")
    step = {"report": {"resolved_ids": ["a__a-1", "c__c-3"]}}
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step], IDS3, camp=camp)
    assert "b__b-2" not in outcomes(camp)


def instr_campaign(tmp_path):
    key = ("pi", "t3", 0)
    return a_campaign(
        tmp_path,
        status=lambda a, t, r: "instrumentation_error" if (a, t, r) == key else None,
        tokens=lambda a, t, r: 9000 if (a, t, r) == key else 1000,
        wall=lambda a, t, r: 500.0 if (a, t, r) == key else 100.0,
    )


def test_k145_the_report_should_count_the_run_as_unresolved_in_h1(tmp_path):
    text, _ = build(instr_campaign(tmp_path), prereg_a(tmp_path))
    row = next(r for r in h1_rows(text) if r["comparison"] == FAMILY[0])
    assert num(row["Δ pass@1"]) == pytest.approx(0.05)  # pi t3: 0 and 1, so 0.5 of 10 tasks


def test_k145_the_report_should_keep_its_tokens_and_wall_in_h2(tmp_path):
    text, _ = build(instr_campaign(tmp_path), prereg_a(tmp_path))
    rows = h2_rows(text)
    tokens = rows[(FAMILY[0], "total tokens")]
    wall = rows[(FAMILY[0], "wall seconds")]
    assert num(tokens["mean paired difference per task"]) == pytest.approx(-400)  # t3: 1000 - 5000
    assert num(wall["mean paired difference per task"]) == pytest.approx(-20)  # t3: 100 - 300
    assert num(tokens["tasks used"]) == 10 and num(tokens["tasks left out"]) == 0


def test_k145_the_report_should_list_the_run_per_arm(tmp_path):
    text, _ = build(instr_campaign(tmp_path), prereg_a(tmp_path))
    lines = [ln for ln in text.splitlines() if "a.pi.s0.t3" in ln and "instrumentation_error" in ln]
    assert lines


def test_k145_the_run_should_not_be_a_conformance_mismatch(tmp_path):
    _, conformance = build(instr_campaign(tmp_path), prereg_a(tmp_path))
    assert conformance["mismatches"] == []


def test_k145_analysis_plan_should_state_the_rule():
    text = " ".join(str(prereg.ANALYSIS_PLAN_A["exclusions"]).lower().split())
    assert "instrumentation_error" in text
    assert "unresolved" in text


# ---- K147 not final while re-grades are pending ----

PENDING = {("pi", "t3", 0): "grader_infra", ("hermes", "t4", 0): "ambiguous"}


def claim_lines(lines):
    return [
        ln
        for ln in lines
        if ln.startswith("- ") and ("difference claimed" in ln or "no detectable difference" in ln)
    ]


@pytest.mark.parametrize("marker", ["H1", "H2"])
def test_k147_headings_should_say_the_report_is_not_final(tmp_path, marker):
    text, _ = build(graded(tmp_path, PENDING), prereg_a(tmp_path))
    assert "not final: 2 runs pending grading" in subsection(text, marker)[0]


@pytest.mark.parametrize("marker", ["H1", "H2"])
def test_k147_no_claim_lines_should_be_written_while_re_grades_are_pending(tmp_path, marker):
    text, _ = build(graded(tmp_path, PENDING), prereg_a(tmp_path))
    assert claim_lines(subsection(text, marker)) == []


@pytest.mark.parametrize("status", ["ungraded", "stale", "missing"])
def test_k147_every_pending_status_should_count(tmp_path, status):
    text, _ = build(graded(tmp_path, {("pi", "t3", 0): status}), prereg_a(tmp_path))
    assert "not final: 1 run pending grading" in subsection(text, "H1")[0]


@pytest.mark.parametrize("marker", ["H1", "H2"])
def test_k147_a_final_report_should_keep_its_headings_and_claim_lines(tmp_path, marker):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))
    assert "not final" not in subsection(text, marker)[0]
    assert claim_lines(subsection(text, marker))


def test_k147_grader_infra_final_should_not_make_the_report_not_final(tmp_path):
    text, _ = build(with_unjudged_pi_t9(tmp_path), prereg_a(tmp_path))
    assert "not final" not in subsection(text, "H1")[0]


# ---- K148 only the SWE-rebench stage a ----


def broken_campaign(tmp_path):
    return graded(tmp_path, PENDING)


def test_k148_a_stage_a_file_without_rules_check_should_get_the_plain_report(tmp_path):
    d = prereg_a(tmp_path, rules_check=False, run_digest=sampling.manifest_digest(TASKS[:5]),
                 repeats="0")  # fmt: skip
    text, conformance = build(broken_campaign(tmp_path), d)
    assert "## Suite A" in text.splitlines()
    assert "possibly contaminated" not in text and "Tasks created" not in text
    assert "### Pairwise" in text
    assert "H2" not in "".join(ln for ln in text.splitlines() if ln.startswith("#"))
    assert conformance["mismatches"] == []


def test_k148_the_same_file_with_rules_check_should_get_the_stage_a_report(tmp_path):
    d = prereg_a(tmp_path, run_digest=sampling.manifest_digest(TASKS[:5]), repeats="0")
    _, conformance = build(broken_campaign(tmp_path), d)
    fields = {m["field"] for m in conformance["mismatches"]}
    assert {"task_digest", "grade_status"} <= fields
    assert any(f.startswith("repeats.") for f in fields)


# ---- K149 mutants ----


@pytest.mark.parametrize("marker", ["H1", "H2"])
def test_k149_the_claim_line_n_should_count_the_tasks_run_not_the_tasks_used(tmp_path, marker):
    text, _ = build(with_unjudged_pi_t9(tmp_path), prereg_a(tmp_path))  # pi has 9 judged tasks
    pi_lines = [ln for ln in claim_lines(subsection(text, marker)) if f"{MINUS} pi" in ln]
    assert pi_lines and all("at n=10" in ln for ln in pi_lines)
    assert "n=9" not in "\n".join(subsection(text, marker))


def test_k149_h2_should_use_the_per_task_mean_over_the_repeats(tmp_path):
    camp = a_campaign(
        tmp_path,
        tokens=lambda a, t, r: (1000 if r == 0 else 3000) if a == "hone-inject" else 1000,
        wall=lambda a, t, r: (100.0 if r == 0 else 300.0) if a == "hone-inject" else 100.0,
    )
    text, _ = build(camp, prereg_a(tmp_path))
    rows = h2_rows(text)
    tokens = rows[(FAMILY[0], "total tokens")]
    wall = rows[(FAMILY[0], "wall seconds")]
    assert num(tokens["mean paired difference per task"]) == pytest.approx(1000)  # 2000 - 1000
    assert num(wall["mean paired difference per task"]) == pytest.approx(100)  # 200 - 100


def test_k149_tokens_per_resolved_run_should_divide_judged_tokens_by_resolved_runs(tmp_path):
    tasks = ["t0", "t1", "t2", "t3"]
    solved = {("t0", 0), ("t0", 1), ("t1", 0)}  # 3 resolved runs, 2 resolved tasks
    pending = ("pi", "t3", 0)

    def status(arm, task, rep):
        return "grader_infra_final" if (arm, task, rep) == pending else None

    def resolved(arm, task, rep):
        if (arm, task, rep) == pending:
            return None
        return True if arm == "hone-inject" else (task, rep) in solved

    camp = a_campaign(
        tmp_path,
        arms=("pi", "hone-inject"),
        tasks=tasks,
        tokens=lambda a, t, r: 5000 if (a, t, r) == pending else 1000,
        status=status,
        resolved=resolved,
    )
    text, _ = build(camp, prereg_a(tmp_path, tasks=tasks))
    from test_campaign1_round16 import HEADING, subsection_after

    by_arm = {r["arm"]: r for r in table(subsection_after(text, HEADING))}
    pi = by_arm["pi"]
    assert "tokens per resolved run" in pi
    # 7 judged runs * 1000 tokens (the unjudged 5000 is out) / 3 resolved runs
    assert num(pi["tokens per resolved run"]) == pytest.approx(7000 / 3, abs=1)
    assert "all judged runs' tokens ÷ resolved runs" in text


def test_k149_the_header_digest_line_should_use_run_digest(tmp_path):
    good = sampling.manifest_digest(TASKS)
    drawn = sampling.manifest_digest([*TASKS, "t10"])
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path, digest=drawn, run_digest=good))
    line = next(ln for ln in text.splitlines() if "Pre-registered task digest" in ln)
    assert good in line and drawn not in line
    assert "MATCH" in line and "MISMATCH" not in line
