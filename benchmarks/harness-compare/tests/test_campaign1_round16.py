"""Spec section 11.17, round 16, K110-K113 and K141-K143: the stage-a report.

Contract choices made here (the spec leaves them open):
* K110: the heading and the created-at line are pinned as written in the spec; a report
  without a stage-a pre-registration headed the section plain "Suite A".
* K111/K112: under a stage-a pre-registration the suite A subsections are
  "### H1 ... (pre-registered)" (the pairwise table) and "### H2 ... (pre-registered)".
  H1 columns keep the old ones and add `tasks used` and `tasks left out`.
  H2 columns: `comparison`, `metric` (`total tokens` or `wall seconds`),
  `mean paired difference per task`, `95% CI (paired bootstrap)`, `p`, `p (Holm-adjusted)`,
  `claimed`, `tasks used`, `tasks left out`. The suite A table gets `tokens per resolved run`
  = all the arm's tokens divided by its resolved tasks.
* K113/K142: conformance mismatches are dicts with `run_id`, `field`, `expected`, `found`; the
  fields are `task_digest`, `image`, `repeats.<arm>` and `grade_status`.
* K141: a task with no judged run for an arm is left out of every contrast with that arm.
"""

import csv
import json
import re

import pytest
from test_campaign1_report import run_report, write_run
from test_campaign1_round15b import fork_log, outcomes, run_fork, run_log, step

from hcbench import prereg as prereg_mod
from hcbench import report, sampling
from hcbench.stats import bootstrap_pvalue

MINUS = "−"
PRICES = {"in_per_m": 0.0, "out_per_m": 0.0}
TASKS = [f"t{i}" for i in range(10)]
ARMS = ("pi", "hone-inject", "hermes")
HEADING = "## Suite A — SWE-rebench (SWE-rebench grading fork), possibly contaminated"
CREATED = {t: f"2026-03-{k + 2:02d} 09:18:02" for k, t in enumerate(TASKS)}
CREATED["t9"] = "2026-05-12 10:00:00"
CREATED_LINE = (
    "Tasks created 2026-03-02 to 2026-05-12. Qwen3.8 was released in August 2026, "
    "so the model may have seen these tasks or their fixes."
)


def image_of(task):
    return f"swerebench/sweb.eval.x86_64.{task}:latest@sha256:" + f"{int(task[1:]):064x}"


