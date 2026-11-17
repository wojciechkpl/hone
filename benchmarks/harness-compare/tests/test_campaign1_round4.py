"""Spec section 11.3 and 11.6, K40-K47 (third review). Interfaces: see test_campaign1_report.py.

Invocation lines (K43), appended by hcbench.matrix to invocations.jsonl, hold the earlier keys
plus: "bundle" (manifest dict, {} when none), "files" ({files_sha256 key: sha256} for each
hashed file that exists: arms_toml, suite_b, suite_c, suite_smoke, docker/mini.Dockerfile,
docker/redteam.Dockerfile, docker/redteam/plant.sh, docker/tools.Dockerfile), "head" (str) and
"dirty" (bool).
"""

import hashlib
import json
import os
import subprocess
import sys
import tomllib
from pathlib import Path

import pytest
from fakebin import (
    ROOT,
    SHA40,
    calls,
    docker_calls,
    harmful,
    make_fake_bin,
    make_rig,
    run_script,
)
from test_campaign1_prereg import load_cb, pin, run_cb
from test_campaign1_prereg_script import prereg_env
from test_campaign1_report import (
    MINUS,
    PREREG,
    SCEN,
    TASKS,
    b_run,
    b_runs,
    build_campaign,
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
from test_campaign1_round3 import CLEAN, START, fields, older_b_campaign, status_run

from hcbench import report
from hcbench.config import load_env_file

CB = {**PREREG, "stage": "cb"}


# ---- K40 worst-case bounds ---------------------------------------------------------------


def probe_rows():
    """The reviewer's probe: 25 scenarios x 3 repeats."""
    pg = {s: s in [f"s{i}" for i in range(1, 7)] for s in SCEN25}  # pi-guarded escapes s1-s6
    rows = c_runs("pi-guarded", pg) + c_runs("hone-guarded", dict.fromkeys(SCEN25, False))
    for r in rows:
        i = int(r["task_id"][1:])
        if r["arm"] == "pi-guarded" and 7 <= i <= 15 and r["repeat"] == 0:
            r["outcome"] = None  # one unjudged run in each of s7-s15
        if r["arm"] == "hone-guarded" and 16 <= i <= 24 and r["repeat"] == 0:
            r["outcome"] = None  # one unjudged run in each of s16-s24
    return rows


SCEN25 = [f"s{i}" for i in range(25)]


def test_k40_the_reviewers_probe_is_not_claimed():
    res = report.stage1_analysis(probe_rows())
    row = contrast(res, "C-cross", "hone-guarded", "pi-guarded")
    # against hone-guarded: its 9 unjudged runs count as escaped, pi-guarded's 9 as clean
    assert row["against_a"]["estimate"] == pytest.approx(-0.12)
    # against pi-guarded: the reverse
    assert row["against_b"]["estimate"] == pytest.approx(-0.36)
    assert row["against_a"]["n_units"] == row["against_b"]["n_units"] == 25
    assert row["favours"] == "hone-guarded"
    assert row["p"] == pytest.approx(0.2106, abs=0.01)  # the p-value under the worse bound
    assert row["p"] == row["against_a"]["p"]
    assert row["against_b"]["p"] < 0.001 + 1e-12
    assert row["reject"] is False


def test_k40_without_unjudged_runs_the_two_bounds_are_the_plain_contrast():
    flags = {s: s in [f"s{i}" for i in range(1, 7)] for s in SCEN25}
    rows = c_runs("pi", flags) + c_runs("pi-guarded", dict.fromkeys(SCEN25, False))
    row = contrast(report.stage1_analysis(rows), "C-guard", "pi-guarded", "pi")
    assert row["against_a"] == row["against_b"]
    assert row["against_a"]["estimate"] == pytest.approx(-0.24)
    assert row["against_a"]["n_units"] == 25
    assert row["favours"] == "pi-guarded"
    assert row["p"] == row["against_a"]["p"]
    assert row["reject"] is True  # p = 0.001 on these 25 units


def test_k40_bounds_that_favour_different_arms_give_p_one_and_no_claim():
    # both arms have one unjudged run in every scenario: against a the estimate is +0.5 (favours
    # b), against b it is -0.5 (favours a)
    rows = []
    for arm in ("pi-guarded", "pi"):
        for sc in SCEN:
            rows += c_runs(arm, {sc: False}, repeats=(0, 1))
    for r in rows:
        if r["repeat"] == 1:
            r["outcome"] = None
    row = contrast(report.stage1_analysis(rows), "C-guard", "pi-guarded", "pi")
    assert row["against_a"]["estimate"] == pytest.approx(0.5)
    assert row["against_b"]["estimate"] == pytest.approx(-0.5)
    assert row["favours"] is None and row["p"] == 1.0 and row["reject"] is False


def test_k40_lower_escape_rate_favours_the_arm_with_fewer_escapes():
    clean = report.stage1_analysis(
        c_runs("pi-guarded", dict.fromkeys(SCEN, False)) + c_runs("pi", dict.fromkeys(SCEN, True))
    )
    assert contrast(clean, "C-guard", "pi-guarded", "pi")["favours"] == "pi-guarded"
    flipped = report.stage1_analysis(
        c_runs("pi-guarded", dict.fromkeys(SCEN, True)) + c_runs("pi", dict.fromkeys(SCEN, False))
    )
    row = contrast(flipped, "C-guard", "pi-guarded", "pi")
    assert row["favours"] == "pi"
    assert row["against_a"]["estimate"] == pytest.approx(1.0)


def test_k40_higher_pass_rate_favours_the_arm_that_passes_more():
    good = report.stage1_analysis(
        b_runs("pi-guarded", dict.fromkeys(TASKS, True)) + b_runs("pi", dict.fromkeys(TASKS, False))
    )
    assert contrast(good, "B-guard", "pi-guarded", "pi")["favours"] == "pi-guarded"
    flipped = report.stage1_analysis(
        b_runs("pi-guarded", dict.fromkeys(TASKS, False)) + b_runs("pi", dict.fromkeys(TASKS, True))
    )
    row = contrast(flipped, "B-guard", "pi-guarded", "pi")
    assert row["favours"] == "pi"
    assert row["against_a"]["estimate"] == pytest.approx(-1.0)


def esc_rows(esc, unjudged_arm=None):
    flags = {s: s in [f"s{i}" for i in range(1, esc + 1)] for s in SCEN25}
    rows = c_runs("pi", flags) + c_runs("pi-guarded", dict.fromkeys(SCEN25, False))
    for r in rows:
        i = int(r["task_id"][1:])
        if unjudged_arm in (r["arm"], "both") and 7 <= i <= 15 and r["repeat"] == 0:
            r["outcome"] = None
    return rows


def guard(rows):
    return contrast(report.stage1_analysis(rows), "C-guard", "pi-guarded", "pi")


def test_k40_unjudged_runs_in_the_favoured_arm_revoke_a_claim():
    assert guard(esc_rows(6))["reject"] is True
    after = guard(esc_rows(6, "pi-guarded"))
    assert after["favours"] == "pi-guarded"
    assert after["reject"] is False
    assert after["p"] > guard(esc_rows(6))["p"]
    assert after["against_a"]["estimate"] == pytest.approx(-0.12)
    assert after["against_b"]["estimate"] == pytest.approx(-0.24)


def test_k40_unjudged_runs_in_the_other_arm_keep_a_claim_that_the_data_supports():
    base = guard(esc_rows(6))
    after = guard(esc_rows(6, "pi"))
    assert after["reject"] is True
    assert after["p"] == base["p"]
    assert after["against_b"]["estimate"] == pytest.approx(-0.36)


def test_k40_unjudged_runs_in_both_arms_revoke_the_claim():
    assert guard(esc_rows(6, "both"))["reject"] is False


@pytest.mark.parametrize("arm", [None, "pi-guarded", "pi", "both"])
def test_k40_unjudged_runs_never_turn_a_non_claim_into_a_claim(arm):
    assert guard(esc_rows(2))["reject"] is False
    assert guard(esc_rows(2, arm))["reject"] is False
    assert guard(esc_rows(2, arm))["p"] >= guard(esc_rows(2))["p"]


# ---- K42 dirty tree ----------------------------------------------------------------------


def git(rig, *args):
    return subprocess.run(
        ["git", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", *args],
        cwd=rig, capture_output=True, text=True, check=True,
    )  # fmt: skip


def git_rig(tmp_path):
    rig = make_rig(tmp_path)
    git(rig, "init", "-q")
    git(rig, "add", "-A")
    git(rig, "commit", "-q", "-m", "rig")
    return rig


def prereg_cli(rig, tmp_path, *extra, revision=SHA40):
    vals = load_env_file(
        rig / "config" / "bench.env.example", base={"HOME": "/h", "HC_ROOT": str(rig)}
    )
    env = {k: v for k, v in os.environ.items() if not k.startswith(("HC_", "GIT_"))}
    for k, v in vals.items():
        if "CHANGE-ME" in v:
            env[k] = revision if k == "HC_MODEL_REVISION" else "pinned-for-test"
    env.update(prereg_env(rig, tmp_path))
    env["PYTHONPATH"] = str(rig)
    out = rig / "prereg" / "t" / "preregistration.toml"
    r = subprocess.run(
        [sys.executable, "-m", "hcbench.prereg", "--stage", "cb", "--out", str(out),
         "--env-file", str(rig / "config" / "bench.env.example"), *extra],
        cwd=rig, capture_output=True, text=True, env=env, check=False,
    )  # fmt: skip
    return r, out


def test_k42_files_from_a_previous_draft_do_not_make_the_tree_dirty(tmp_path):
    rig = git_rig(tmp_path)
    (rig / "prereg" / "t").mkdir(parents=True)
    (rig / "prereg" / "t" / "DEVIATIONS.md").write_text("old\n")
    (rig / "prereg" / "t" / "preregistration.toml").write_text("draft = true\n")
    r, out = prereg_cli(rig, tmp_path)
    assert r.returncode == 0, r.stderr
    assert tomllib.loads(out.read_text())["repo"]["dirty"] is False


def test_k42_non_draft_is_refused_when_another_path_is_dirty_and_the_message_lists_it(tmp_path):
    rig = git_rig(tmp_path)
    (rig / "config" / "arms.toml").write_text(
        (rig / "config" / "arms.toml").read_text() + "\n# x\n"
    )
    (rig / "suites" / "new_file.toml").write_text("x = 1\n")
    r, out = prereg_cli(rig, tmp_path)
    assert r.returncode != 0
    assert "config/arms.toml" in r.stderr
    assert "suites/new_file.toml" in r.stderr
    assert not out.exists()


def test_k42_draft_still_writes_and_records_dirty_true(tmp_path):
    rig = git_rig(tmp_path)
    (rig / "config" / "arms.toml").write_text(
        (rig / "config" / "arms.toml").read_text() + "\n# x\n"
    )
    r, out = prereg_cli(rig, tmp_path, "--draft")
    assert r.returncode == 0, r.stderr
    assert tomllib.loads(out.read_text())["repo"]["dirty"] is True


def test_k42_rig_gitignore_ignores_dot_claude(tmp_path):
    rig = git_rig(tmp_path)
    (rig / ".claude").mkdir()
    (rig / ".claude" / "settings.json").write_text("{}")
    ignored = subprocess.run(
        ["git", "check-ignore", "-q", ".claude/settings.json"], cwd=rig, check=False
    )
    assert ignored.returncode == 0
    assert git(rig, "status", "--porcelain", "--", ".").stdout.strip() == ""


# ---- K43 evidence from every invocation ---------------------------------------------------

HASHED = {
    "arms_toml": "config/arms.toml",
    "suite_b": "suites/b_mini.toml",
    "suite_c": "suites/c_redteam.toml",
    "suite_smoke": "suites/smoke.toml",
    "docker/mini.Dockerfile": "docker/mini.Dockerfile",
    "docker/redteam.Dockerfile": "docker/redteam.Dockerfile",
    "docker/redteam/plant.sh": "docker/redteam/plant.sh",
    "docker/tools.Dockerfile": "docker/tools.Dockerfile",
}
BUNDLE = {"hermes_ref": "v2026.9.24", "pi_version": "0.73.1"}


def sha(rel):
    return hashlib.sha256((ROOT / rel).read_bytes()).hexdigest()


def test_k43_every_matrix_invocation_appends_bundle_files_head_and_dirty(tmp_path, monkeypatch):
    from hcbench import matrix

    bundle = tmp_path / "bundle"
    bundle.mkdir()
    (bundle / "manifest.json").write_text(json.dumps(BUNDLE))
    monkeypatch.setenv("HC_BUNDLE_DIR", str(bundle))
    monkeypatch.setattr(
        matrix,
        "execute_run",
        lambda task, arm, seed, cfg, out_root, **kw: {"run_id": "x", "exit_code": 0, "proxy": {}},
    )
    out = tmp_path / "out"
    argv = ["--suite", "b", "--arms", "pi", "--limit", "1", "--out", str(out),
            "--env-file", str(tmp_path / "none.env")]  # fmt: skip
    matrix.main(argv)
    matrix.main(argv)
    lines = [json.loads(ln) for ln in (out / "invocations.jsonl").read_text().splitlines()]
    assert len(lines) == 2
    for line in lines:
        assert line["bundle"] == BUNDLE
        assert line["files"] == {key: sha(rel) for key, rel in HASHED.items()}
        assert isinstance(line["head"], str)
        assert isinstance(line["dirty"], bool)


PRE43 = {
    **CLEAN,
    "bundle": BUNDLE,
    "files_sha256": {"arms_toml": "a" * 64, "suite_b": "b" * 64},
}


def invocation(**over):
    base = {
        "suite": "b",
        "started_at": START,
        "bundle": dict(BUNDLE),
        "files": {"arms_toml": "a" * 64, "suite_b": "b" * 64},
        "head": "abc",
        "dirty": False,
    }
    return {**base, **over}


def conf(invocations):
    return report.check_conformance([status_run()], PRE43, invocations=invocations)


def test_k43_identical_invocations_give_no_mismatch():
    assert conf([invocation(), invocation()])["mismatches"] == []


def test_k43_head_and_dirty_are_shown_not_compared():
    res = conf([invocation(head="something-else", dirty=True)])
    assert res["mismatches"] == []


def test_k43_a_later_invocation_with_a_different_bundle_names_that_invocation():
    other = {**BUNDLE, "pi_version": "9.9"}
    res = conf([invocation(), invocation(bundle=other)])
    (m,) = res["mismatches"]
    assert m["invocation"] == 1
    assert m["field"] == "bundle.pi_version"
    assert (m["expected"], m["found"]) == ("0.73.1", "9.9")


def test_k43_a_different_file_hash_names_the_invocation_and_the_files_key():
    res = conf([invocation(files={"arms_toml": "c" * 64, "suite_b": "b" * 64}), invocation()])
    (m,) = res["mismatches"]
    assert m["invocation"] == 0
    assert m["field"] == "files.arms_toml"
    assert (m["expected"], m["found"]) == ("a" * 64, "c" * 64)


def test_k43_every_invocation_is_compared_not_only_the_first():
    bad = invocation(files={"arms_toml": "c" * 64, "suite_b": "d" * 64})
    res = conf([invocation(), invocation(), bad])
    assert sorted((m["invocation"], m["field"]) for m in res["mismatches"]) == [
        (2, "files.arms_toml"),
        (2, "files.suite_b"),
    ]


def write_invocations(camp, lines):
    (camp / "invocations.jsonl").write_text("".join(json.dumps(ln) + "\n" for ln in lines))


def test_k43_report_exits_3_for_a_changed_arms_file_in_a_second_invocation(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    write_invocations(
        camp, [invocation(), invocation(files={"arms_toml": "0" * 64, "suite_b": "b" * 64})]
    )
    assert run_report(camp, write_prereg(tmp_path, PRE43)) == 3
    text = section((camp / "report.md").read_text(), "Conformance")
    assert "files.arms_toml" in text
    assert "invocation" in text.lower()


def test_k43_report_with_matching_invocations_exits_zero(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    write_invocations(camp, [invocation(), invocation(head="later", dirty=True)])
    assert run_report(camp, write_prereg(tmp_path, PRE43)) == 0


# ---- K44 null fields ---------------------------------------------------------------------


def test_k44_a_null_manifest_value_is_left_out_of_the_toml(env):
    manifest = {**env["manifest"], "pi_version": None, "node": None, "keep": "yes"}
    (env["tmp"] / "bundle" / "manifest.json").write_text(json.dumps(manifest))
    bundle = load_cb(env)["bundle"]
    assert "pi_version" not in bundle
    assert "node" not in bundle
    assert bundle["keep"] == "yes"
    assert bundle["hermes_ref"] == env["manifest"]["hermes_ref"]


@pytest.mark.parametrize("empty", [None, "", "__missing__"])
@pytest.mark.parametrize("registered", [None, "", "__missing__"])
def test_k44_missing_null_and_empty_string_compare_equal(registered, empty):
    pre_bundle = dict(BUNDLE)
    camp_bundle = dict(BUNDLE)
    if registered != "__missing__":
        pre_bundle["node"] = registered
    if empty != "__missing__":
        camp_bundle["node"] = empty
    pre = {**CLEAN, "bundle": pre_bundle}
    res = report.check_conformance([status_run()], pre, bundles={"b": camp_bundle})
    assert res["mismatches"] == []


def test_k44_a_missing_key_against_a_real_value_is_still_a_mismatch():
    pre = {**CLEAN, "bundle": dict(BUNDLE)}
    res = report.check_conformance([status_run()], pre, bundles={"b": {**BUNDLE, "extra": "value"}})
    assert fields(res) == ["bundle.extra"]


# ---- K45 exit codes ----------------------------------------------------------------------


def report_cli(rig, camp, pre=None):
    cmd = [sys.executable, "-m", "hcbench.report", "--campaign", str(camp),
           "--env-file", str(rig / "none.env")]  # fmt: skip
    if pre:
        cmd += ["--prereg", str(pre)]
    return subprocess.run(
        cmd, cwd=rig, capture_output=True, text=True, env={"PYTHONPATH": str(rig)}, check=False
    )


def test_k45_report_that_itself_fails_exits_with_a_code_other_than_0_and_3(tmp_path):
    rig = make_rig(tmp_path)
    camp = rig / "results" / "camp"
    d = camp / "b" / "pi" / "s0" / "t1"
    d.mkdir(parents=True)
    (d / "run.json").write_text("{ not json")
    r = report_cli(rig, camp)
    assert r.returncode not in (0, 3)


def test_k45_conformance_mismatch_exits_3_and_writes_the_report(tmp_path):
    rig = make_rig(tmp_path)
    camp = rig / "results" / "camp"
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0, temperature=0.1))])
    pre = rig / "prereg" / "camp"
    pre.mkdir(parents=True)
    (pre / "preregistration.toml").write_text(__import__("hcbench.prereg").prereg.dump_toml(CLEAN))
    r = report_cli(rig, camp, pre)
    assert r.returncode == 3
    assert (camp / "report.md").exists()


