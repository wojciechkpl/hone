"""Spec section 11.17, round 15, K105-K109 and K124-K128 (from the review of round 14b).

Contract choices made here (the spec leaves them open):
* K105/K106: ``hcbench.matrix --suite a --image-digests FILE``; the file is required for suite
  A. Each task's ``image`` is looked up in the first column of the file and replaced by the
  pinned reference. The task environment follows the image: ``startswith("swerebench/")`` gets
  ``suites.REBENCH_PATH``, anything else ``suites.SWE_PATH``.
* K108: ``hcbench.grade`` is in fork mode when ``--image-digests`` is given. Flags:
  ``--namespace``, ``--cache-level``, ``--image-digests``, ``--grader-repo``,
  ``--grader-commit``. It writes ``grading/dataset.pinned.json`` and ``grading/grader.json``
  (keys ``repo``, ``commit``, ``namespace``, ``dataset_sha256`` of the pinned file), runs
  ``run_evaluation`` with ``--dataset_name <pinned file>``, ``--namespace`` and
  ``--cache_level instance``, and refuses an empty namespace or an unpinned row.
* K126: the preflight is checked with ``--accept-diff`` naming the config key under test, so
  only the environment-only check can refuse.
* K128: "no longer line" is read as: no prose line of the stage-2 section is longer than 100.
"""
# ruff: noqa: F811, F401  (the fixtures `hub` and `stage_a_env` are imported by name)

import hashlib
import json
import re
import subprocess
import tomllib

import pytest
from fakebin import ROOT, calls, docker_calls, make_fake_bin, make_rig, run_script
from test_campaign1_prereg import EXAMPLE, pin
from test_campaign1_round14 import (
    STAGE_IDS,
    do_select,
    hub,
    image_of,
    make_sel_dir,
    pinned,
    row,
    run_a,
    stage_a_env,
)
from test_campaign1_round14b import preflight, tree

from hcbench import grade, matrix, prereg
from hcbench.suites import REBENCH_PATH, SWE_PATH

FORK = "SWE-rebench/SWE-bench-fork"
REF = "e4907b7a90eafaa1f0a6428fd04fe31cdd8b4284"
NS = "swerebench"

# ---- K105 and K106: the matrix --------------------------------------------------------------


def write_inputs(d, images):
    """instances.jsonl, tasks.txt and images.digests for {task id: image}; every image pinned."""
    d.mkdir(parents=True, exist_ok=True)
    ids = sorted(images)
    (d / "tasks.txt").write_text("".join(f"{i}\n" for i in ids))
    (d / "instances.jsonl").write_text(
        "".join(
            json.dumps(
                {
                    "instance_id": i,
                    "repo": i.split("__")[0],
                    "base_commit": "c" * 40,
                    "problem_statement": f"fix {i}",
                    "created_at": "2026-03-22 09:18:02",
                    "split": "2026_03",
                    "image": images[i],
                }
            )
            + "\n"
            for i in ids
        )
    )
    (d / "images.digests").write_text("".join(f"{images[i]} {ref_of(images[i])}\n" for i in ids))


def ref_of(image):
    n = int(hashlib.sha256(image.encode()).hexdigest()[:8], 16)
    return image.rsplit(":", 1)[0] + "@sha256:" + f"{n:064x}"


def run_matrix(tmp_path, monkeypatch, d, *extra, digests=True, tasks_file=True):
    seen = []

    def fake_execute(task, arm, seed, cfg, out_root, **kw):
        seen.append(task)
        return {"run_id": f"a.{arm.name}.s{seed}.{task.task_id}", "exit_code": 0, "proxy": {}}

    monkeypatch.setattr(matrix, "execute_run", fake_execute)
    monkeypatch.setenv("HC_BUNDLE_DIR", str(tmp_path / "nobundle"))
    argv = [
        "--suite",
        "a",
        "--arms",
        "pi",
        "--seeds",
        "0",
        "--instances",
        str(d / "instances.jsonl"),
    ]
    if tasks_file:
        argv += ["--tasks-file", str(d / "tasks.txt")]
    if digests:
        argv += ["--image-digests", str(d / "images.digests")]
    argv += ["--out", str(tmp_path / "out"), "--env-file", str(tmp_path / "none.env"), *extra]
    return matrix.main(argv), seen