def a_campaign(
    tmp_path,
    *,
    arms=ARMS,
    tasks=TASKS,
    repeats=(0, 1),
    tokens=lambda arm, task, rep: 1000,
    wall=lambda arm, task, rep: 100.0,
    resolved=lambda arm, task, rep: True,
    status=None,
    image=image_of,
    skip=(),
):
    """A suite A campaign: runs on disk and grading/outcomes.csv. `resolved` may return None
    (unjudged); `status(arm, task, rep)` overrides the grade status, and the special statuses
    "ungraded" (no outcome row) and "stale" (another patch hash) are honoured."""
    camp = tmp_path / "camp"
    rows = []
    for arm in arms:
        for task in tasks:
            for rep in repeats:
                if (arm, task, rep) in skip:
                    continue
                run_id = f"a.{arm}.s{rep}.{task}"
                run = {
                    "run_id": run_id,
                    "suite": "a",
                    "arm": arm,
                    "repeat": rep,
                    "seed": 20261117 + rep,
                    "task_id": task,
                    "image": image(task),
                    "patch_sha256": "p",
                    "wall_s": wall(arm, task, rep),
                    "proxy": {"total_tokens": tokens(arm, task, rep)},
                }
                res = resolved(arm, task, rep)
                st = (status(arm, task, rep) if status else None) or (
                    "resolved" if res else "unresolved" if res is False else "missing"
                )
                if st == "instrumentation_error":  # K145: as runner.py writes such a run
                    run["instrumentation_error"] = "RuntimeError: the agent broke its checkout"
                    run["patch_sha256"] = None
                    run["patch_bytes"] = None
                    res = False
                write_run(camp, run)
                if st == "ungraded":
                    continue
                rows.append(
                    {
                        "run_id": run_id,
                        "patch_sha256": (
                            ""
                            if st == "instrumentation_error"
                            else "other"
                            if st == "stale"
                            else "p"
                        ),
                        "resolved": "" if res is None else int(bool(res)),
                        "status": "resolved" if st == "stale" else st,
                    }
                )
    (camp / "grading").mkdir(parents=True, exist_ok=True)
    with (camp / "grading" / "outcomes.csv").open("w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["run_id", "patch_sha256", "resolved", "status"])
        w.writeheader()
        w.writerows(rows)
    return camp


def prereg_a(
    tmp_path,
    *,
    tasks=TASKS,
    digest=None,
    run_digest="same",
    repeats="0,1",
    rules_check=True,
    **over,
):
    """A stage-a prereg directory: preregistration.toml and selection.json."""
    d = tmp_path / "prereg"
    d.mkdir(exist_ok=True)
    good = sampling.manifest_digest(tasks)
    run_digest = good if run_digest == "same" else run_digest
    digest = digest or good
    tasks_table = {"digest": digest, "n": 60, "seed": 1}
    if run_digest is not None:
        tasks_table["run_digest"] = run_digest
    data = {
        "title": "t",
        "stage": "a",
        "created_at": "2026-01-01T00:00:00Z",
        "draft": False,
        "repo": {"head": "", "dirty": False},
        "config": {
            "HC_SERVED_NAME": "qwen3.8-27b",
            "HC_A_REPEATS": repeats,
            "HC_ARMS_MAIN": ",".join(ARMS),  # K152: the arms every task is expected to have run
        },
        "bundle": {"x": 1},
        "tasks": tasks_table,
        "task_created_at": {t: CREATED[t] for t in tasks},
        "task_images": {t: image_of(t) for t in tasks},
        "excluded_tasks": {},
        "files_sha256": {},
        "analysis": {},
        **over,
    }
    if rules_check:  # K148: only a SWE-rebench stage-a file has [rules_check]
        data["rules_check"] = {"path": "prereg/c1/preregistration.toml", "sha256": "0" * 64,
                               "repo_head": "", "accepted": {}}  # fmt: skip
    (d / "preregistration.toml").write_text(prereg_mod.dump_toml(data))
    selection = {"digest": digest}
    if run_digest is not None:
        selection["run_digest"] = run_digest
    (d / "selection.json").write_text(json.dumps(selection))
    return d


def build(camp, prereg_dir=None):
    out, conformance = report.build(camp, prereg_dir, PRICES)
    return out.read_text(), conformance


def num(cell):
    return float(re.findall(r"[-+]?\d+(?:\.\d+)?", cell.replace(MINUS, "-"))[0])


def nums(cell):
    return [float(x) for x in re.findall(r"[-+]?\d+(?:\.\d+)?", cell.replace(MINUS, "-"))]


def subsection(text, marker):
    """The lines under the first `###` heading that contains `marker`, up to the next heading."""
    lines = text.splitlines()
    start = next(i for i, ln in enumerate(lines) if ln.startswith("###") and marker in ln)
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("#")), len(lines))
    return lines[start:end]


def table(lines):
    """The first markdown table in `lines` as a list of {header: cell}."""
    start = next(i for i, ln in enumerate(lines) if ln.startswith("|"))
    rows = []
    for ln in lines[start:]:
        if not ln.startswith("|"):
            break
        rows.append([c.strip() for c in ln.strip().strip("|").split("|")])
    header, body = rows[0], rows[2:]
    return [dict(zip(header, r, strict=True)) for r in body]


def heading_lines(text):
    return [ln for ln in text.splitlines() if ln.startswith("## ")]


# ---- K110 label ----


def test_k110_stage_a_heading_should_name_the_fork_and_the_contamination(tmp_path):
    camp = a_campaign(tmp_path)
    text, _ = build(camp, prereg_a(tmp_path))
    assert HEADING in text.splitlines()


def test_k110_stage_a_should_give_the_creation_dates_under_the_heading(tmp_path):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))
    lines = text.splitlines()
    after = lines[lines.index(HEADING) + 1 : lines.index(HEADING) + 6]
    assert CREATED_LINE in after


