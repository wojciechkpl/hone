"""Spec section 11.3 and 11.5, K31-K39 (second review). Interfaces: see test_campaign1_report.py."""

import asyncio
import json
import subprocess
import sys

import pytest
from fakebin import make_fake_bin, make_rig, run_script
from fakes import FakeUpstream, proxy_stack
from test_campaign1_prereg import pin, run_cb
from test_campaign1_report import (
    MINUS,
    PREREG,
    SCEN,
    TASKS,
    all_c,
    b_run,
    b_runs,
    build_campaign,
    c_run,
    c_runs,
    contrast,
    good_sampling,
    rec,
    run_of,
    run_report,
    section,
    write_prereg,
    write_run,
)

from hcbench import prereg as prereg_mod
from hcbench import report

CB = {**PREREG, "stage": "cb"}
EARLY = "2026-11-17T09:00:00Z"
START = "2026-11-17T10:00:00Z"


# ---- K32 two treatments ------------------------------------------------------------------


def test_k32_a_contrast_that_rejects_only_when_unjudged_runs_are_counted_is_not_claimed():
    # The reviewer's probe: every pi run is unjudged. Counted, pi escapes everywhere and
    # pi-guarded - pi rejects; dropped, pi has no judged run, so nothing can be claimed.
    pi = c_runs("pi", dict.fromkeys(SCEN, False))
    for r in pi:
        r["outcome"] = None
    rows = pi + all_c(
        {"pi-guarded": False, "hone-inject": True, "hone-guarded": False, "hermes": True,
         "hermes-guarded": False}
    )  # fmt: skip
    res = report.stage1_analysis(rows)
    row = contrast(res, "C-guard", "pi-guarded", "pi")
    assert row["counted"]["estimate"] == pytest.approx(-1.0)
    assert row["counted"]["reject"] is True
    assert row["dropped"]["n_units"] == 0
    assert row["dropped"]["reject"] is False
    assert row["reject"] is False
    # the unaffected contrasts still reject under both treatments
    for other in (("hone-guarded", "hone-inject"), ("hermes-guarded", "hermes")):
        assert contrast(res, "C-guard", *other)["reject"] is True
    assert sorted(res["unjudged"]["escaped"]) == sorted(r["run_id"] for r in pi)


def test_k32_dropped_treatment_leaves_out_units_with_no_judged_run_in_one_arm():
    guarded = c_runs("pi-guarded", {"s1": True, "s2": False})  # s1: escaped (judged)
    pi = c_runs("pi", {"s1": False, "s2": True})
    for r in pi:
        if r["task_id"] == "s1":
            r["outcome"] = None  # counted: escaped
    row = contrast(report.stage1_analysis(guarded + pi), "C-guard", "pi-guarded", "pi")
    assert row["counted"]["n_units"] == 2
    assert row["counted"]["estimate"] == pytest.approx(-0.5)  # s1: 1-1, s2: 0-1
    assert row["dropped"]["n_units"] == 1
    assert row["dropped"]["estimate"] == pytest.approx(-1.0)  # only s2


def test_k32_rates_differ_between_treatments_when_runs_are_unjudged():
    rows = c_runs("pi", {"s1": True, "s2": False})
    rows[3]["outcome"] = None  # one s2 run unjudged
    res = report.stage1_analysis(rows)
    assert res["escape_rate"]["counted"]["pi"] == pytest.approx((1 + 1 / 3) / 2)
    assert res["escape_rate"]["dropped"]["pi"] == pytest.approx((1 + 0) / 2)


def test_k32_arm_with_no_judged_run_has_no_dropped_rate():
    rows = c_runs("pi", {"s1": False})
    for r in rows:
        r["outcome"] = None
    res = report.stage1_analysis(rows)
    assert res["escape_rate"]["counted"]["pi"] == 1.0
    assert res["escape_rate"]["dropped"].get("pi") is None


def test_k32_report_shows_both_treatments(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, c_run("pi", 0, escaped=True))
    write_run(camp, c_run("pi-guarded", 0))
    run_report(camp, write_prereg(tmp_path, CB))
    text = section((camp / "report.md").read_text(), "Stage 1 analysis").lower()
    assert "counted" in text
    assert "dropped" in text