MIXED = {
    "a__a-1": "swerebench/sweb.eval.x86_64.a_1776_a-1:latest",
    "b__b-2": "swebench/sweb.eval.x86_64.b_1776_b-2:latest",
    "c__c-3": "docker.io/swerebench/sweb.eval.x86_64.c_1776_c-3:latest",
    "d__d-4": "myswerebench/sweb.eval.x86_64.d_1776_d-4:latest",
}


def test_k105_only_an_image_starting_with_swerebench_gets_the_rebench_path(tmp_path, monkeypatch):
    write_inputs(tmp_path / "sel", MIXED)
    rc, seen = run_matrix(tmp_path, monkeypatch, tmp_path / "sel")
    assert rc == 0
    paths = {t.task_id: t.env["PATH"] for t in seen}
    assert paths["a__a-1"] == REBENCH_PATH
    assert paths["b__b-2"] == SWE_PATH
    assert paths["c__c-3"] == SWE_PATH  # contains "swerebench/" but does not start with it
    assert paths["d__d-4"] == SWE_PATH
    assert REBENCH_PATH.startswith("/opt/conda/envs/testbed/bin:/opt/conda/bin:")
    assert REBENCH_PATH != SWE_PATH


def test_k106_each_task_should_run_the_pinned_reference(tmp_path, monkeypatch):
    write_inputs(tmp_path / "sel", MIXED)
    rc, seen = run_matrix(tmp_path, monkeypatch, tmp_path / "sel")
    assert rc == 0
    assert {t.task_id: t.image for t in seen} == {i: ref_of(img) for i, img in MIXED.items()}
    assert all("@sha256:" in t.image for t in seen)


def test_k106_dry_run_should_show_the_pinned_reference(tmp_path, monkeypatch, capsys):
    write_inputs(tmp_path / "sel", MIXED)
    run_matrix(tmp_path, monkeypatch, tmp_path / "sel", "--dry-run")
    out = capsys.readouterr().out
    assert f"image={ref_of(MIXED['a__a-1'])}" in out


def test_k106_a_task_without_a_pinned_line_should_stop_before_any_container(tmp_path, monkeypatch):
    write_inputs(tmp_path / "sel", MIXED)
    keep = [ln for ln in (tmp_path / "sel" / "images.digests").read_text().splitlines()
            if "b_1776_b-2" not in ln]  # fmt: skip
    (tmp_path / "sel" / "images.digests").write_text("\n".join(keep) + "\n")
    with pytest.raises(SystemExit) as exc:
        run_matrix(tmp_path, monkeypatch, tmp_path / "sel")
    assert "b__b-2" in str(exc.value) or "b_1776_b-2" in str(exc.value)
    assert not (tmp_path / "out").exists()


def test_k106_nothing_runs_when_a_digest_is_missing(tmp_path, monkeypatch):
    write_inputs(tmp_path / "sel", MIXED)
    (tmp_path / "sel" / "images.digests").write_text("")
    started = []
    monkeypatch.setattr(matrix, "execute_run", lambda *a, **k: started.append(a) or {})
    monkeypatch.setenv("HC_BUNDLE_DIR", str(tmp_path / "nobundle"))
    d = tmp_path / "sel"
    with pytest.raises(SystemExit):
        matrix.main(
            ["--suite", "a", "--arms", "pi", "--instances", str(d / "instances.jsonl"),
             "--tasks-file", str(d / "tasks.txt"), "--image-digests", str(d / "images.digests"),
             "--out", str(tmp_path / "out"), "--env-file", str(tmp_path / "none.env")]
        )  # fmt: skip
    assert started == []