def test_k45_clean_report_exits_0(tmp_path):
    rig = make_rig(tmp_path)
    camp = rig / "results" / "camp"
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    assert report_cli(rig, camp).returncode == 0


def test_k45_package_is_written_after_exit_3_and_the_code_is_kept(tmp_path):
    rig = make_rig(tmp_path)
    bindir, _ = make_fake_bin(tmp_path)
    camp = rig / "results" / "camp"
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0, temperature=0.1))])
    pre = rig / "prereg" / "camp"
    pre.mkdir(parents=True)
    (pre / "preregistration.toml").write_text(__import__("hcbench.prereg").prereg.dump_toml(CLEAN))
    r = run_script(rig, "30_report.sh", ["camp", "--package"], bindir, {})
    assert r.returncode == 3, r.stderr
    assert (camp / "publish" / "trajectories.tar.gz").exists()


def test_k45_package_is_not_written_when_the_report_itself_fails(tmp_path):
    rig = make_rig(tmp_path)
    bindir, _ = make_fake_bin(tmp_path)
    camp = rig / "results" / "camp"
    d = camp / "b" / "pi" / "s0" / "t1"
    d.mkdir(parents=True)
    (d / "run.json").write_text("{ not json")
    direct = report_cli(rig, camp)
    r = run_script(rig, "30_report.sh", ["camp", "--package"], bindir, {})
    assert r.returncode == direct.returncode
    assert r.returncode not in (0, 3)
    assert not (camp / "publish").exists()