def test_k110_the_dates_should_come_from_task_created_at(tmp_path):
    created = {t: f"2026-04-{k + 5:02d} 00:00:00" for k, t in enumerate(TASKS)}
    d = prereg_a(tmp_path, task_created_at=created)
    text, _ = build(a_campaign(tmp_path), d)
    assert "Tasks created 2026-04-05 to 2026-04-14." in text


def test_k110_stage_a_should_not_say_lite_or_the_official_grader(tmp_path):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))
    assert "SWE-bench Lite" not in text and "official grader" not in text


@pytest.mark.parametrize("stage", [None, "cb"])
def test_k110_without_a_stage_a_prereg_the_heading_should_be_plain_suite_a(tmp_path, stage):
    camp = a_campaign(tmp_path)
    prereg_dir = None
    if stage:
        prereg_dir = tmp_path / "prereg"
        prereg_dir.mkdir()
        (prereg_dir / "preregistration.toml").write_text(
            prereg_mod.dump_toml({"stage": "cb", "draft": False, "config": {}})
        )
    text, _ = build(camp, prereg_dir)
    assert "## Suite A" in text.splitlines()
    assert "SWE-bench Lite" not in text and "official grader" not in text
    assert "possibly contaminated" not in text
    assert "Tasks created" not in text


# ---- K111 H1 ----


def test_k111_h1_should_be_labelled_pre_registered_under_a_stage_a_prereg(tmp_path):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))
    heading = subsection(text, "H1")[0]
    assert "pre-registered" in heading and "exploratory" not in heading
    assert "Pairwise — exploratory" not in text


def test_k111_without_a_prereg_the_pairwise_heading_should_stay(tmp_path):
    text, _ = build(a_campaign(tmp_path), None)
    assert "### Pairwise" in text
    assert "H1" not in "".join(ln for ln in text.splitlines() if ln.startswith("###"))


def test_k111_the_claim_text_should_use_the_number_of_tasks_run(tmp_path):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))  # every arm solves everything
    h1 = "\n".join(subsection(text, "H1"))
    assert "no detectable difference at n=10" in h1


def test_k111_the_number_of_tasks_run_should_follow_the_campaign(tmp_path):
    tasks = TASKS[:6]
    text, _ = build(a_campaign(tmp_path, tasks=tasks), prereg_a(tmp_path, tasks=tasks))
    assert "no detectable difference at n=6" in "\n".join(subsection(text, "H1"))


# ---- K112 H2 ----


def h2_rows(text):
    return {(r["comparison"], r["metric"]): r for r in table(subsection(text, "H2"))}


def test_k112_h2_should_have_one_row_per_contrast_and_metric(tmp_path):
    text, _ = build(a_campaign(tmp_path), prereg_a(tmp_path))
    heading = subsection(text, "H2")[0]
    assert "pre-registered" in heading
    # K144: the four family tests, plus the outside contrast whose arms are present (pi, hermes)
    assert set(h2_rows(text)) == {
        (f"hone-inject {MINUS} pi", "total tokens"),
        (f"hone-inject {MINUS} pi", "wall seconds"),
        (f"hone-inject {MINUS} hermes", "total tokens"),
        (f"hone-inject {MINUS} hermes", "wall seconds"),
        (f"pi {MINUS} hermes", "total tokens"),
        (f"pi {MINUS} hermes", "wall seconds"),
    }
    row = next(iter(h2_rows(text).values()))
    for col in ("mean paired difference per task", "95% CI (paired bootstrap)", "p",
                "p (Holm-adjusted)", "claimed", "tasks used", "tasks left out"):  # fmt: skip
        assert col in row


def test_k112_h2_should_report_the_paired_mean_ci_and_p_on_obvious_data(tmp_path):
    tok = {"hone-inject": 1000, "pi": 1500, "hermes": 2000}
    wall = {"hone-inject": 100.0, "pi": 120.0, "hermes": 150.0}
    camp = a_campaign(tmp_path, tokens=lambda a, t, r: tok[a], wall=lambda a, t, r: wall[a])
    text, _ = build(camp, prereg_a(tmp_path))
    rows = h2_rows(text)
    pi_tok = rows[(f"hone-inject {MINUS} pi", "total tokens")]
    assert num(pi_tok["mean paired difference per task"]) == pytest.approx(-500)
    assert nums(pi_tok["95% CI (paired bootstrap)"]) == pytest.approx([-500, -500])
    assert num(pi_tok["p"]) == pytest.approx(0.0)
    assert num(pi_tok["p (Holm-adjusted)"]) == pytest.approx(0.0)
    assert pi_tok["claimed"] == "yes"
    assert num(
        rows[(f"hone-inject {MINUS} hermes", "total tokens")]["mean paired difference per task"]
    ) == pytest.approx(-1000)
    assert num(
        rows[(f"hone-inject {MINUS} pi", "wall seconds")]["mean paired difference per task"]
    ) == pytest.approx(-20)
    assert num(
        rows[(f"hone-inject {MINUS} hermes", "wall seconds")]["mean paired difference per task"]
    ) == pytest.approx(-50)