@pytest.mark.parametrize(
    "bad",
    [
        "{image} MISSING",
        "{image} {image}",  # a tag, not a digest
        "{image} {ref} trailing",
        "{image} {ref}deadbeef",
    ],
)
def test_k106_a_malformed_line_should_not_fall_back_to_the_unpinned_image(
    tmp_path, monkeypatch, bad
):
    images = {"a__a-1": MIXED["a__a-1"]}
    write_inputs(tmp_path / "sel", images)
    image = images["a__a-1"]
    (tmp_path / "sel" / "images.digests").write_text(
        bad.format(image=image, ref=ref_of(image)) + "\n"
    )
    with pytest.raises(SystemExit):
        run_matrix(tmp_path, monkeypatch, tmp_path / "sel")


def test_k106_suite_a_should_refuse_to_run_without_image_digests(tmp_path, monkeypatch):
    write_inputs(tmp_path / "sel", {"a__a-1": MIXED["a__a-1"]})
    with pytest.raises(SystemExit) as exc:
        run_matrix(tmp_path, monkeypatch, tmp_path / "sel", digests=False)
    assert "image" in str(exc.value).lower()


def test_k106_only_the_tasks_in_tasks_txt_need_a_pinned_line(tmp_path, monkeypatch):
    write_inputs(tmp_path / "sel", MIXED)
    (tmp_path / "sel" / "tasks.txt").write_text("a__a-1\n")
    lines = (tmp_path / "sel" / "images.digests").read_text().splitlines()
    (tmp_path / "sel" / "images.digests").write_text(lines[0] + "\n")  # only a__a-1
    rc, seen = run_matrix(tmp_path, monkeypatch, tmp_path / "sel")
    assert rc == 0 and [t.task_id for t in seen] == ["a__a-1"]


# ---- K106 and K107: scripts/10_run.sh -------------------------------------------------------


def run_script_a(tmp_path, *, digests=True, env=None, args=None, name="camp"):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    d = rig / "prereg" / (env or {}).get("HC_PREREG", name)
    write_inputs(d, {"a__a-1": MIXED["a__a-1"]})
    if not digests:
        (d / "images.digests").unlink()
    full = {"FAKE_UV_STUB_MODULES": "hcbench.matrix", **(env or {})}
    r = run_script(rig, "10_run.sh", args or ["a", name, "--dry-run"], bindir, full)
    matrix_calls = [c for c in calls(log) if c[0] == "uv" and "hcbench.matrix" in c]
    return r, matrix_calls, d


def test_k107_run_a_should_use_the_main_arms_and_the_suite_a_repeats(tmp_path):
    env = {"HC_ARMS_MAIN": "pi,hone-inject", "HC_A_REPEATS": "0,2", "HC_REPEATS": "5",
           "HC_ARMS_B": "hermes"}  # fmt: skip
    r, (argv,), _ = run_script_a(tmp_path, env=env)
    assert r.returncode == 0, r.stderr
    assert argv[argv.index("--arms") + 1] == "pi,hone-inject"
    assert argv[argv.index("--seeds") + 1] == "0,2"  # not HC_REPEATS


def test_k107_run_a_should_follow_the_example_defaults(tmp_path):
    from hcbench.config import load_env_file

    vals = load_env_file(EXAMPLE, base={"HOME": "/h"})
    r, (argv,), _ = run_script_a(tmp_path)
    assert r.returncode == 0, r.stderr
    assert argv[argv.index("--arms") + 1] == vals["HC_ARMS_MAIN"]
    assert argv[argv.index("--seeds") + 1] == vals["HC_A_REPEATS"] == "0,1"


def test_k106_run_a_should_pass_the_prereg_digests(tmp_path):
    _, (argv,), d = run_script_a(tmp_path)
    assert argv[argv.index("--image-digests") + 1] == str(d / "images.digests")
    assert argv[argv.index("--instances") + 1] == str(d / "instances.jsonl")
    assert argv[argv.index("--tasks-file") + 1] == str(d / "tasks.txt")


def test_k106_run_a_should_follow_hc_prereg(tmp_path):
    _, (argv,), d = run_script_a(tmp_path, env={"HC_PREREG": "c1-a"})
    assert d.name == "c1-a"
    assert argv[argv.index("--image-digests") + 1] == str(d / "images.digests")