# ---- K46 labels --------------------------------------------------------------------------


def test_k46_suite_b_descriptives_are_not_under_the_exploratory_heading(tmp_path):
    camp = older_b_campaign(tmp_path)
    run_report(camp, write_prereg(tmp_path, CB))
    text = (camp / "report.md").read_text()
    lines = text.splitlines()
    marker = next(i for i, ln in enumerate(lines) if "exploratory, not pre-registered" in ln)
    end = next((i for i in range(marker + 1, len(lines)) if lines[i].startswith("#")), len(lines))
    under_marker = "\n".join(lines[marker:end])
    assert "median tokens" not in under_marker  # the per-arm tokens / wall time / tool calls
    assert "median tokens" in section(text, "Suite B")


# ---- K47 pinned revision ------------------------------------------------------------------

IMAGE = {"HC_VLLM_IMAGE": "vllm/vllm-openai:test", "HC_MODEL_HF": "org/model"}


@pytest.mark.parametrize(
    "revision", ["main", "abc123", "a" * 39, "a" * 41, "g" * 40, "v1.0", "refs/heads/main"]
)
def test_k47_vllm_stops_before_any_container_unless_the_revision_is_a_40_hex_commit(
    tmp_path, revision
):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(
        rig, "02_serve_model.sh", ["up"], bindir, {**IMAGE, "HC_MODEL_REVISION": revision}
    )
    assert r.returncode != 0
    assert "HC_MODEL_REVISION" in r.stderr
    assert harmful(log) == []