# ---- K33 contrasts left out --------------------------------------------------------------


def test_k33_left_out_contrasts_are_returned_per_family():
    rows = (
        c_runs("pi", dict.fromkeys(SCEN, True))
        + c_runs("pi-guarded", dict.fromkeys(SCEN, False))
        + b_runs("pi", dict.fromkeys(TASKS, True))
        + b_runs("pi-guarded", dict.fromkeys(TASKS, True))
    )
    left = report.stage1_analysis(rows)["left_out"]
    guard = [f"hone-guarded {MINUS} hone-inject", f"hermes-guarded {MINUS} hermes"]
    assert left == {
        "C-guard": guard,
        "C-cross": [
            f"hone-guarded {MINUS} pi-guarded",
            f"hone-guarded {MINUS} hermes-guarded",
        ],
        "B-guard": guard,
    }


def test_k33_nothing_left_out_gives_empty_lists():
    arms = ["pi", "pi-guarded", "hone-inject", "hone-guarded", "hermes", "hermes-guarded"]
    rows = []
    for a in arms:
        rows += c_runs(a, dict.fromkeys(SCEN, False)) + b_runs(a, dict.fromkeys(TASKS, True))
    left = report.stage1_analysis(rows)["left_out"]
    assert left == {"C-guard": [], "C-cross": [], "B-guard": []}


def test_k33_report_names_the_left_out_contrasts(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, c_run("pi", 0, escaped=True))
    write_run(camp, c_run("pi-guarded", 0))
    run_report(camp, write_prereg(tmp_path, CB))
    text = section((camp / "report.md").read_text(), "Stage 1 analysis")
    assert "left out" in text.lower()
    assert f"hone-guarded {MINUS} hone-inject" in text
    assert f"hermes-guarded {MINUS} hermes" in text


# ---- K35 text matches code ---------------------------------------------------------------


def test_k35_report_without_a_preregistration_has_no_stage_1_section(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, c_run("hone-guarded", 0))
    write_run(camp, b_run("hone-guarded", 0))
    assert run_report(camp) == 0
    assert "Stage 1 analysis" not in (camp / "report.md").read_text()


def test_k35_report_with_a_stage_a_preregistration_has_no_stage_1_section(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, c_run("hone-guarded", 0))
    run_report(camp, write_prereg(tmp_path, {**PREREG, "stage": "a"}))
    assert "Stage 1 analysis" not in (camp / "report.md").read_text()


def test_k35_report_with_a_stage_cb_preregistration_has_the_stage_1_section(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, c_run("hone-guarded", 0))
    run_report(camp, write_prereg(tmp_path, CB))
    assert "Stage 1 analysis" in (camp / "report.md").read_text()


def older_b_campaign(tmp_path):
    camp = build_campaign(tmp_path)
    # Seeds are NOT base + repeat here, so the smallest seed is repeat 1: selecting "repeat 0"
    # on the seed would give the opposite discordant counts.
    for arm, flags in (("hone-inject", (True, False)), ("pi", (False, True))):
        for repeat, seed in ((0, 900), (1, 100)):
            for task in ("t1", "t2", "t3"):
                run = b_run(arm, repeat, task, flags[repeat], seed=seed)
                write_run(camp, run)
    return camp


def test_k35_older_suite_b_table_is_headed_exploratory_with_a_stage_cb_preregistration(tmp_path):
    camp = older_b_campaign(tmp_path)
    run_report(camp, write_prereg(tmp_path, CB))
    text = section((camp / "report.md").read_text(), "Suite B")
    assert "exploratory, not pre-registered" in text


def test_k35_older_table_selects_repeat_0_on_the_repeat_field(tmp_path):
    camp = older_b_campaign(tmp_path)
    run_report(camp, write_prereg(tmp_path, CB))
    text = section((camp / "report.md").read_text(), "Suite B")
    line = next(ln for ln in text.splitlines() if ln.startswith(f"| hone-inject {MINUS} pi"))
    assert "| 3/0 |" in line  # repeat 0: hone-inject passes the three tasks, pi none


# ---- K36 status of the pre-registration --------------------------------------------------

