"""Spec section 11.3 and 11.7, K48-K52 (fourth review). Interfaces: see test_campaign1_report.py.

Added in round 5:

    check_conformance(runs, prereg, bundles=None, invocations=None, code=None, left_out=None)
        code:     {"changed": {head: [rig-relative path] | None},
                   "report":  {"head": str | None, "dirty_paths": [str]}}
                  `changed[h]` is what differs outside prereg/ between the pre-registration's
                  repo.head and h (None: git cannot resolve h). The caller (hcbench.report)
                  computes it with git for every distinct head it sees.
        left_out: stage1_analysis(...)["left_out"] ({family: [contrast label]}).
        Invocation lines now also hold "dirty_paths": [str] (prereg/ left out of both "dirty"
        and "dirty_paths") and "paths": {pre-registered key: path str} for the suite file and
        the arms file actually used ("suite_b"/"suite_c"/"suite_smoke" and "arms_toml"), whose
        sha256 is recorded in "files" under the same key.
        New mismatches (run_id None; "invocation" is the 0-based index or the string "report"):
          field "code.changed" found [paths]; "code.dirty" found [paths]; "code.head" found
          the head string; "arms.<family>" for a contrast left out although the
          pre-registered arms list (config HC_ARMS_REDTEAM for C, HC_ARMS_B for B) names both
          arms and the suite has runs (suite = first segment of a run_id); the contrast label
          appears in the mismatch.
"""

import json
import subprocess
import sys

from fakebin import ROOT, make_rig
from test_campaign1_prereg import pin
from test_campaign1_prereg_script import prereg_env
from test_campaign1_report import (
    MINUS,
    PREREG,
    b_run,
    build_campaign,
    c_run,
    good_sampling,
    rec,
    run_of,
    run_report,
    section,
    write_prereg,
    write_run,
)
from test_campaign1_round3 import CLEAN, START, fields
from test_campaign1_round4 import clean_git_env, git, git_rig, prereg_cli

from hcbench import prereg as prereg_mod
from hcbench import report

SIX = "pi,pi-guarded,hone-inject,hone-guarded,hermes,hermes-guarded"


# ---- K48 code state (pure) ---------------------------------------------------------------


def inv(**over):
    base = {
        "suite": "b",
        "started_at": START,
        "bundle": {},
        "files": {},
        "head": "h1",
        "dirty": False,
        "dirty_paths": [],
    }
    return {**base, **over}


CODE_OK = {
    "changed": {"h1": [], "h2": ["config/arms.toml"], "dead": None},
    "report": {"head": "h1", "dirty_paths": []},
}


def code_check(invocations, code=CODE_OK):
    return report.check_conformance(
        [run_of("b.pi.s0.t1", 0, rec(good_sampling(0)))],
        {**CLEAN, "repo": {"head": "h1", "dirty": False}},
        invocations=invocations,
        code=code,
    )


def test_k48_clean_code_gives_no_mismatch():
    assert code_check([inv(), inv()])["mismatches"] == []


def test_k48_a_changed_file_names_the_invocation_and_the_path():
    res = code_check([inv(), inv(head="h2")])
    (m,) = res["mismatches"]
    assert m["invocation"] == 1
    assert m["field"] == "code.changed"
    assert m["found"] == ["config/arms.toml"]


def test_k48_a_changed_file_at_report_time_is_named_report():
    code = {**CODE_OK, "report": {"head": "h2", "dirty_paths": []}}
    (m,) = code_check([inv()], code)["mismatches"]
    assert (m["invocation"], m["field"], m["found"]) == (
        "report",
        "code.changed",
        ["config/arms.toml"],
    )


def test_k48_a_dirty_invocation_gives_code_dirty_with_the_paths():
    res = code_check([inv(dirty=True, dirty_paths=["hcbench/report.py", "config/arms.toml"])])
    (m,) = res["mismatches"]
    assert (m["invocation"], m["field"]) == (0, "code.dirty")
    assert sorted(m["found"]) == ["config/arms.toml", "hcbench/report.py"]


def test_k48_a_dirty_rig_at_report_time_is_named_report():
    code = {**CODE_OK, "report": {"head": "h1", "dirty_paths": ["hcbench/stats.py"]}}
    (m,) = code_check([inv()], code)["mismatches"]
    assert (m["invocation"], m["field"], m["found"]) == (
        "report",
        "code.dirty",
        ["hcbench/stats.py"],
    )