def test_k112_a_difference_that_does_not_reject_should_not_be_claimed(tmp_path):
    wobble = lambda t: 100 if int(t[1:]) % 2 == 0 else -100  # noqa: E731
    camp = a_campaign(
        tmp_path, tokens=lambda a, t, r: 1000 + (wobble(t) if a == "hone-inject" else 0)
    )
    text, _ = build(camp, prereg_a(tmp_path))
    row = h2_rows(text)[(f"hone-inject {MINUS} pi", "total tokens")]
    assert num(row["mean paired difference per task"]) == pytest.approx(0)
    assert row["claimed"] == "no" and num(row["p (Holm-adjusted)"]) > 0.05
    assert "no detectable difference at n=10" in "\n".join(subsection(text, "H2"))


def test_k112_holm_should_run_across_the_four_h2_tests(tmp_path):
    # Every test has p = 0.019: Holm over 4 tests needs p <= 0.0125 (none claimed); a Holm over
    # the two contrasts of one metric would need 0.025 and claim them.
    d = [3, -3, 5, 0, 4, 4, 5, 0, 2, 0]
    base_t, base_w = 5000, 200.0
    idx = {t: k for k, t in enumerate(TASKS)}
    hone_t = {t: base_t for t in TASKS}
    other_t = {t: base_t - 100 * d[idx[t]] for t in TASKS}
    hone_w = {t: base_w for t in TASKS}
    other_w = {t: base_w - d[idx[t]] for t in TASKS}
    camp = a_campaign(
        tmp_path,
        tokens=lambda a, t, r: hone_t[t] if a == "hone-inject" else other_t[t],
        wall=lambda a, t, r: hone_w[t] if a == "hone-inject" else other_w[t],
    )
    text, _ = build(camp, prereg_a(tmp_path))
    expected = bootstrap_pvalue(hone_t, other_t)
    assert 0.0125 < expected < 0.025
    rows = h2_rows(text)
    family = [r for (c, _), r in rows.items() if c.startswith(f"hone-inject {MINUS} ")]
    assert len(rows) == 6 and len(family) == 4
    for row in family:
        assert num(row["p"]) == pytest.approx(expected, abs=0.0006)
        assert num(row["p (Holm-adjusted)"]) == pytest.approx(0.076, abs=0.0025)  # 4 * 0.019
        assert row["claimed"] == "no"


def test_k112_the_suite_a_table_should_give_tokens_per_resolved_task(tmp_path):
    tokens = {"t0": 1000, "t1": 2000, "t2": 3000, "t3": 4000}
    tasks = list(tokens)
    camp = a_campaign(
        tmp_path,
        arms=("pi", "hone-inject"),
        tasks=tasks,
        repeats=(0,),
        tokens=lambda a, t, r: tokens[t],
        resolved=lambda a, t, r: t in ("t0", "t1"),
    )
    text, _ = build(camp, prereg_a(tmp_path, tasks=tasks, repeats="0"))
    suite_table = table(subsection_after(text, HEADING))
    by_arm = {r["arm"]: r for r in suite_table}
    assert num(by_arm["pi"]["tokens per resolved run"]) == pytest.approx(5000)  # 10000 / 2
    assert num(by_arm["hone-inject"]["tokens per resolved run"]) == pytest.approx(5000)


def subsection_after(text, heading):
    lines = text.splitlines()
    start = lines.index(heading)
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("###")), len(lines))
    return lines[start:end]