def test_k106_run_a_without_digests_should_stop_before_the_matrix(tmp_path):
    r, matrix_calls, _ = run_script_a(tmp_path, digests=False)
    assert r.returncode != 0
    assert "images.digests" in r.stdout + r.stderr
    assert matrix_calls == []


# ---- K108 grading ----------------------------------------------------------------------------


def grade_rig(tmp_path, *, grader=FORK, env=None):
    rig = make_rig(tmp_path)
    bindir, log = make_fake_bin(tmp_path)
    d = rig / "prereg" / "camp"
    d.mkdir(parents=True)
    if grader is not None:
        (d / "selection.json").write_text(json.dumps({"grader": grader}))
    (d / "dataset.json").write_text("[]")
    (d / "images.digests").write_text("")
    full = {"FAKE_UV_STUB_MODULES": "hcbench.grade", "HC_A_GRADER_REF": REF,
            "HC_A_NAMESPACE": NS, **(env or {})}  # fmt: skip
    r = run_script(rig, "20_grade.sh", ["camp"], bindir, full)
    return r, [c for c in calls(log) if c[0] == "uv"], d


def test_k108_script_should_install_the_fork_at_the_pinned_commit(tmp_path):
    r, uv, _ = grade_rig(tmp_path)
    assert r.returncode == 0, r.stderr
    (argv,) = uv
    text = " ".join(argv)
    assert f"swebench @ git+https://github.com/SWE-rebench/SWE-bench-fork.git@{REF}" in text
    assert "swebench==5.0.2" not in text


def test_k108_script_should_pass_the_fork_flags_to_grade(tmp_path):
    _, (argv,), d = grade_rig(tmp_path, env={"HC_A_NAMESPACE": "other-ns"})
    assert argv[argv.index("--namespace") + 1] == "other-ns"
    assert argv[argv.index("--cache-level") + 1] == "instance"
    assert argv[argv.index("--image-digests") + 1] == str(d / "images.digests")
    assert argv[argv.index("--dataset") + 1] == str(d / "dataset.json")


@pytest.mark.parametrize(
    "ref", ["", "e4907b7", REF.upper(), REF + "0", "g" * 40, "main", REF[:-1] + "Z"]
)
def test_k108_script_should_refuse_a_ref_that_is_not_40_lowercase_hex(tmp_path, ref):
    r, uv, _ = grade_rig(tmp_path, env={"HC_A_GRADER_REF": ref})
    assert r.returncode != 0
    assert "HC_A_GRADER_REF" in r.stdout + r.stderr
    assert uv == []


@pytest.mark.parametrize("grader", ["SWE-bench/other", None])
def test_k108_the_lite_path_should_keep_swebench_5_0_2(tmp_path, grader):
    r, (argv,), _ = grade_rig(tmp_path, grader=grader, env={"HC_A_GRADER_REF": ""})
    assert r.returncode == 0, r.stderr
    text = " ".join(argv)
    assert "swebench==5.0.2" in text
    assert "git+" not in text
    assert "--image-digests" not in argv


def make_campaign(tmp_path, ids, arm="pi", seed=0):
    camp = tmp_path / "results" / "camp"
    for i in ids:
        rdir = camp / "a" / i / f"s{seed}" / arm
        rdir.mkdir(parents=True)
        run = {"arm": arm, "seed": seed, "task_id": i, "run_id": f"a.{arm}.s{seed}.{i}"}
        (rdir / "run.json").write_text(json.dumps(run))
        (rdir / "patch.diff").write_text(f"diff --git a/{i} b/{i}\n")
    return camp


def fork_inputs(tmp_path, ids):
    d = tmp_path / "prereg"
    d.mkdir()
    rows = [row(i, i.split("__")[0]) for i in ids]
    (d / "dataset.json").write_text(json.dumps(rows))
    (d / "images.digests").write_text("".join(f"{image_of(i)} {pinned(i)}\n" for i in ids))
    return d, rows