CLEAN = {**CB, "draft": False, "created_at": EARLY, "repo": {"head": "abc", "dirty": False}}


def status_run(started=START):
    return {**run_of("b.pi.s0.t1", 0, rec(good_sampling(0))), "started_at": started}


def fields(res):
    return sorted(m["field"] for m in res["mismatches"])


def test_k36_clean_earlier_preregistration_gives_no_mismatch():
    assert report.check_conformance([status_run()], CLEAN)["mismatches"] == []


@pytest.mark.parametrize(
    ("over", "field", "expected", "found"),
    [
        ({"draft": True}, "draft", False, True),
        ({"repo": {"head": "abc", "dirty": True}}, "repo.dirty", False, True),
    ],
)
def test_k36_draft_or_dirty_preregistration_is_a_mismatch(over, field, expected, found):
    res = report.check_conformance([status_run()], {**CLEAN, **over})
    (m,) = [m for m in res["mismatches"] if m["field"] == field]
    assert (m["expected"], m["found"]) == (expected, found)
    assert fields(res) == [field]


def test_k36_created_after_the_first_run_started_is_a_mismatch():
    pre = {**CLEAN, "created_at": "2026-11-17T10:00:01Z"}
    assert fields(report.check_conformance([status_run()], pre)) == ["created_at"]


def test_k36_created_at_is_compared_with_the_earliest_run_start():
    runs = [status_run("2026-11-17T12:00:00Z"), status_run("2026-11-17T09:30:00Z")]
    runs[1]["run_id"] = "b.pi.s0.t2"
    pre = {**CLEAN, "created_at": "2026-11-17T09:45:00Z"}
    assert fields(report.check_conformance(runs, pre)) == ["created_at"]


def test_k36_created_at_equal_to_the_first_start_is_not_a_mismatch():
    pre = {**CLEAN, "created_at": START}
    assert report.check_conformance([status_run()], pre)["mismatches"] == []


@pytest.mark.parametrize(
    ("over", "word"),
    [
        ({"draft": True}, "draft"),
        ({"repo": {"head": "abc", "dirty": True}}, "dirty"),
        ({"created_at": "2099-01-01T00:00:00Z"}, "created_at"),
    ],
)
def test_k36_report_exits_non_zero_after_writing_for_a_bad_status(tmp_path, over, word):
    camp = build_campaign(tmp_path)
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    rc = run_report(camp, write_prereg(tmp_path, {**CLEAN, **over}))
    assert rc != 0
    assert word in section((camp / "report.md").read_text(), "Conformance")


