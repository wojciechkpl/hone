"""Spec section 11.17, round 16c, K150-K157: what the review of round 16b found.

Contract choices made here (the spec leaves them open):
* K150: `report.apply_outcomes` sets `grade_status` "stale" and `resolved` None for a row that
  does not fit its run; a fitting `instrumentation_error` row gives status
  `instrumentation_error` and resolved False.
* K151: `execute_run` is driven with a fake executor and a fake admin HTTP whose records
  endpoint raises `urllib.error.URLError`.
* K152: mismatch dicts have `run_id`, `field`, `expected`, `found`. For a missing run the
  `expected` text is "<arm> <task> repeat <r>" (backticks around arm and task are allowed and
  stripped) and `run_id` is not pinned. For a run of an arm outside HC_ARMS_MAIN one or more
  `arm` mismatches are written (per run or per arm); each names the arm somewhere in the dict.
  `HC_ARMS_MAIN` is a comma-separated string, like `HC_A_REPEATS`.
* K153: the heading ends with "not final: " and the non-zero parts; the Holm cell of H1 and the
  claimed cell of H2 read "not final" on family rows, and stay "-" on outside rows.
* K154: `hcbench.grade.main` returns 0 and writes outcomes.csv for a campaign whose only suite A
  runs are instrumentation errors.
"""

import json
import urllib.error

import pytest
from test_campaign1_report import run_report
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
    prereg_a,
    subsection,
    table,
)
from test_campaign1_round16b import ALL_ARMS, FAMILY, OUTSIDE, claim_lines, h1_rows, instrument
from test_campaign1_seed import ROOT, FakeHttp, fake_exec, ok

from hcbench import grade, prereg, report
from hcbench.arms import load_arms
from hcbench.config import BenchConfig
from hcbench.runner import execute_run
from hcbench.suites import Task

# ---- K150 a grade must match its run ----


def a_run(**over):
    return {"suite": "a", "run_id": "a.pi.s0.t1", "patch_sha256": "p", **over}


BROKEN = {"instrumentation_error": "RuntimeError: broke", "patch_sha256": None}


def row_of(status, patch="", resolved="0"):
    return {"run_id": "a.pi.s0.t1", "patch_sha256": patch, "resolved": resolved, "status": status}


def apply_one(run, row):
    runs = [run]
    report.apply_outcomes(runs, {run["run_id"]: row})
    return runs[0]


@pytest.mark.parametrize(
    ("run", "row"),
    [
        # A broken run was graded, re-run with --force, and reported before a re-grade.
        (a_run(), row_of("instrumentation_error")),
        # The run broke after its grade: the row came from a clean run (hand-blank patch).
        (a_run(**BROKEN), row_of("resolved", resolved="1")),
        (a_run(**BROKEN), row_of("unresolved")),
        # Both errors: the run is an infrastructure error first, so it is not a broken run.
        (a_run(**BROKEN, infra_error="records: boom"), row_of("instrumentation_error")),
        (a_run(**BROKEN, infra_error="records: boom"), row_of("unresolved")),
        # An infrastructure error alone never fits an instrumentation_error row.
        (
            a_run(infra_error="docker run failed", patch_sha256=None),
            row_of("instrumentation_error"),
        ),
    ],
    ids=[
        "clean-run-broken-row",
        "broken-run-resolved-row",
        "broken-run-unresolved-row",
        "both-errors-broken-row",
        "both-errors-other-row",
        "infra-run-broken-row",
    ],
)
def test_k150_a_row_that_does_not_fit_its_run_should_make_the_run_stale(run, row):
    got = apply_one(run, row)
    assert got["grade_status"] == "stale"
    assert got["resolved"] is None


def test_k150_an_instrumentation_error_row_should_fit_a_run_with_that_error_alone():
    got = apply_one(a_run(**BROKEN), row_of("instrumentation_error"))
    assert got["grade_status"] == "instrumentation_error"
    assert got["resolved"] is False