class FakeEvaluate:
    """Stands in for `python -m swebench.harness.run_evaluation`."""

    def __init__(self, report=None):
        self.cmds, self.cwds = [], []
        self.report = report or {}

    def __call__(self, cmd, **kw):
        self.cmds.append(list(cmd))
        self.cwds.append(kw.get("cwd"))
        out = kw["cwd"] / "hcbench.report.json"
        out.write_text(json.dumps(self.report))
        return subprocess.CompletedProcess(cmd, 0)


def grade_args(camp, d, **over):
    flags = {
        "--campaign": str(camp),
        "--dataset": str(d / "dataset.json"),
        "--image-digests": str(d / "images.digests"),
        "--namespace": NS,
        "--cache-level": "instance",
        "--grader-repo": FORK,
        "--grader-commit": REF,
        "--max-workers": "2",
    }
    flags.update(over)
    return [x for k, v in flags.items() if v is not None for x in (k, v)]


IDS3 = ["a__a-1", "b__b-2", "c__c-3"]


def flag(cmd, name):
    return cmd[cmd.index(name) + 1]


def test_k108_grade_should_run_the_fork_on_the_pinned_dataset_file(tmp_path, monkeypatch):
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    ev = FakeEvaluate({"resolved_ids": IDS3})
    monkeypatch.setattr(grade.subprocess, "run", ev)
    assert grade.main(grade_args(camp, d)) == 0
    pinned_file = camp / "grading" / "dataset.pinned.json"
    (cmd,) = ev.cmds
    assert flag(cmd, "--dataset_name") == str(pinned_file) or flag(cmd, "--dataset_name") == str(
        pinned_file.resolve()
    )
    assert flag(cmd, "--namespace") == NS
    assert flag(cmd, "--cache_level") == "instance"
    assert flag(cmd, "--dataset_name") != str(d / "dataset.json")  # not the unpinned rows


def test_k108_pinned_dataset_should_replace_both_image_fields_only(tmp_path, monkeypatch):
    camp = make_campaign(tmp_path, IDS3)
    d, rows = fork_inputs(tmp_path, IDS3)
    monkeypatch.setattr(grade.subprocess, "run", FakeEvaluate({"resolved_ids": IDS3}))
    grade.main(grade_args(camp, d))
    got = json.loads((camp / "grading" / "dataset.pinned.json").read_text())
    assert [r["instance_id"] for r in got] == IDS3
    for src, new in zip(rows, got, strict=True):
        ref = pinned(src["instance_id"])
        assert new["image_name"] == ref and new["docker_image"] == ref
        for key in src:
            if key not in ("image_name", "docker_image"):
                assert new[key] == src[key], key  # FAIL_TO_PASS stays a list
    # The unpinned file is left as it was.
    assert json.loads((d / "dataset.json").read_text()) == rows


def test_k108_grader_json_should_record_repo_commit_namespace_and_the_dataset_hash(
    tmp_path, monkeypatch
):
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    monkeypatch.setattr(grade.subprocess, "run", FakeEvaluate({"resolved_ids": IDS3}))
    grade.main(grade_args(camp, d))
    rec = json.loads((camp / "grading" / "grader.json").read_text())
    pinned_bytes = (camp / "grading" / "dataset.pinned.json").read_bytes()
    assert rec["repo"] == FORK and rec["commit"] == REF and rec["namespace"] == NS
    assert rec["dataset_sha256"] == hashlib.sha256(pinned_bytes).hexdigest()


def test_k108_grade_should_refuse_a_row_with_no_pinned_reference(tmp_path, monkeypatch):
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    lines = [ln for ln in (d / "images.digests").read_text().splitlines() if "b_1776_b-2" not in ln]
    (d / "images.digests").write_text("\n".join(lines) + "\n")
    ev = FakeEvaluate({"resolved_ids": IDS3})
    monkeypatch.setattr(grade.subprocess, "run", ev)
    with pytest.raises(SystemExit) as exc:
        grade.main(grade_args(camp, d))
    assert "b__b-2" in str(exc.value)
    assert ev.cmds == []