def test_k36_report_with_a_clean_earlier_preregistration_exits_zero(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    assert run_report(camp, write_prereg(tmp_path, CLEAN)) == 0


# ---- K37 bundle --------------------------------------------------------------------------

BUNDLE = {"hermes_ref": "v2026.9.24", "pi_version": "0.73.1"}


def test_k37_matrix_writes_the_bundle_manifest_into_the_campaign_file(tmp_path, monkeypatch):
    from hcbench import matrix

    bundle = tmp_path / "bundle"
    bundle.mkdir()
    (bundle / "manifest.json").write_text(json.dumps(BUNDLE))
    monkeypatch.setenv("HC_BUNDLE_DIR", str(bundle))

    def fake_execute(task, arm, seed, cfg, out_root, **kw):
        return {"run_id": f"b.{arm.name}.s{seed}.{task.task_id}", "exit_code": 0, "proxy": {}}

    monkeypatch.setattr(matrix, "execute_run", fake_execute)
    out = tmp_path / "out"
    rc = matrix.main(
        ["--suite", "b", "--arms", "pi", "--limit", "1", "--out", str(out),
         "--env-file", str(tmp_path / "none.env")]
    )  # fmt: skip
    assert rc == 0
    assert json.loads((out / "campaign.b.json").read_text())["bundle"] == BUNDLE


def test_k37_matrix_without_a_manifest_records_an_empty_bundle(tmp_path, monkeypatch):
    from hcbench import matrix

    monkeypatch.setenv("HC_BUNDLE_DIR", str(tmp_path / "nobundle"))
    monkeypatch.setattr(
        matrix,
        "execute_run",
        lambda task, arm, seed, cfg, out_root, **kw: {"run_id": "x", "exit_code": 0, "proxy": {}},
    )
    out = tmp_path / "out"
    matrix.main(
        ["--suite", "b", "--arms", "pi", "--limit", "1", "--out", str(out),
         "--env-file", str(tmp_path / "none.env")]
    )  # fmt: skip
    assert json.loads((out / "campaign.b.json").read_text())["bundle"] == {}


def test_k37_identical_bundle_gives_no_mismatch():
    pre = {**CLEAN, "bundle": BUNDLE}
    res = report.check_conformance([status_run()], pre, bundles={"b": dict(BUNDLE)})
    assert res["mismatches"] == []


def test_k37_each_differing_key_is_a_mismatch_named_bundle_dot_key():
    pre = {**CLEAN, "bundle": BUNDLE}
    changed = {**BUNDLE, "hermes_ref": "v2026.10.1", "pi_version": "0.74.0"}
    res = report.check_conformance([status_run()], pre, bundles={"b": changed})
    assert fields(res) == ["bundle.hermes_ref", "bundle.pi_version"]
    (m,) = [m for m in res["mismatches"] if m["field"] == "bundle.hermes_ref"]
    assert (m["expected"], m["found"]) == ("v2026.9.24", "v2026.10.1")


def test_k37_bundles_of_every_suite_are_checked():
    pre = {**CLEAN, "bundle": BUNDLE}
    bundles = {"b": dict(BUNDLE), "c": {**BUNDLE, "pi_version": "9"}}
    assert fields(report.check_conformance([status_run()], pre, bundles=bundles)) == [
        "bundle.pi_version"
    ]


def write_campaign_bundle(camp, bundle, suite="b"):
    (camp / f"campaign.{suite}.json").write_text(json.dumps({"bundle": bundle}))


def test_k37_report_exits_non_zero_for_a_differing_bundle_key(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    write_campaign_bundle(camp, {**BUNDLE, "hermes_ref": "other"})
    rc = run_report(camp, write_prereg(tmp_path, {**CLEAN, "bundle": BUNDLE}))
    assert rc != 0
    assert "bundle.hermes_ref" in section((camp / "report.md").read_text(), "Conformance")


def test_k37_report_with_an_identical_bundle_exits_zero(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    write_campaign_bundle(camp, dict(BUNDLE))
    assert run_report(camp, write_prereg(tmp_path, {**CLEAN, "bundle": BUNDLE})) == 0


# ---- K38 packaging after a failing report -------------------------------------------------


def test_k38_package_is_still_written_when_the_report_fails_and_the_code_is_kept(tmp_path):
    rig = make_rig(tmp_path)
    bindir, _ = make_fake_bin(tmp_path)
    camp = rig / "results" / "camp"
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0, temperature=0.1))])
    pre = rig / "prereg" / "camp"
    pre.mkdir(parents=True)
    (pre / "preregistration.toml").write_text(prereg_mod.dump_toml(CLEAN))
    direct = subprocess.run(
        [sys.executable, "-m", "hcbench.report", "--campaign", str(camp), "--prereg", str(pre),
         "--env-file", str(tmp_path / "none.env")],
        cwd=rig, capture_output=True, text=True, env={"PYTHONPATH": str(rig)}, check=False,
    )  # fmt: skip
    assert direct.returncode != 0
    r = run_script(rig, "30_report.sh", ["camp", "--package"], bindir, {})
    assert r.returncode == direct.returncode, r.stderr
    publish = camp / "publish"
    assert (publish / "trajectories.tar.gz").exists()
    assert (publish / "SHA256SUMS").exists()
    assert (publish / "report.md").exists()
    assert (publish / "prereg" / "preregistration.toml").exists()


def test_k38_package_after_a_passing_report_exits_zero(tmp_path):
    rig = make_rig(tmp_path)
    bindir, _ = make_fake_bin(tmp_path)
    camp = rig / "results" / "camp"
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    r = run_script(rig, "30_report.sh", ["camp", "--package"], bindir, {})
    assert r.returncode == 0, r.stderr
    assert (camp / "publish" / "trajectories.tar.gz").exists()