def test_k150_a_graded_broken_run_re_run_before_a_regrade_should_not_make_a_final_report(
    tmp_path,
):
    key = ("pi", "t3", 0)
    camp = a_campaign(
        tmp_path, status=lambda a, t, r: "instrumentation_error" if (a, t, r) == key else None
    )
    path = camp / "a" / "pi" / "s0" / "t3" / "run.json"
    run = json.loads(path.read_text())
    run["instrumentation_error"] = None  # re-run with --force: now a clean run with a patch
    run["patch_sha256"] = "p"
    path.write_text(json.dumps(run))
    text, conformance = build(camp, prereg_a(tmp_path))
    found = [m for m in conformance["mismatches"] if m["field"] == "grade_status"]
    assert [(m["run_id"], m["found"]) for m in found] == [("a.pi.s0.t3", "stale")]
    assert "not final" in subsection(text, "H1")[0]


# ---- K151 a lost request log is an infrastructure error ----


class LostRecords(FakeHttp):
    """The admin API answers everything but the records of a run."""

    def __call__(self, method, url, payload=None, timeout=30, token=""):
        if method == "GET" and url.endswith("/records"):
            raise urllib.error.URLError("connection refused")
        return super().__call__(method, url, payload, timeout=timeout, token=token)


def exec_failing_agent(argv, *, input=None, timeout=None):
    if "timeout" in argv and "-k" in argv:  # the agent's own command
        raise RuntimeError("the agent broke its checkout")
    return ok()


def one_run(tmp_path, monkeypatch, *, ex=fake_exec, http=None, suite="a", **kw):
    env = {"HC_BUNDLE_DIR": str(tmp_path / "bundle"), "HC_ADMIN_TOKEN": "t"}
    monkeypatch.delenv("HC_SEED_BASE", raising=False)
    cfg = BenchConfig.from_env(env)
    arm = load_arms(ROOT / "config" / "arms.toml")["pi"]
    task = Task(suite=suite, task_id="t1", image="img", workdir="/work", prompt="p")
    out = tmp_path / "out"
    return execute_run(task, arm, 0, cfg, out, ex=ex, http=http or LostRecords(), **kw), out


@pytest.mark.parametrize("suite", ["a", "b"])
def test_k151_lost_records_after_the_harness_started_should_be_an_infra_error(
    tmp_path, monkeypatch, suite
):
    res, _ = one_run(tmp_path, monkeypatch, suite=suite)
    assert str(res["infra_error"]).startswith("records:")
    assert res["instrumentation_error"] is None


def test_k151_an_earlier_instrumentation_error_should_stay_beside_the_infra_error(
    tmp_path, monkeypatch
):
    res, out = one_run(tmp_path, monkeypatch, ex=exec_failing_agent)
    assert str(res["infra_error"]).startswith("records:")
    assert "the agent broke its checkout" in str(res["instrumentation_error"])
    on_disk = json.loads((out / "a" / "pi" / "s0" / "t1" / "run.json").read_text())
    assert on_disk["infra_error"] == res["infra_error"]
    assert on_disk["instrumentation_error"] == res["instrumentation_error"]


def test_k151_grade_should_skip_a_run_whose_records_were_lost(tmp_path, monkeypatch):
    _, out = one_run(tmp_path, monkeypatch, ex=exec_failing_agent)
    assert grade.collect_instrumented(out) == []


def test_k151_retry_errors_should_re_run_a_run_whose_records_were_lost(tmp_path, monkeypatch):
    first, _ = one_run(tmp_path, monkeypatch)
    assert first["infra_error"]
    again, _ = one_run(tmp_path, monkeypatch, http=FakeHttp(), retry_errors=True)
    assert not again.get("skipped")
    assert again["infra_error"] is None
    kept, _ = one_run(tmp_path, monkeypatch, http=FakeHttp())
    assert kept["skipped"] is True  # the healthy run is final again