def test_k47_vllm_accepts_a_40_hex_commit(tmp_path):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(rig, "02_serve_model.sh", ["up"], bindir, {**IMAGE, "HC_MODEL_REVISION": SHA40})
    assert r.returncode == 0, r.stderr
    assert any(c[:1] == ["run"] for c in docker_calls(log))


@pytest.mark.parametrize("server", ["mock", "llamacpp"])
def test_k47_mock_and_llamacpp_servers_are_not_blocked(tmp_path, server):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    r = run_script(
        rig, "02_serve_model.sh", ["up"], bindir, {"HC_SERVER": server, "HC_MODEL_REVISION": "main"}
    )
    assert r.returncode == 0, r.stderr
    assert any(c[:1] == ["run"] for c in docker_calls(log))
    assert calls(log)


@pytest.mark.parametrize("revision", ["main", "abc123", "a" * 41])
def test_k47_non_draft_stage_cb_preregistration_needs_a_40_hex_revision(env, revision):
    pin(env)
    env["mp"].setenv("HC_MODEL_REVISION", revision)
    with pytest.raises(SystemExit) as exc:
        run_cb(env)
    assert "HC_MODEL_REVISION" in str(exc.value)


def test_k47_a_draft_preregistration_may_carry_any_revision(env):
    pin(env)
    env["mp"].setenv("HC_MODEL_REVISION", "main")
    rc, _ = run_cb(env, "--draft")
    assert rc == 0


def test_k47_a_40_hex_revision_is_accepted(env):
    pin(env)
    rc, out = run_cb(env)
    assert rc == 0
    assert tomllib.loads(out.read_text())["config"]["HC_MODEL_REVISION"] == SHA40


def test_k47_other_servers_do_not_need_a_pinned_revision(env):
    pin(env)
    env["mp"].setenv("HC_SERVER", "llamacpp")
    env["mp"].setenv("HC_MODEL_REVISION", "main")
    rc, _ = run_cb(env)
    assert rc == 0


def test_k47_helpers_are_importable():
    assert MINUS and run_of and c_runs and Path and b_runs and rec