def test_k48_a_head_git_cannot_resolve_gives_code_head():
    (m,) = code_check([inv(head="dead")])["mismatches"]
    assert (m["invocation"], m["field"], m["found"]) == (0, "code.head", "dead")


def test_k48_the_dirty_flag_and_the_head_value_alone_are_not_mismatches():
    assert code_check([inv(dirty=False, head="h1")])["mismatches"] == []


def test_k48_without_code_evidence_nothing_is_checked():
    res = report.check_conformance(
        [status_like()], {**CLEAN, "repo": {"head": "h1"}}, invocations=[inv(dirty_paths=["x"])]
    )
    assert res["mismatches"] == []


def status_like():
    return run_of("b.pi.s0.t1", 0, rec(good_sampling(0)))


# ---- K48 code state (real repositories) --------------------------------------------------


def rev(rig):
    return git(rig, "rev-parse", "HEAD").stdout.strip()


def code_rig(tmp_path):
    rig = make_rig(tmp_path)
    (rig / "prereg" / "camp").mkdir(parents=True)
    (rig / "prereg" / "camp" / "DEVIATIONS.md").write_text("none\n")
    git(rig, "init", "-q")
    git(rig, "add", "-A")
    git(rig, "commit", "-q", "-m", "c1")
    return rig, rev(rig)


def commit_change(rig, rel, text="\n# changed\n"):
    path = rig / rel
    path.write_text(path.read_text() + text)
    git(rig, "commit", "-q", "-am", f"change {rel}")
    return rev(rig)


def code_campaign(rig, head1, invocations):
    camp = rig / "results" / "camp"
    write_run(camp, b_run("pi", 0, started_at=START), [rec(good_sampling(0))])
    lines = [
        {"suite": "b", "started_at": START, "bundle": {}, "files": {}, "dirty": False,
         "dirty_paths": [], **i}
        for i in invocations
    ]  # fmt: skip
    (camp / "invocations.jsonl").write_text("".join(json.dumps(x) + "\n" for x in lines))
    pre = rig / "prereg" / "camp"
    (pre / "preregistration.toml").write_text(
        prereg_mod.dump_toml({**CLEAN, "repo": {"head": head1, "dirty": False}})
    )
    return camp, pre


def run_cli(rig, camp, pre):
    r = subprocess.run(
        [sys.executable, "-m", "hcbench.report", "--campaign", str(camp), "--prereg", str(pre),
         "--env-file", str(rig / "none.env")],
        cwd=rig, capture_output=True, text=True, check=False,
        env={**clean_git_env(), "PYTHONPATH": str(rig)},
    )  # fmt: skip
    md = (camp / "report.md").read_text() if (camp / "report.md").exists() else ""
    return r, md


def code_rows(md, field):
    return [ln for ln in md.splitlines() if f"| {field} |" in ln]


def test_k48_clean_repository_state_exits_0(tmp_path):
    rig, h1 = code_rig(tmp_path)
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, r.stderr + md
    assert "code." not in md


def test_k48_a_file_changed_after_the_preregistration_exits_3_naming_invocation_and_path(tmp_path):
    rig, h1 = code_rig(tmp_path)
    h2 = commit_change(rig, "config/arms.toml")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    rows = code_rows(md, "code.changed")
    assert any("invocation 0" in ln and "config/arms.toml" in ln for ln in rows)
    assert any("report" in ln and "config/arms.toml" in ln for ln in rows)  # HEAD is h2 too


def test_k48_the_report_run_itself_is_checked(tmp_path):
    rig, h1 = code_rig(tmp_path)
    commit_change(rig, "config/arms.toml")  # report runs at this newer head
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    rows = code_rows(md, "code.changed")
    assert len(rows) == 1 and "report" in rows[0] and "invocation" not in rows[0]