def test_k151_analysis_plan_should_state_the_rule():
    text = " ".join(str(prereg.ANALYSIS_PLAN_A["exclusions"]).split())
    assert "A run whose request log can't be read is an infrastructure error." in text


def test_k151_h2_should_not_count_a_missing_value_as_zero(tmp_path):
    # Regression guard (may already pass): pi's t3 has wall 300 in repeat 0 and none in repeat 1;
    # t4 has no token count at all for hermes. Missing values leave the mean, never become 0.
    def wall(a, t, r):
        if a == "pi" and t == "t3":
            return 300.0 if r == 0 else None
        return 100.0

    def tokens(a, t, r):
        return None if (a, t) == ("hermes", "t4") else 1000

    text, _ = build(a_campaign(tmp_path, wall=wall, tokens=tokens), prereg_a(tmp_path))
    rows = h2_rows(text)
    pi_wall = rows[(f"hone-inject {MINUS} pi", "wall seconds")]
    assert num(pi_wall["mean paired difference per task"]) == pytest.approx(-20)  # (100-300)/10
    hermes_tok = rows[(f"hone-inject {MINUS} hermes", "total tokens")]
    assert num(hermes_tok["mean paired difference per task"]) == pytest.approx(0)
    assert (num(hermes_tok["tasks used"]), num(hermes_tok["tasks left out"])) == (9, 1)


# ---- K152 every pre-registered run is there ----


def mark_infra(camp, arm, task, repeat):
    """Rewrite a run as runner.py writes an infrastructure error: no patch."""
    path = camp / "a" / arm / f"s{repeat}" / task / "run.json"
    run = json.loads(path.read_text())
    run["infra_error"] = "InfraError: docker run failed"
    run["patch_sha256"] = None
    run["patch_bytes"] = None
    path.write_text(json.dumps(run))


def by_field(conformance, field):
    return [m for m in conformance["mismatches"] if m["field"] == field]


def plain(text):
    return str(text).replace("`", "")


def test_k152_a_missing_run_should_be_a_mismatch(tmp_path):
    camp = a_campaign(tmp_path, skip={("pi", "t3", 1)})
    _, conformance = build(camp, prereg_a(tmp_path))
    (m,) = by_field(conformance, "run")
    assert plain(m["expected"]) == "pi t3 repeat 1"
    assert m["found"] == "no run"


def test_k152_a_missing_run_should_make_the_report_exit_with_the_conformance_code(tmp_path):
    camp = a_campaign(tmp_path, skip={("pi", "t3", 1)})
    assert run_report(camp, prereg_a(tmp_path)) == report.CONFORMANCE_EXIT


def test_k152_a_whole_missing_arm_should_list_each_of_its_runs(tmp_path):
    camp = a_campaign(tmp_path, arms=("pi", "hone-inject"))  # hermes never ran
    _, conformance = build(camp, prereg_a(tmp_path))
    got = sorted(plain(m["expected"]) for m in by_field(conformance, "run"))
    assert got == sorted(f"hermes {t} repeat {r}" for t in TASKS for r in (0, 1))


def test_k152_an_infra_error_run_should_be_a_mismatch_with_its_run_id(tmp_path):
    camp = a_campaign(tmp_path)
    mark_infra(camp, "hermes", "t4", 0)
    _, conformance = build(camp, prereg_a(tmp_path))
    (m,) = by_field(conformance, "run")
    assert m["run_id"] == "a.hermes.s0.t4"
    assert plain(m["expected"]) == "hermes t4 repeat 0"
    assert m["found"] == "infrastructure error, not re-run"


def test_k152_an_infra_error_run_should_get_no_grade_status_mismatch(tmp_path):
    camp = a_campaign(tmp_path)
    mark_infra(camp, "hermes", "t4", 0)  # its old grade is stale now
    _, conformance = build(camp, prereg_a(tmp_path))
    assert by_field(conformance, "grade_status") == []