def test_k112_the_stage_cb_report_should_have_no_h2_table(tmp_path):
    camp = a_campaign(tmp_path)
    d = tmp_path / "prereg"
    d.mkdir()
    (d / "preregistration.toml").write_text(
        prereg_mod.dump_toml({"stage": "cb", "draft": False, "config": {}})
    )
    text, _ = build(camp, d)
    assert "H2" not in "".join(ln for ln in text.splitlines() if ln.startswith("#"))


# ---- K141 the tasks left out ----


UNJUDGED = {"grader_infra", "ambiguous", "missing", "grader_infra_final", "ungraded", "stale"}


def graded(tmp_path, statuses, **kw):
    """A campaign where `statuses` ({(arm, task, repeat): grade status}) are set; every other
    run is resolved. Unjudged statuses have a blank `resolved`, the others 0 (except resolved)."""

    def status(arm, task, rep):
        return statuses.get((arm, task, rep))

    def resolved(arm, task, rep):
        st = statuses.get((arm, task, rep))
        if st is None or st == "resolved":
            return True
        return None if st in UNJUDGED else False

    return a_campaign(tmp_path, status=status, resolved=resolved, **kw)


def with_unjudged_pi_t9(tmp_path):
    return graded(tmp_path, {("pi", "t9", r): "grader_infra_final" for r in (0, 1)})


def test_k141_h1_should_give_the_tasks_used_and_left_out_per_contrast(tmp_path):
    text, _ = build(with_unjudged_pi_t9(tmp_path), prereg_a(tmp_path))
    rows = {r["comparison"]: r for r in table(subsection(text, "H1"))}
    pi = rows[f"hone-inject {MINUS} pi"]
    hermes = rows[f"hone-inject {MINUS} hermes"]
    assert (num(pi["tasks used"]), num(pi["tasks left out"])) == (9, 1)
    assert (num(hermes["tasks used"]), num(hermes["tasks left out"])) == (10, 0)


def test_k141_h2_should_give_the_same_counts_per_contrast(tmp_path):
    text, _ = build(with_unjudged_pi_t9(tmp_path), prereg_a(tmp_path))
    rows = h2_rows(text)
    for metric in ("total tokens", "wall seconds"):
        pi = rows[(f"hone-inject {MINUS} pi", metric)]
        hermes = rows[(f"hone-inject {MINUS} hermes", metric)]
        assert (num(pi["tasks used"]), num(pi["tasks left out"])) == (9, 1)
        assert (num(hermes["tasks used"]), num(hermes["tasks left out"])) == (10, 0)


def test_k141_a_run_that_is_unjudged_in_one_repeat_only_should_keep_the_task(tmp_path):
    camp = graded(tmp_path, {("pi", "t9", 1): "grader_infra_final"})
    text, _ = build(camp, prereg_a(tmp_path))
    pi = next(r for r in table(subsection(text, "H1")) if r["comparison"].endswith(f"{MINUS} pi"))
    assert (num(pi["tasks used"]), num(pi["tasks left out"])) == (10, 0)


# ---- K142 pending grades ----


def pending_campaign(tmp_path, status):
    return graded(tmp_path, {("pi", "t3", 0): status})


@pytest.mark.parametrize("status", ["ungraded", "stale", "grader_infra", "ambiguous", "missing"])
def test_k142_a_pending_grade_should_be_a_conformance_mismatch_listed_by_run(tmp_path, status):
    camp = pending_campaign(tmp_path, status)
    _, conformance = build(camp, prereg_a(tmp_path))
    found = [m for m in conformance["mismatches"] if m["field"] == "grade_status"]
    assert [m["run_id"] for m in found] == ["a.pi.s0.t3"]
    assert found[0]["found"] == status


def test_k142_the_report_should_exit_with_the_conformance_code_but_still_be_written(tmp_path):
    camp = pending_campaign(tmp_path, "ambiguous")
    rc = run_report(camp, prereg_a(tmp_path))
    assert rc == report.CONFORMANCE_EXIT
    assert (camp / "report.md").exists()


@pytest.mark.parametrize(
    "status",
    ["resolved", "unresolved", "empty_patch", "apply_failed", "error", "ambiguous_repeated"],
)
def test_k142_a_judged_run_should_not_be_a_mismatch(tmp_path, status):
    camp = graded(tmp_path, {("pi", "t3", 0): status})
    _, conformance = build(camp, prereg_a(tmp_path))
    assert conformance["mismatches"] == []