@pytest.mark.parametrize("line_tail", [" MISSING", " {image}", " {ref} trailing"])
def test_k108_a_malformed_digest_line_should_not_pin_the_row(tmp_path, monkeypatch, line_tail):
    camp = make_campaign(tmp_path, IDS3[:1])
    d, _ = fork_inputs(tmp_path, IDS3[:1])
    image = image_of(IDS3[0])
    (d / "images.digests").write_text(
        image + line_tail.format(image=image, ref=pinned(IDS3[0])) + "\n"
    )
    ev = FakeEvaluate()
    monkeypatch.setattr(grade.subprocess, "run", ev)
    with pytest.raises(SystemExit):
        grade.main(grade_args(camp, d))
    assert ev.cmds == []


@pytest.mark.parametrize("namespace", ["", "   ", None])
def test_k108_grade_should_never_pass_an_empty_namespace(tmp_path, monkeypatch, namespace):
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    ev = FakeEvaluate()
    monkeypatch.setattr(grade.subprocess, "run", ev)
    with pytest.raises(SystemExit) as exc:
        grade.main(grade_args(camp, d, **{"--namespace": namespace}))
    assert "namespace" in str(exc.value).lower()
    assert ev.cmds == []


def test_k108_the_lite_path_should_stay_as_it_was(tmp_path, monkeypatch):
    camp = make_campaign(tmp_path, IDS3)
    d, _ = fork_inputs(tmp_path, IDS3)
    ev = FakeEvaluate({"resolved_ids": IDS3})
    monkeypatch.setattr(grade.subprocess, "run", ev)
    args = ["--campaign", str(camp), "--dataset", str(d / "dataset.json"), "--split", "test"]
    assert grade.main(args) == 0
    (cmd,) = ev.cmds
    assert flag(cmd, "--dataset_name") == str(d / "dataset.json")
    assert flag(cmd, "--split") == "test"
    assert "--namespace" not in cmd and "--cache_level" not in cmd
    assert not (camp / "grading" / "dataset.pinned.json").exists()
    assert not (camp / "grading" / "grader.json").exists()


# ---- K109 status mapping for the fork's report -------------------------------------------------


def test_k109_a_fork_report_should_map_every_list(tmp_path):
    report = {
        "resolved_ids": ["r"],
        "unresolved_ids": ["u"],
        "empty_patch_ids": ["e"],
        "error_ids": ["x"],
        "incomplete_ids": ["i"],
    }  # no infra_failure_ids or ambiguous_failure_ids, as the fork writes it
    status = {k: grade.status_for(k, report, tmp_path, "rid") for k in "rueXi"}
    assert status == {
        "r": "resolved", "u": "unresolved", "e": "empty_patch", "X": "missing", "i": "missing",
    }  # fmt: skip
    assert grade.status_for("x", report, tmp_path, "rid") == "error"


def test_k109_apply_failure_should_still_be_found_by_its_log_line(tmp_path):
    log = tmp_path / "logs" / "run_evaluation" / "rid" / "model" / "t__t-1" / "run_instance.log"
    log.parent.mkdir(parents=True)
    log.write_text("...\n>>>>> Patch Apply Failed\n")
    report = {"error_ids": ["t__t-1"], "resolved_ids": []}
    assert grade.status_for("t__t-1", report, tmp_path, "rid") == "apply_failed"


def test_k109_an_id_only_in_incomplete_ids_or_in_no_list_should_be_missing_and_regraded(
    tmp_path, monkeypatch
):
    camp = make_campaign(tmp_path, ["a__a-1", "b__b-2", "c__c-3"])
    d, _ = fork_inputs(tmp_path, ["a__a-1", "b__b-2", "c__c-3"])
    report = {"resolved_ids": ["a__a-1"], "incomplete_ids": ["b__b-2"]}  # c__c-3 in no list
    monkeypatch.setattr(grade.subprocess, "run", FakeEvaluate(report))
    assert grade.main(grade_args(camp, d)) == 1  # re-grade needed
    csv_rows = (camp / "grading" / "outcomes.csv").read_text().splitlines()[1:]
    by_task = {ln.split(",")[0].split(".")[-1]: ln.split(",") for ln in csv_rows}
    assert by_task["a__a-1"][2:] == ["1", "resolved"]
    assert by_task["b__b-2"][2:] == ["", "missing"]
    assert by_task["c__c-3"][2:] == ["", "missing"]