def test_k152_a_run_of_an_arm_outside_hc_arms_main_should_be_a_mismatch(tmp_path):
    camp = a_campaign(tmp_path, arms=ALL_ARMS)  # hone-noinject and hone-callgraph are extra
    _, conformance = build(camp, prereg_a(tmp_path))
    found = by_field(conformance, "arm")
    assert found
    text = json.dumps(found)
    assert "hone-noinject" in text and "hone-callgraph" in text
    assert "hone-inject" not in text  # an arm in HC_ARMS_MAIN is not reported
    assert by_field(conformance, "run") == []  # the extra arms are not expected runs


@pytest.mark.parametrize(
    "over",
    [
        {"config": {"HC_SERVED_NAME": "m", "HC_A_REPEATS": "0,1"}},
        {"config": {"HC_SERVED_NAME": "m", "HC_ARMS_MAIN": ",".join(ARMS)}},
        {"config": {"HC_SERVED_NAME": "m", "HC_A_REPEATS": "0,1", "HC_ARMS_MAIN": ""}},
        {"task_images": {}},
    ],
    ids=["no-arms", "no-repeats", "empty-arms", "no-task-images"],
)
def test_k152_a_file_that_expects_nothing_should_be_a_mismatch(tmp_path, over):
    _, conformance = build(a_campaign(tmp_path), prereg_a(tmp_path, **over))
    assert len(by_field(conformance, "expected_runs")) == 1


# ---- K153 not final covers every unfinished run ----


def unfinished(tmp_path, missing, infra, pending):
    """`missing` pi runs absent, `infra` hermes runs with an infrastructure error, `pending`
    hone-inject runs with a pending grade; all of repeat 0 on different tasks."""
    skip = {("pi", f"t{k}", 0) for k in range(missing)}
    camp = graded(
        tmp_path,
        {("hone-inject", f"t{k}", 0): "grader_infra" for k in range(pending)},
        skip=skip,
    )
    for k in range(infra):
        mark_infra(camp, "hermes", f"t{k}", 0)
    return camp


@pytest.mark.parametrize(
    ("counts", "parts"),
    [
        ((1, 0, 0), "1 run missing"),
        ((0, 1, 0), "1 run pending re-run"),
        ((0, 0, 1), "1 run pending grading"),
        ((2, 0, 3), "2 runs missing, 3 runs pending grading"),
        ((0, 2, 1), "2 runs pending re-run, 1 run pending grading"),
        ((1, 2, 3), "1 run missing, 2 runs pending re-run, 3 runs pending grading"),
    ],
)
@pytest.mark.parametrize("marker", ["H1", "H2"])
def test_k153_headings_should_name_every_non_zero_part_in_order(tmp_path, marker, counts, parts):
    text, _ = build(unfinished(tmp_path, *counts), prereg_a(tmp_path))
    assert subsection(text, marker)[0].endswith(f"not final: {parts}")


@pytest.mark.parametrize("marker", ["H1", "H2"])
def test_k153_a_run_should_count_once_even_when_its_grade_is_stale(tmp_path, marker):
    # Both hermes runs have an infra_error and a stale old grade: two re-runs, no grading.
    camp = a_campaign(tmp_path)
    mark_infra(camp, "hermes", "t1", 0)
    mark_infra(camp, "hermes", "t2", 1)
    text, _ = build(camp, prereg_a(tmp_path))
    heading = subsection(text, marker)[0]
    assert heading.endswith("not final: 2 runs pending re-run")
    assert "pending grading" not in heading and "re-grade" not in heading


@pytest.mark.parametrize("marker", ["H1", "H2"])
def test_k153_no_claim_lines_should_be_written_for_a_missing_run(tmp_path, marker):
    text, _ = build(unfinished(tmp_path, 1, 0, 0), prereg_a(tmp_path))
    assert claim_lines(subsection(text, marker)) == []