def test_k48_a_change_only_under_prereg_is_not_a_violation(tmp_path):
    rig, h1 = code_rig(tmp_path)
    h2 = commit_change(rig, "prereg/camp/DEVIATIONS.md", "| 2026-11-18 | x | y | z |\n")
    camp, pre = code_campaign(rig, h1, [{"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    assert "code." not in md


def test_k48_a_dirty_rig_at_report_time_exits_3_with_the_paths(tmp_path):
    rig, h1 = code_rig(tmp_path)
    (rig / "config" / "arms.toml").write_text("# edited, not committed\n")
    (rig / "prereg" / "camp" / "scratch.txt").write_text("ignored: under prereg/\n")
    camp, pre = code_campaign(rig, h1, [{"head": h1}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    (row,) = code_rows(md, "code.dirty")
    assert "report" in row and "config/arms.toml" in row
    assert "scratch.txt" not in md


def test_k48_a_dirty_invocation_exits_3_with_the_paths(tmp_path):
    rig, h1 = code_rig(tmp_path)
    camp, pre = code_campaign(
        rig, h1, [{"head": h1, "dirty": True, "dirty_paths": ["suites/b_mini.toml"]}]
    )
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    (row,) = code_rows(md, "code.dirty")
    assert "invocation 0" in row and "suites/b_mini.toml" in row


def test_k48_an_unresolvable_head_exits_3_with_code_head(tmp_path):
    rig, h1 = code_rig(tmp_path)
    camp, pre = code_campaign(rig, h1, [{"head": "deadbeef" * 5}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 3
    (row,) = code_rows(md, "code.head")
    assert "invocation 0" in row and "deadbeef" * 5 in row


def test_k48_conformance_section_lists_each_invocations_head_and_dirty_flag(tmp_path):
    rig, h1 = code_rig(tmp_path)
    h2 = commit_change(rig, "prereg/camp/DEVIATIONS.md", "x\n")
    camp, pre = code_campaign(rig, h1, [{"head": h1}, {"head": h2}])
    r, md = run_cli(rig, camp, pre)
    assert r.returncode == 0, md
    text = section(md, "Conformance")
    assert "invocation 0" in text and "invocation 1" in text
    assert h1 in text and h2 in text
    assert "dirty" in text.lower()


MATRIX_DRIVER = (
    "import sys\n"
    "from hcbench import matrix\n"
    "matrix.execute_run = lambda t, a, s, c, o, **kw: "
    "{'run_id': 'x', 'exit_code': 0, 'proxy': {}}\n"
    "sys.exit(matrix.main(sys.argv[1:]))\n"
)


def run_matrix_in(rig, tmp_path, extra=()):
    env = {**clean_git_env(), "PYTHONPATH": str(rig), **prereg_env(rig, tmp_path)}
    out = rig / "results" / "camp"
    r = subprocess.run(
        [sys.executable, "-c", MATRIX_DRIVER, "--suite", "b", "--arms", "pi", "--limit", "1",
         "--out", str(out), "--env-file", str(rig / "none.env"), *extra],
        cwd=rig, capture_output=True, text=True, env=env, check=False,
    )  # fmt: skip
    assert r.returncode == 0, r.stderr
    return [json.loads(ln) for ln in (out / "invocations.jsonl").read_text().splitlines()]


def test_k48_matrix_invocation_leaves_prereg_out_of_dirty_and_dirty_paths(tmp_path):
    rig, _ = code_rig(tmp_path)
    (rig / "prereg" / "camp" / "new_draft.txt").write_text("x\n")
    (line,) = run_matrix_in(rig, tmp_path)
    assert line["dirty"] is False
    assert line["dirty_paths"] == []
    assert line["head"] == rev(rig)


def test_k48_matrix_invocation_lists_the_dirty_paths_outside_prereg(tmp_path):
    rig, _ = code_rig(tmp_path)
    (rig / "config" / "new.toml").write_text("x = 1\n")
    (line,) = run_matrix_in(rig, tmp_path)
    assert line["dirty"] is True
    assert line["dirty_paths"] == ["config/new.toml"]


# ---- K49 overrides -----------------------------------------------------------------------


def test_k49_suite_file_and_arms_file_overrides_are_hashed_with_their_paths(tmp_path, monkeypatch):
    import hashlib

    from hcbench import matrix

    suite = tmp_path / "my_suite.toml"
    suite.write_text((ROOT / "suites" / "b_mini.toml").read_text() + "\n# local edit\n")
    arms = tmp_path / "my_arms.toml"
    arms.write_text((ROOT / "config" / "arms.toml").read_text() + "\n# local edit\n")
    monkeypatch.setenv("HC_BUNDLE_DIR", str(tmp_path / "nobundle"))
    monkeypatch.setattr(
        matrix,
        "execute_run",
        lambda task, arm, seed, cfg, out_root, **kw: {"run_id": "x", "exit_code": 0, "proxy": {}},
    )
    out = tmp_path / "out"
    matrix.main(
        ["--suite", "b", "--arms", "pi", "--limit", "1", "--out", str(out),
         "--suite-file", str(suite), "--arms-file", str(arms),
         "--env-file", str(tmp_path / "none.env")]
    )  # fmt: skip
    (line,) = [json.loads(ln) for ln in (out / "invocations.jsonl").read_text().splitlines()]
    assert line["files"]["suite_b"] == hashlib.sha256(suite.read_bytes()).hexdigest()
    assert line["files"]["arms_toml"] == hashlib.sha256(arms.read_bytes()).hexdigest()
    assert (
        line["files"]["suite_b"]
        != hashlib.sha256((ROOT / "suites" / "b_mini.toml").read_bytes()).hexdigest()
    )
    assert line["paths"]["suite_b"] == str(suite)
    assert line["paths"]["arms_toml"] == str(arms)
    # the other pre-registered files are still hashed from the rig
    assert "suite_c" in line["files"]


def test_k49_overridden_files_are_a_files_mismatch_and_default_files_are_not():
    pre = {**CLEAN, "files_sha256": {"arms_toml": "a" * 64, "suite_b": "b" * 64}}
    same = inv(files={"arms_toml": "a" * 64, "suite_b": "b" * 64})
    other = inv(
        files={"arms_toml": "a" * 64, "suite_b": "c" * 64}, paths={"suite_b": "/tmp/x.toml"}
    )
    runs = [status_like()]
    assert report.check_conformance(runs, pre, invocations=[same])["mismatches"] == []
    res = report.check_conformance(runs, pre, invocations=[same, other])
    assert fields(res) == ["files.suite_b"]
    assert res["mismatches"][0]["invocation"] == 1


# ---- K50 draft and scope -----------------------------------------------------------------


def test_k50_draft_flag_records_draft_true_even_without_placeholders(env):
    import tomllib

    from test_campaign1_prereg import run_cb

    pin(env)
    rc, out = run_cb(env, "--draft")
    assert rc == 0
    assert tomllib.loads(out.read_text())["draft"] is True


def test_k50_draft_flag_records_draft_true_for_stage_a_too(env):
    import tomllib

    pin(env)
    sel = env["tmp"] / "selection.json"
    sel.write_text("{}")
    out = env["tmp"] / "pa.toml"
    rc = prereg_mod.main(
        ["--stage", "a", "--selection", str(sel), "--out", str(out),
         "--env-file", str(ROOT / "config" / "bench.env.example"), "--draft"]
    )  # fmt: skip
    assert rc == 0
    assert tomllib.loads(out.read_text())["draft"] is True


def test_k50_without_draft_and_without_placeholders_the_file_is_not_a_draft(env):
    from test_campaign1_prereg import load_cb

    pin(env)
    assert load_cb(env)["draft"] is False


def test_k50_an_edit_under_hcbench_is_refused_even_when_out_points_into_hcbench(tmp_path):
    rig = git_rig(tmp_path)
    (rig / "hcbench" / "report.py").write_text(
        (rig / "hcbench" / "report.py").read_text() + "\n# edited\n"
    )
    r, out = prereg_cli(rig, tmp_path, out=rig / "hcbench" / "pre.toml")
    assert r.returncode != 0
    assert "hcbench/report.py" in r.stderr
    assert not out.exists()


def test_k50_an_out_path_elsewhere_does_not_hide_a_dirty_path(tmp_path):
    rig = git_rig(tmp_path)
    (rig / "suites" / "smoke.toml").write_text(
        (rig / "suites" / "smoke.toml").read_text() + "\n# edited\n"
    )
    r, _ = prereg_cli(rig, tmp_path, out=tmp_path / "elsewhere" / "pre.toml")
    assert r.returncode != 0
    assert "suites/smoke.toml" in r.stderr


def test_k50_a_non_draft_preregistration_is_refused_when_the_head_cannot_be_read(tmp_path):
    rig = make_rig(tmp_path)  # not a git repository: no head
    r, out = prereg_cli(rig, tmp_path)
    assert r.returncode != 0
    assert "head" in r.stderr.lower()
    assert not out.exists()


def test_k50_a_repository_without_commits_has_no_readable_head(tmp_path):
    rig = make_rig(tmp_path)
    git(rig, "init", "-q")
    r, _ = prereg_cli(rig, tmp_path)
    assert r.returncode != 0
    assert "head" in r.stderr.lower()


def test_k50_a_draft_may_be_written_without_a_readable_head(tmp_path):
    rig = make_rig(tmp_path)
    r, out = prereg_cli(rig, tmp_path, "--draft")
    assert r.returncode == 0, r.stderr
    assert out.exists()


# ---- K51 missing arms --------------------------------------------------------------------

PRE51 = {
    **CLEAN,
    "config": {"HC_SERVED_NAME": "qwen3.8-27b", "HC_ARMS_REDTEAM": SIX, "HC_ARMS_B": SIX},
}
GUARD_LEFT = [f"hone-guarded {MINUS} hone-inject", f"hermes-guarded {MINUS} hermes"]
LEFT = {
    "C-guard": [f"hermes-guarded {MINUS} hermes"],
    "C-cross": [f"hone-guarded {MINUS} hermes-guarded"],
    "B-guard": GUARD_LEFT,
}


def c_runs_only():
    return [run_of("c.pi.s0.rt01", 0, rec(good_sampling(0)))]


def test_k51_a_left_out_contrast_whose_arms_were_both_registered_is_a_mismatch():
    res = report.check_conformance(c_runs_only(), PRE51, left_out=LEFT)
    arm_rows = [m for m in res["mismatches"] if m["field"].startswith("arms.")]
    assert sorted(m["field"] for m in arm_rows) == ["arms.C-cross", "arms.C-guard"]
    by = {m["field"]: json.dumps(m, ensure_ascii=False) for m in arm_rows}
    assert f"hermes-guarded {MINUS} hermes" in by["arms.C-guard"]
    assert f"hone-guarded {MINUS} hermes-guarded" in by["arms.C-cross"]


def test_k51_no_mismatch_for_a_suite_with_no_runs_at_all():
    res = report.check_conformance(c_runs_only(), PRE51, left_out=LEFT)
    assert "arms.B-guard" not in fields(res)


def test_k51_suite_b_with_runs_gets_its_own_mismatch():
    runs = [*c_runs_only(), run_of("b.pi.s0.t1", 0, rec(good_sampling(0)))]
    res = report.check_conformance(runs, PRE51, left_out=LEFT)
    assert "arms.B-guard" in fields(res)


def test_k51_no_mismatch_when_the_arm_is_not_in_the_registered_list():
    config = {**PRE51["config"], "HC_ARMS_REDTEAM": "pi,pi-guarded,hone-inject,hone-guarded,hermes"}
    res = report.check_conformance(c_runs_only(), {**PRE51, "config": config}, left_out=LEFT)
    assert [f for f in fields(res) if f.startswith("arms.")] == []


def test_k51_without_left_out_nothing_is_checked():
    res = report.check_conformance(c_runs_only(), PRE51)
    assert [f for f in fields(res) if f.startswith("arms.")] == []


def test_k51_report_exits_3_when_a_registered_arm_has_no_runs(tmp_path):
    camp = build_campaign(tmp_path)
    for arm in ("pi", "pi-guarded"):
        write_run(camp, c_run(arm, 0))
    pre = write_prereg(tmp_path, {**PRE51, "stage": "cb"})
    assert run_report(camp, pre) == 3
    text = section((camp / "report.md").read_text(), "Conformance")
    assert "arms.C-guard" in text
    assert f"hermes-guarded {MINUS} hermes" in text
    assert "arms.B-guard" not in text


# ---- K52 labels --------------------------------------------------------------------------


def test_k52_the_tested_row_names_the_bound_it_was_tested_under(tmp_path):
    camp = build_campaign(tmp_path)
    write_run(camp, c_run("pi", 0, escaped=True))
    write_run(camp, c_run("pi-guarded", 0))
    run_report(camp, write_prereg(tmp_path, {**PREREG, "stage": "cb"}))
    text = section((camp / "report.md").read_text(), "Stage 1 analysis")
    (row,) = [ln for ln in text.splitlines() if ln.startswith(f"| pi-guarded {MINUS} pi | tested")]
    assert "against pi-guarded" in row  # favours pi-guarded: tested against it