def test_k142_grader_infra_final_should_not_be_a_mismatch_but_listed_as_unjudged(tmp_path):
    text, conformance = build(with_unjudged_pi_t9(tmp_path), prereg_a(tmp_path))
    assert conformance["mismatches"] == []
    assert "unjudged" in text.lower()
    lines = [ln for ln in text.splitlines() if "grader_infra_final" in ln]
    joined = "\n".join(lines)
    assert "a.pi.s0.t9" in joined and "a.pi.s1.t9" in joined


def test_k142_ambiguous_repeated_runs_should_be_listed_in_the_report(tmp_path):
    statuses = {("hermes", "t4", r): "ambiguous_repeated" for r in (0, 1)}
    text, _ = build(graded(tmp_path, statuses), prereg_a(tmp_path))
    listed = "\n".join(ln for ln in text.splitlines() if "ambiguous_repeated" in ln)
    assert "a.hermes.s0.t4" in listed and "a.hermes.s1.t4" in listed


def test_k142_without_a_stage_a_prereg_pending_grades_are_not_mismatches(tmp_path):
    camp = pending_campaign(tmp_path, "ambiguous")
    _, conformance = build(camp, None)
    assert conformance["mismatches"] == [] and conformance["made"] is False


# ---- K113 conformance for stage a ----


def test_k113_a_consistent_campaign_should_have_no_mismatch(tmp_path):
    _, conformance = build(a_campaign(tmp_path), prereg_a(tmp_path))
    assert conformance["mismatches"] == []
    assert run_report(tmp_path / "camp", tmp_path / "prereg") == 0


def test_k113_a_task_digest_that_differs_from_run_digest_should_be_a_mismatch(tmp_path):
    wrong = sampling.manifest_digest(TASKS[:9])
    _, conformance = build(a_campaign(tmp_path), prereg_a(tmp_path, run_digest=wrong))
    (m,) = [m for m in conformance["mismatches"] if m["field"] == "task_digest"]
    assert m["expected"] == wrong
    assert m["found"] == sampling.manifest_digest(TASKS)


def test_k113_run_digest_should_be_compared_not_digest(tmp_path):
    # `digest` is the drawn list (more tasks); `run_digest` is what ran.
    drawn = sampling.manifest_digest([*TASKS, "t10"])
    _, conformance = build(a_campaign(tmp_path), prereg_a(tmp_path, digest=drawn))
    assert [m for m in conformance["mismatches"] if m["field"] == "task_digest"] == []


@pytest.mark.parametrize(("digest_ok", "n_mismatch"), [(True, 0), (False, 1)])
def test_k113_without_run_digest_the_digest_should_be_compared(tmp_path, digest_ok, n_mismatch):
    digest = sampling.manifest_digest(TASKS if digest_ok else TASKS[:9])
    d = prereg_a(tmp_path, digest=digest, run_digest=None)
    _, conformance = build(a_campaign(tmp_path), d)
    assert len([m for m in conformance["mismatches"] if m["field"] == "task_digest"]) == n_mismatch


def test_k113_a_run_image_that_differs_from_task_images_should_be_a_mismatch(tmp_path):
    def image(task):
        return "swerebench/other:latest@sha256:" + "0" * 64 if task == "t5" else image_of(task)

    camp = a_campaign(tmp_path, arms=("pi",), image=image, repeats=(0,))
    d = prereg_a(tmp_path, repeats="0")
    _, conformance = build(camp, d)
    (m,) = [m for m in conformance["mismatches"] if m["field"] == "image"]
    assert m["run_id"] == "a.pi.s0.t5"
    assert m["expected"] == image_of("t5") and m["found"].startswith("swerebench/other")


def test_k113_an_arms_repeats_that_differ_from_hc_a_repeats_should_be_a_mismatch(tmp_path):
    skip = {("pi", t, 1) for t in TASKS}  # pi has repeat 0 only
    extra = {("hermes", t, 2) for t in ()}  # (hermes keeps 0 and 1)
    camp = a_campaign(tmp_path, skip=skip | extra)
    _, conformance = build(camp, prereg_a(tmp_path))
    (m,) = [m for m in conformance["mismatches"] if m["field"].startswith("repeats")]
    assert m["field"] == "repeats.pi"
    assert m["expected"] == [0, 1] and m["found"] == [0]