def test_k153_family_rows_should_read_not_final_in_h1(tmp_path):
    text, _ = build(unfinished(tmp_path, 1, 0, 0), prereg_a(tmp_path))
    rows = {r["comparison"]: r for r in h1_rows(text)}
    assert [rows[c]["Holm"] for c in FAMILY] == ["not final", "not final"]
    assert rows[OUTSIDE[1]]["Holm"] == "-"  # pi - hermes: outside, never a decision


def test_k153_family_rows_should_read_not_final_in_h2(tmp_path):
    text, _ = build(unfinished(tmp_path, 0, 1, 0), prereg_a(tmp_path))
    rows = table(subsection(text, "H2"))
    family = [r for r in rows if r["comparison"] in FAMILY]
    assert len(family) == 4 and {r["claimed"] for r in family} == {"not final"}
    assert {r["claimed"] for r in rows if r["comparison"] in OUTSIDE} == {"-"}


# ---- K154 broken runs are written even when nothing can be graded ----

IDS3 = ["a__a-1", "b__b-2", "c__c-3"]


@pytest.mark.parametrize("fork", [True, False])
def test_k154_only_broken_runs_should_still_write_their_rows_and_exit_zero(
    tmp_path, monkeypatch, fork
):
    camp = make_campaign(tmp_path, ["a__a-1", "c__c-3"])
    instrument(camp, "a__a-1")
    instrument(camp, "c__c-3")
    rc, camp, ev = run_fork(tmp_path, monkeypatch, [{}], ["a__a-1", "c__c-3"], fork=fork, camp=camp)
    assert rc == 0
    assert outcomes(camp) == {
        "a__a-1": ("0", "instrumentation_error"),
        "c__c-3": ("0", "instrumentation_error"),
    }
    assert ev.cmds == []  # nothing was sent to the grader


def test_k154_with_neither_graded_nor_broken_runs_it_should_stop_as_before(tmp_path, monkeypatch):
    # Regression guard (may already pass): an infrastructure error is neither.
    camp = make_campaign(tmp_path, ["a__a-1"])
    instrument(camp, "a__a-1", key="infra_error")
    with pytest.raises(SystemExit) as exc:
        run_fork(tmp_path, monkeypatch, [{}], ["a__a-1"], camp=camp)
    assert "no finished suite-a runs" in str(exc.value)


# ---- K155 one definition of the family ----


def test_k155_h2_contrasts_should_be_gone():
    assert not hasattr(report, "H2_CONTRASTS")


def test_k155_the_two_family_definitions_should_hold_the_same_pairs():
    assert set(report.STAGE_A_FAMILY) == report.PRIMARY_FAMILY
    assert len(report.STAGE_A_FAMILY) == len(report.PRIMARY_FAMILY) == 2


# ---- K157 mutants that survived ----


def test_k157_a_family_p_between_alpha_over_two_and_alpha_should_not_be_claimed(tmp_path):
    # hone-inject beats pi on 6 tasks and loses 1 (p = 0.045, above 0.025 and at most 0.05), and
    # beats hermes on 6 and loses 2 (a larger p). Holm needs 0.05 / 2 for the smaller one.
    def resolved(arm, task, rep):
        k = int(task[1:])
        if arm == "hone-inject":
            return k <= 5
        if arm == "pi":
            return k == 6
        return k in (6, 7)  # hermes

    camp = a_campaign(tmp_path, repeats=(0,), resolved=resolved)
    text, _ = build(camp, prereg_a(tmp_path, repeats="0"))
    rows = {r["comparison"]: r for r in h1_rows(text)}
    p_pi = num(rows[FAMILY[0]]["p (paired bootstrap)"])
    p_hermes = num(rows[FAMILY[1]]["p (paired bootstrap)"])
    assert 0.025 < p_pi <= 0.05 and p_hermes > p_pi
    assert rows[FAMILY[0]]["Holm"] == "no" and rows[FAMILY[1]]["Holm"] == "no"
    lines = claim_lines(subsection(text, "H1"))
    assert len(lines) == 2 and all("no detectable difference" in ln for ln in lines)