# ---- K39 small items ---------------------------------------------------------------------


def test_k39_check_hermes_bundle_also_looks_at_hc_arms_b():
    env = {
        "HC_ARMS_MAIN": "pi",
        "HC_ARMS_REDTEAM": "pi",
        "HC_ARMS_B": "pi,hermes",
        "HC_HERMES_REF": "v1",
        "HC_HERMES_SHA": "s" * 40,
    }
    with pytest.raises(SystemExit):
        prereg_mod.check_hermes_bundle(env, Path_missing())


def test_k39_check_hermes_bundle_accepts_a_consistent_manifest_for_arms_b(tmp_path):
    env = {
        "HC_ARMS_MAIN": "pi",
        "HC_ARMS_REDTEAM": "pi",
        "HC_ARMS_B": "pi,hermes",
        "HC_HERMES_REF": "v1",
        "HC_HERMES_SHA": "s" * 40,
    }
    manifest = tmp_path / "manifest.json"
    manifest.write_text(json.dumps({"hermes_ref": "v1", "hermes_sha": "s" * 40}))
    prereg_mod.check_hermes_bundle(env, manifest)


def Path_missing():
    from pathlib import Path

    return Path("/nonexistent/manifest.json")


@pytest.mark.parametrize(
    "value", ["2026-9-1", "2026/09/01", "tomorrow", "2026-13-01", "2026-02-30"]
)
def test_k39_min_created_must_be_a_yyyy_mm_dd_date(env, value):
    pin(env)
    env["mp"].setenv("HC_A_MIN_CREATED", value)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_A_MIN_CREATED" in str(exc.value)


@pytest.mark.parametrize("value", ["0", "-5", "6.5", "sixty"])
def test_k39_a_n_must_be_a_positive_integer(env, value):
    pin(env)
    env["mp"].setenv("HC_A_N", value)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_A_N" in str(exc.value)


def test_k39_valid_min_created_and_n_are_accepted(env):
    pin(env)
    env["mp"].setenv("HC_A_MIN_CREATED", "2026-09-01")
    env["mp"].setenv("HC_A_N", "60")
    rc, _ = run_cb(env)
    assert rc == 0


async def _call(tmp_path, body, overrides):
    up = FakeUpstream()
    async with proxy_stack(tmp_path, up, overrides) as st:
        await st.register("c.pi.s0.rt01", seed=1)
        status, _ = await st.chat(
            "c.pi.s0.rt01", {"messages": [{"role": "user", "content": "x"}], **body}
        )
        assert status == 200
        _, records = await st.records("c.pi.s0.rt01")
    return records[0]


def test_k39_response_format_is_outside_the_known_set(tmp_path):
    record = asyncio.run(_call(tmp_path, {"response_format": {"type": "json_object"}}, {}))
    assert record["extra_keys"] == ["response_format"]


def test_k39_record_lists_the_sorted_keys_the_proxy_removed(tmp_path):
    body = {"top_k": 5, "reasoning_effort": "high", "frequency_penalty": 1, "temperature": 0.1}
    record = asyncio.run(_call(tmp_path, body, {"temperature": 0.7}))
    assert record["dropped_keys"] == ["frequency_penalty", "reasoning_effort", "top_k"]


def test_k39_an_overridden_key_is_replaced_not_dropped(tmp_path):
    record = asyncio.run(
        _call(tmp_path, {"top_k": 5, "temperature": 0.1}, {"top_k": 20, "temperature": 0.7})
    )
    assert record["dropped_keys"] == []


def test_k39_max_completion_tokens_and_chat_template_kwargs_count_as_removed(tmp_path):
    body = {"max_completion_tokens": 9, "chat_template_kwargs": {"enable_thinking": True}}
    record = asyncio.run(_call(tmp_path, body, {"max_tokens": 100}))
    assert record["dropped_keys"] == ["chat_template_kwargs", "max_completion_tokens"]


def test_k39_record_without_removals_has_an_empty_dropped_keys_list(tmp_path):
    record = asyncio.run(_call(tmp_path, {}, {"temperature": 0.7}))
    assert record["dropped_keys"] == []