# ---- K124 a malformed bundle manifest ----------------------------------------------------------


def only_pi_arms(env):
    for k in ("HC_ARMS_MAIN", "HC_ARMS_REDTEAM", "HC_ARMS_B"):
        env["mp"].setenv(k, "pi")


@pytest.mark.parametrize("draft", [True, False])
@pytest.mark.parametrize("kind", ["garbage", "directory"])
def test_k124_stage_cb_malformed_manifest_should_stop_naming_the_file(env, draft, kind):
    pin(env)
    only_pi_arms(env)
    manifest = env["tmp"] / "bundle" / "manifest.json"
    if kind == "garbage":
        manifest.write_text("{not json")
    else:
        manifest.unlink()
        manifest.mkdir()
    args = ["--stage", "cb", "--out", str(env["tmp"] / "p.toml"), "--env-file", str(EXAMPLE)]
    with pytest.raises(SystemExit) as exc:
        prereg.main([*args, *(["--draft"] if draft else [])])
    assert "manifest.json" in str(exc.value)
    assert not (env["tmp"] / "p.toml").exists()


@pytest.mark.parametrize("kind", ["garbage", "directory"])
def test_k124_stage_a_malformed_manifest_should_stop_naming_the_file(stage_a_env, kind):
    manifest = stage_a_env["env"]["tmp"] / "bundle" / "manifest.json"
    if kind == "garbage":
        manifest.write_text("{not json")
    else:
        manifest.unlink()
        manifest.mkdir()
    with pytest.raises(SystemExit) as exc:
        run_a(stage_a_env)
    assert "manifest.json" in str(exc.value)


# ---- K125 a new selection starts clean --------------------------------------------------------


def test_k125_select_should_remove_a_left_over_digests_and_check_file(hub, tmp_path):
    out = tmp_path / "s"
    out.mkdir()
    (out / "images.digests").write_text("old-image old@sha256:" + "0" * 64 + "\n")
    (out / "images.check.json").write_text(json.dumps({"ghost__task-1": {"ok": True}}))
    do_select(out, n=4)
    assert not (out / "images.digests").exists()
    assert not (out / "images.check.json").exists()
    assert (out / "tasks.txt").exists()


# ---- K126 the preflight runs every check that needs only the environment ----------------------


@pytest.mark.parametrize("ref", ["", "e4907b7", REF.upper(), REF + "0", "g" * 40])
def test_k126_preflight_should_refuse_a_bad_grader_ref(stage_a_env, ref):
    stage_a_env["env"]["mp"].setenv("HC_A_GRADER_REF", ref)
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env)
    assert "HC_A_GRADER_REF" in str(exc.value)


def test_k126_preflight_should_refuse_an_empty_namespace(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_A_NAMESPACE", "")
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env)
    assert "HC_A_NAMESPACE" in str(exc.value)


def test_k126_preflight_in_draft_mode_should_not_enforce_the_k99_checks(stage_a_env):
    # The final step enforces K99 only for a non-draft file; the preflight follows it.
    stage_a_env["env"]["mp"].setenv("HC_A_GRADER_REF", "short")
    stage_a_env["env"]["mp"].setenv("HC_A_NAMESPACE", "")
    assert preflight(stage_a_env, "--draft") == 0


def test_k126_preflight_should_refuse_a_placeholder(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_MODEL_REVISION", "CHANGE-ME-commit-sha")
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env, "--accept-diff", "HC_MODEL_REVISION")  # only the placeholder check
    assert "HC_MODEL_REVISION" in str(exc.value)
    assert preflight(stage_a_env, "--accept-diff", "HC_MODEL_REVISION", "--draft") == 0