def test_k113_an_extra_repeat_should_be_a_mismatch_too(tmp_path):
    camp = a_campaign(tmp_path, repeats=(0, 1, 2))
    _, conformance = build(camp, prereg_a(tmp_path))
    fields = sorted(
        m["field"] for m in conformance["mismatches"] if m["field"].startswith("repeats")
    )
    assert fields == ["repeats.hermes", "repeats.hone-inject", "repeats.pi"]


def test_k113_the_repeats_should_come_from_the_prereg_config(tmp_path):
    camp = a_campaign(tmp_path, repeats=(0,))
    _, ok = build(camp, prereg_a(tmp_path, repeats="0"))
    assert ok["mismatches"] == []
    _, bad = build(camp, prereg_a(tmp_path, repeats="0,1"))
    # K152: the repeat-1 runs the file expects are missing too, as `run` mismatches
    assert {m["field"] for m in bad["mismatches"]} == {f"repeats.{a}" for a in ARMS} | {"run"}


# ---- K143 apply failure from the harness's own line ----

REAL_APPLY = (
    ">>>>> Patch Apply Failed:\nerror: patch failed: f.py:1\nerror: f.py: patch does not apply"
)
INFRA = "docker.errors.APIError: Error creating container"
TS = "2026-11-20 23:27:23,136"


def apply_log(iid, *, level="info", error=None, patch="diff --git a/f.py b/f.py\n+pass\n"):
    ok = [("info", f"Git diff before:\n{patch}")]
    msgs = [*ok, (level, REAL_APPLY)]
    if error is not None:
        msgs.append(
            (
                "error",
                f"Error in evaluating model for {iid}: {error}\nTraceback (most recent call last):",
            )
        )
    return fork_log(*msgs)


def test_k143_the_harness_line_should_give_apply_failed(tmp_path, monkeypatch):
    log = apply_log("a__a-1", error=f"a__a-1: Patch Apply Failed; {INFRA}")
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "apply_failed")


@pytest.mark.parametrize(
    "patch",
    [
        "diff --git a/f.py b/f.py\n+>>>>> Patch Apply Failed\n",
        f"diff --git a/f.py b/f.py\n+{TS} - INFO - >>>>> Patch Apply Failed\n",
        f"diff --git a/f.py b/f.py\n {TS} - INFO - >>>>> Patch Apply Failed\n",
    ],
)
def test_k143_the_marker_in_the_agents_diff_should_not_count(tmp_path, monkeypatch, patch):
    log = run_log("a__a-1", INFRA, patch=patch)
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")  # classified, not apply_failed


def test_k143_the_marker_in_the_diff_should_leave_an_unclassified_error_as_error(
    tmp_path, monkeypatch
):
    patch = "diff --git a/f.py b/f.py\n+# >>>>> Patch Apply Failed\n"
    log = run_log("a__a-1", "AssertionError: tests failed", patch=patch)
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"] == ("0", "error")


@pytest.mark.parametrize("level", ["error", "warning"])
def test_k143_only_an_info_line_should_count(tmp_path, monkeypatch, level):
    log = apply_log("a__a-1", level=level, error=INFRA)
    _, camp, _ = run_fork(tmp_path, monkeypatch, [step("a__a-1", log=log)])
    assert outcomes(camp)["a__a-1"][1] != "apply_failed"


def test_k143_the_marker_in_test_output_should_not_count(tmp_path, monkeypatch):
    s = step("a__a-1", INFRA, test_output=f"{TS} - INFO - >>>>> Patch Apply Failed\n")
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")


def test_k143_the_marker_in_another_log_file_should_not_count(tmp_path, monkeypatch):
    s = step("a__a-1", INFRA)
    s["logs"]["a__a-1"]["build_image.log"] = fork_log(("info", REAL_APPLY))
    _, camp, _ = run_fork(tmp_path, monkeypatch, [s])
    assert outcomes(camp)["a__a-1"] == ("", "grader_infra")