def test_k157_the_holm_adjusted_p_should_never_exceed_one(tmp_path):
    # Regression guard (may already pass): with p near 1 the step-down factor 4 would give 4.
    wobble = lambda t: 100 if int(t[1:]) % 2 == 0 else -100  # noqa: E731
    camp = a_campaign(
        tmp_path,
        tokens=lambda a, t, r: 1000 + (wobble(t) if a == "hone-inject" else 0),
        wall=lambda a, t, r: 100.0 + (wobble(t) / 100 if a == "hone-inject" else 0),
    )
    text, _ = build(camp, prereg_a(tmp_path))
    family = [r for r in table(subsection(text, "H2")) if r["comparison"] in FAMILY]
    adjusted = [num(r["p (Holm-adjusted)"]) for r in family]
    assert len(adjusted) == 4 and all(0 < p <= 1 for p in adjusted)
    assert max(adjusted) == pytest.approx(1.0)


@pytest.mark.parametrize("marker", ["H1", "H2"])
def test_k157_the_claim_line_n_should_count_a_task_no_arm_has_a_judged_run_for(tmp_path, marker):
    # Regression guard (may already pass): t9 is unjudged in every arm and repeat.
    statuses = {(a, "t9", r): "grader_infra_final" for a in ARMS for r in (0, 1)}
    text, _ = build(graded(tmp_path, statuses), prereg_a(tmp_path))
    lines = claim_lines(subsection(text, marker))
    assert lines and all("at n=10" in ln for ln in lines)
    assert "n=9" not in "\n".join(subsection(text, marker))


def test_k157_the_unjudged_list_should_be_sorted_by_arm_then_run(tmp_path):
    # Loaded in task order (pi t1, hermes t2, hone-inject t3); sorted by arm they differ.
    statuses = {
        ("pi", "t1", 0): "grader_infra_final",
        ("hermes", "t2", 0): "grader_infra_final",
        ("hone-inject", "t3", 0): "grader_infra_final",
        ("pi", "t5", 0): "grader_infra_final",
        ("pi", "t2", 1): "grader_infra_final",
    }
    text, _ = build(graded(tmp_path, statuses), prereg_a(tmp_path))
    listed = [ln for ln in text.splitlines() if "grader_infra_final (" in ln]
    ids = [ln.split(":")[1].strip() for ln in listed]
    assert ids == [
        "a.hermes.s0.t2",
        "a.hone-inject.s0.t3",
        "a.pi.s0.t1",
        "a.pi.s0.t5",
        "a.pi.s1.t2",
    ]


def test_k157_the_broken_run_list_should_be_sorted_by_arm_too(tmp_path):
    broken = {
        ("pi", "t1", 0): "instrumentation_error",
        ("hermes", "t2", 0): "instrumentation_error",
        ("hone-inject", "t3", 0): "instrumentation_error",
    }
    text, _ = build(graded(tmp_path, broken), prereg_a(tmp_path))
    listed = [ln for ln in text.splitlines() if "instrumentation_error (" in ln]
    assert [ln.split(":")[1].strip() for ln in listed] == [
        "a.hermes.s0.t2",
        "a.hone-inject.s0.t3",
        "a.pi.s0.t1",
    ]


@pytest.mark.parametrize("fork", [True, False])
def test_k157_a_run_with_both_errors_should_get_no_row_in_outcomes(tmp_path, monkeypatch, fork):
    # Regression guard (may already pass).
    camp = make_campaign(tmp_path, IDS3)
    instrument(camp, "b__b-2")
    both = camp / "a" / "b__b-2" / "s0" / "pi" / "run.json"
    run = json.loads(both.read_text())
    run["infra_error"] = "records: connection refused"
    both.write_text(json.dumps(run))
    step = {"report": {"resolved_ids": ["a__a-1", "c__c-3"]}}
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step], IDS3, fork=fork, camp=camp)
    assert "b__b-2" not in outcomes(camp)
    assert set(outcomes(camp)) == {"a__a-1", "c__c-3"}