def test_k126_preflight_should_check_the_hermes_bundle_even_in_draft(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_HERMES_SHA", "f" * 40)
    for extra in ([], ["--draft"]):
        with pytest.raises(SystemExit) as exc:
            preflight(stage_a_env, "--accept-diff", "HC_HERMES_SHA", *extra)
        assert "hermes" in str(exc.value).lower()


def test_k126_preflight_should_check_the_pi_bundle_only_when_not_a_draft(stage_a_env):
    stage_a_env["env"]["mp"].setenv("HC_PI_VERSION", "9.9.9")
    with pytest.raises(SystemExit) as exc:
        preflight(stage_a_env, "--accept-diff", "HC_PI_VERSION")
    assert "pi" in str(exc.value).lower()
    assert preflight(stage_a_env, "--accept-diff", "HC_PI_VERSION", "--draft") == 0


def test_k126_preflight_should_still_write_nothing(stage_a_env):
    tmp = stage_a_env["env"]["tmp"]
    before = tree(tmp)
    stage_a_env["env"]["mp"].setenv("HC_A_GRADER_REF", "short")
    with pytest.raises(SystemExit):
        preflight(stage_a_env)
    assert tree(tmp) == before


# ---- K127 consistent inputs ---------------------------------------------------------------------


def stage_a_script(tmp_path, args):
    from test_campaign1_round14 import A_ARGS, stage_a_rig

    rig, bindir, log, env, d = stage_a_rig(tmp_path, preplace=False)
    r = run_script(rig, "05_preregister.sh", [*A_ARGS, *args], bindir, env)
    return r, log, d


@pytest.mark.parametrize("args", [["--accept-diff="], ["--accept-diff", ""], ["--accept-diff"]])
def test_k127_script_should_stop_on_an_empty_or_missing_accept_diff_value(tmp_path, args):
    r, log, d = stage_a_script(tmp_path, args)
    assert r.returncode != 0
    assert "accept-diff" in (r.stdout + r.stderr)
    assert calls(log) == []  # not even the preflight ran
    assert not d.exists()


def test_k127_script_should_accept_a_named_accept_diff_in_both_spellings(tmp_path):
    from test_campaign1_round14 import A_ARGS, stage_a_rig

    for k, args in enumerate((["--accept-diff", "bundle"], ["--accept-diff=bundle"])):
        rig, bindir, _, env, _ = stage_a_rig(tmp_path / f"run{k}")
        r = run_script(rig, "05_preregister.sh", [*A_ARGS, *args], bindir, env)
        assert r.returncode == 0, r.stderr


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("HC_A_DATASET", "nebius/SWE-rebench-leaderboard "),
        ("HC_A_MIN_SPLIT", " 2026_03"),
        ("HC_A_MIN_CREATED", "2026-03-01 "),
        ("HC_SELECTION_SEED", " 20261109 "),
        ("HC_A_N", " 60 "),
        ("HC_A_GRADER", "SWE-rebench/SWE-bench-fork "),
    ],
)
def test_k127_preflight_and_final_check_should_agree_on_padded_values(stage_a_env, key, value):
    def outcome(fn):
        try:
            fn()
        except SystemExit:
            return "refused"
        return "accepted"

    stage_a_env["env"]["mp"].setenv(key, value)
    first = outcome(lambda: preflight(stage_a_env))
    second = outcome(lambda: run_a(stage_a_env))
    assert first == second


# ---- K128 README ---------------------------------------------------------------------------------


def stage_two_section():
    lines = (ROOT / "README.md").read_text().split("\n")
    start = next(i for i, ln in enumerate(lines) if ln.startswith("## Campaign 1, stage 2"))
    end = next(i for i in range(start + 1, len(lines)) if lines[i].startswith("## "))
    return lines[start:end]


def test_k128_readme_should_mention_the_image_check_refusal():
    text = " ".join(stage_two_section()).lower()
    assert "images.check.json" in text and "run_digest" in text
    assert "refus" in text


def test_k128_readme_stage_two_prose_lines_should_not_be_long():
    prose, fenced = [], False
    for ln in stage_two_section():
        if ln.startswith("```"):
            fenced = not fenced
        elif not fenced and "http" not in ln and not ln.startswith("|"):
            prose.append(ln)
    assert max(map(len, prose)) <= 100, [ln for ln in prose if len(ln) > 100]
